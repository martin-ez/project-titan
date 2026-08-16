#!/usr/bin/env bash
# scripts/track.sh — work tracking for titan.
#
# The ONLY supported way to read or change GitHub Issues in this repo. See AGENTS.md.
#
# Never call `gh issue` / `gh api` / `gh search` directly: the legacy issue-search
# index silently ignores `is:blocked` and `no:blocked-by` and returns blocked issues
# as if they were ready, with a 200 OK and no error. This script never touches the
# search index — it derives readiness locally from each issue's blockedBy payload,
# which is also read-your-writes consistent (the search index lags writes by seconds).
#
# Structured output -> stdout.  Progress and diagnostics -> stderr.
# Exit codes:
#   0  success
#   1  error — caller surfaces stderr verbatim and stops
#   2  claim contention — someone else holds it; pick a different issue
#
# Written for bash 3.2 (macOS /bin/bash): no associative arrays, no mapfile.

set -euo pipefail

ISSUE_FIELDS='number,title,state,url,labels,blockedBy,blocking,parent,subIssues,subIssuesSummary'
LIST_LIMIT="${TRACK_LIMIT:-200}"
TITLE_MAX="${TRACK_TITLE_MAX:-70}"
MIN_WRITE_GAP="${TRACK_MIN_WRITE_GAP:-1}"
STALE_HOURS="${TRACK_STALE_HOURS:-24}"

# What one selftest run spends of the account's 5000-point hourly GraphQL
# budget: 751 points, measured across a full run against this repository. The
# headroom above that is for the graph, which every `ready` in a run reads whole
# and which is what the figure scales with, so a run against a larger tracker
# costs more than the one this was measured on.
SELFTEST_COST="${TRACK_SELFTEST_COST:-1000}"

# The open set every derivation reads, when the selftest is supplying one. Set
# from a literal here rather than the environment on purpose: sourced from
# `${...:-}` an exported variable would make `ready` answer off a fixture and
# report a queue that does not exist.
ST_FIXTURE=""

# The rate limit the budget preflight reads, when the selftest is supplying
# one. A literal here for the same reason as ST_FIXTURE: sourced from the
# environment, a figure left over in a shell would talk a real run out of
# starting, or into one the hour cannot pay for.
ST_RATE=""

die()  { printf 'ERROR: %s\n' "$*" >&2; exit 1; }
note() { printf '%s\n' "$*" >&2; }

usage() {
  cat >&2 <<'USAGE'
scripts/track.sh <command> [args] [--json]

  ready                       work that can be started right now
  refs [-F FILE]              issues a pull request body tracks (stdin default)
  blocked                     open work with open blockers, and what blocks it
  plan [--all]                the epic chain in order, the current ones opened
  find <term>                 match issue titles, open and closed
  show <n>                    one issue in full, including claim state
  start <n>                   claim, then branch from main onto it
  claim <n> [--force]         take an issue (adds wip + a claim marker)
  mine                        issues this agent currently holds
  submit <n> [--pr N] [--force]  built, now waiting on a human merge
  release <n> [--force]       give it back
  done <n> [-m MSG] [--force] close it, report what it unblocked
  reopen <n> [-m MSG]         undo a close, report what it re-blocked
  add -t TITLE --area A --kind K --size S --parent N [-b BODY|-F FILE]
      [--blocked-by N,...] [--blocking N,...]
      (an epic is --size l: no --parent, but --blocked-by the chain's open end)
  dep <n> [--needs N] [--drop-needs N] [--parent N] [--child N] [--drop-child N]
  note <n> -m MSG             leave a comment on an issue
  graph                       dependency forest of open issues
  labels-init                 create/update the label taxonomy (idempotent)
  doctor                      check preconditions
  selftest --yes              full lifecycle smoke test on throwaway issues
  selftest --clean [<marker>] --yes  delete throwaway issues, one run's or all
USAGE
  exit 1
}

# ----------------------------------------------------------------- labels ---
LABEL_SPEC='area:sim|1f6feb|Traffic, rovers, pathfinding, production, resources
area:world|388bfd|Hex grid, terrain, roads, buildings, placement
area:ui|58a6ff|HUD, menus, build tools, input, camera
area:render|a5d6ff|Meshes, materials, shaders, visual effects
area:infra|8b949e|Build, CI, tooling, dependencies
kind:feat|2da44e|New capability
kind:bug|d1242f|Something is wrong
kind:chore|6e7781|Maintenance, refactor, dependency bumps
kind:design|d876e3|Game design or balance: recipes, costs, progression
kind:spike|8250df|Time-boxed investigation, output is throwaway
size:s|ededed|Well under one agent session
size:m|d0d7de|About one agent session
size:l|afb8c1|Too big to claim — split into sub-issues first
wip|fbca04|Claimed by an agent. Set by track.sh claim only.
review|d4c5f9|Built and waiting on a human merge. Set by track.sh submit only.
track:selftest|c5def5|Throwaway issue from track.sh selftest. Safe to delete.'

label_names() { printf '%s\n' "$LABEL_SPEC" | awk -F'|' 'NF{print $1}'; }
valid_label()  { label_names | grep -qx -- "$1"; }
label_values() { label_names | grep "^$1:" | sed "s/^$1://" | tr '\n' ' '; }

# --------------------------------------------------------------- identity ---
repo_key() {
  local main
  main="$(git worktree list 2>/dev/null | awk 'NR==1{print $1}')"
  [ -n "$main" ] || return 1
  printf '%s' "$main" | shasum | cut -c1-12
}

agent_id() {
  if [ -n "${TITAN_AGENT:-}" ]; then validate_agent "$TITAN_AGENT"; printf '%s' "$TITAN_AGENT"; return 0; fi
  local br
  br="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || true)"
  case "$br" in
    ''|HEAD)     die "cannot determine agent id — set TITAN_AGENT." ;;
    main|master) die "refusing to act as agent '$br'. Work on a branch, or set TITAN_AGENT." ;;
  esac
  printf '%s' "$br"
}

# Which claims belong to this agent. The id in a claim marker is the branch that
# recorded it, so ownership is a question about branches: the one checked out
# here is this agent's, and one that no worktree holds is work a crashed session
# left behind — the case `mine` exists to answer. A branch checked out in another
# worktree belongs to the agent working there.
#
# Compared by branch name rather than worktree path, because the same worktree
# reached through a symlink yields a path that compares unequal to itself.
foreign_branches() {
  local cur
  cur="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || true)"
  git worktree list --porcelain 2>/dev/null | awk -v cur="$cur" '
    /^branch / { b = substr($0, 8); sub(/^refs\/heads\//, "", b)
                 if (b != "" && b != cur) print b }'
  return 0
}

# TITAN_AGENT names an agent outright, so it answers alone: a caller asking what
# another agent holds must not also be told about the branches lying around here.
held_agent_ids() {
  if [ -n "${TITAN_AGENT:-}" ]; then
    validate_agent "$TITAN_AGENT"
    printf '%s\n' "$TITAN_AGENT"
    return 0
  fi
  { printf 'main\nmaster\n'; foreign_branches; printf -- '--\n'
    git for-each-ref --format='%(refname:short)' refs/heads/ 2>/dev/null; } \
  | awk '/^--$/ { owned = 1; next }
         !owned    { skip[$0] = 1; next }
         !($0 in skip)'
  return 0
}

# `die` inside a command substitution exits that subshell, so `agent_id || true`
# never reaches its fallback: the assignment carries the failure out and set -e
# ends the script with no message to surface. The extra subshell is what makes
# the failure catchable.
agent_id_or_empty() {
  ( agent_id ) 2>/dev/null || true
}

# Who is acting on a claim. One this checkout owns is settled as the branch that
# recorded it, so `release` and `done` can act on everything `mine` reports —
# the skill tells the agent to finish or release exactly that list. Anything else
# falls back to the branch identity, so taking another agent's claim still needs
# --force. Prints nothing when neither is available.
#
# The id set is computed by the caller, before it takes the lock: deriving it
# here could die holding the lock and leave it behind.
acting_agent() {
  local holder="$1" ids="$2"
  if [ -n "$holder" ] && printf '%s\n' "$ids" | grep -qxF -- "$holder"; then
    printf '%s' "$holder"
    return 0
  fi
  agent_id_or_empty
}

# The claim marker parses the agent with [^ ]+, so whitespace would produce a
# marker that can never be matched back: the claim would look successful and be
# invisible to every other command.
validate_agent() {
  case "$1" in
    *[[:space:]]*) die "agent id '$1' contains whitespace; use [A-Za-z0-9._/-] only." ;;
    '')            die "agent id is empty." ;;
  esac
  return 0
}

# ------------------------------------------------- lock + write pacing ------
# mkdir is atomic on any POSIX filesystem (flock is unreliable on macOS).
# Held across read-then-write, this makes `claim` a genuine compare-and-swap.
# It also paces every content-generating request >=1s apart, globally across
# all agents on this machine, satisfying the 80/min and 500/hour secondary limit.
# Resolved on first use, not at load time: computing it eagerly makes --help and
# doctor -- the things you reach for when something is wrong -- fail outside a repo.
STATE_DIR=""
LOCK=""
STAMP=""
LOCK_HELD=0

state_init() {
  [ -n "$STATE_DIR" ] && return 0
  local k
  k="$(repo_key)" || die "not inside a git repository."
  STATE_DIR="${TMPDIR:-/tmp}/titan-track-$k"
  LOCK="$STATE_DIR/lock"
  STAMP="$STATE_DIR/last-write"
  return 0
}

# Whether a recorded holder is one this run left behind, and so has to be broken
# rather than waited on.
#
# What the lock records is the run, not the shell holding it: `$$` is the
# invoking shell's pid inside a subshell, BASHPID is bash 4 and unset on the
# macOS /bin/bash this is written for, and every tracker command reaches the
# lock through a `$( )` or a `( )`. So a lock a subshell orphans names a process
# that is still alive -- this one -- and the wait below would sit on it forever.
# Nothing in a run holds the lock concurrently with anything else in it, so a
# hold recorded against this pid is one no shell is still inside: reaching here
# at all means LOCK_HELD is 0 and this shell is not the holder.
lock_is_ours_to_break() { [ "$1" = "$$" ]; }

lock_acquire() {
  [ "$LOCK_HELD" = 1 ] && return 0
  state_init
  mkdir -p "$STATE_DIR"
  local tries=0 holder=""
  until mkdir "$LOCK" 2>/dev/null; do
    holder="$(cat "$LOCK/pid" 2>/dev/null || true)"
    if lock_is_ours_to_break "$holder"; then
      note "WARNING: breaking a lock this run left behind (pid $holder)"
      rm -rf "$LOCK"
    else
      tries=$((tries + 1))
      if [ "$tries" -gt 300 ]; then
        # A long hold is normal: labels-init and selftest legitimately keep the
        # lock for a minute across paced writes. Only break it once the recorded
        # holder is gone, and reset the counter afterwards -- breaking on every
        # subsequent tick would delete a live lock and let two callers hold it.
        # An unreadable holder belongs here rather than above: it is most often
        # the window between another run's mkdir and its stamp.
        if [ -n "$holder" ] && kill -0 "$holder" 2>/dev/null; then
          note "waiting on live lock holder pid $holder …"
        else
          note "WARNING: breaking a lock whose holder (${holder:-unknown}) is gone"
          rm -rf "$LOCK"
        fi
        tries=0
      fi
    fi
    sleep 0.2
  done
  LOCK_HELD=1
  # The run's pid, which is all `$$` has ever been able to say -- see
  # lock_is_ours_to_break for what reads it and why that is the useful identity.
  printf '%s' "$$" > "$LOCK/pid" 2>/dev/null || true
  return 0
}

lock_release() {
  [ "$LOCK_HELD" = 1 ] || return 0
  rm -rf "$LOCK"
  LOCK_HELD=0
  return 0
}
trap lock_release EXIT INT TERM

# Every mutating gh call goes through here. Never call `gh` directly for writes.
gh_write() {
  lock_acquire
  local now last delta rc
  now="$(date +%s)"
  last="$(cat "$STAMP" 2>/dev/null || echo 0)"
  delta=$(( now - last ))
  if [ "$delta" -lt "$MIN_WRITE_GAP" ]; then
    sleep $(( MIN_WRITE_GAP - delta ))
  fi
  rc=0
  gh "$@" || rc=$?
  date +%s > "$STAMP"
  return "$rc"
}

# ------------------------------------------------------------- jq library ---
JQ_LIB='
def _lbl($p): (.labels // []) | map(.name) | map(select(startswith($p)))
              | (.[0] // "") | ltrimstr($p);
def _has($n): (((.labels // []) | map(.name) | index($n)) != null);
def _trunc: ((.blockedBy.totalCount // 0) > ((.blockedBy.nodes // []) | length))
            or ((.subIssues.totalCount // 0) > ((.subIssues.nodes // []) | length));
def _openblk:  (.blockedBy.nodes // []) | map(select(.state == "OPEN"));
def _doneblk:  (.blockedBy.nodes // []) | map(select(.state == "CLOSED"));
def _openbing: (.blocking.nodes  // []) | map(select(.state == "OPEN"));
def _opensub:  (.subIssues.nodes // []) | map(select(.state == "OPEN"));

def shape: {
  num:       .number,
  title:     ((.title // "") | gsub("[\t\r\n]"; " ")),
  state:     .state,
  area:      _lbl("area:"),
  kind:      _lbl("kind:"),
  size:      _lbl("size:"),
  wip:       _has("wip"),
  review:    _has("review"),
  blockers:  (_openblk  | map(.number)),
  done_deps: (_doneblk  | map(.number)),
  unblocks:  (_openbing | map(.number)),
  subs_open: (_opensub  | map(.number)),
  subs:      (.subIssuesSummary // {total: 0, completed: 0}),
  trunc:     _trunc,
  parent:    (if .parent == null then null
              else {num: .parent.number, title: .parent.title, state: .parent.state} end),
  url:       .url
};

# Readiness is inherited: work under a blocked epic is not startable however
# clear its own edges are. That is what carries an order stated between epics
# down to the issues an agent actually claims, without an edge from every child
# to whatever precedes the epic it belongs to -- the parent says that already,
# once, and an edge per child is the same fact copied and hand-maintained.
#
# The walk stops at an ancestor that is closed, because a finished epic gates
# nothing, and at a depth no real tree reaches, so a parent cycle cannot spin
# here. `gated_by` names the nearest ancestor carrying the blocker, not the
# blocker, since that is the row a reader has to go and look at.
def with_gates:
  (INDEX(.[]; .num | tostring)) as $by
  | map(. + { gated_by:
      ( [ limit(8; recurse(
            if (.parent != null) and ($by[.parent.num | tostring] != null)
            then $by[.parent.num | tostring] else empty end)) ]
        | .[1:]
        | map(select((.blockers | length) > 0))
        | (.[0].num // null) ) });

def is_gated: (.state == "OPEN") and (.gated_by != null);
def is_ready:   (.state == "OPEN") and (.wip | not) and (.trunc | not)
                and (.size != "l") and (is_gated | not)
                and ((.blockers | length) == 0) and ((.subs_open | length) == 0);
def needs_split: (.state == "OPEN") and (.wip | not) and (.size == "l")
                and (is_gated | not)
                and ((.blockers | length) == 0) and ((.subs_open | length) == 0);
def is_container: (.state == "OPEN") and ((.subs_open | length) > 0);
def is_blocked: (.state == "OPEN") and ((.blockers | length) > 0);
def epic_is_current: ((.blockers | length) == 0) and (.gated_by == null);

# An open end is an epic no other open epic comes off: the chain stops there, so
# a new epic extends it from one of these. Being startable has nothing to do with
# it -- a new epic follows the last one filed, not the last one anybody can pick
# up. Which epic is an end is a fact about what points at it, so like with_gates
# it is computed over the set rather than read off the epic.
def with_ends:
  ([ .[] | select(.size == "l") | .blockers[] ]) as $taken
  | map(.num as $n
        | . + {chain_end: ((.size == "l") and (($taken | index($n)) == null))});
def epic_is_end: (.chain_end // false);

# Kahn peeling: repeatedly drop nodes whose remaining blockers are all satisfied.
# Whatever survives is in a cycle or downstream of one. Blockers are first
# restricted to issues we actually fetched, so an out-of-window blocker cannot
# masquerade as a cycle.
def cycle_nodes:
  ([.[] | .num]) as $known
  | [ .[] | {num: .num, blk: [ .blockers[] | select(IN($known[])) ]} ] as $init
  | ( reduce range(0; ($init | length) + 1) as $_ ($init;
        (map(select(.blk | length == 0) | .num)) as $free
        | if ($free | length) == 0 then .
          else [ .[] | select((.blk | length) > 0) | {num: .num, blk: (.blk - $free)} ]
          end) )
  | map(.num) | sort;
def has_cycle: ((cycle_nodes | length) > 0);

# The graph the repository owns. A selftest run files a chain of throwaway
# issues and edits their edges, and a run that dies partway leaves them behind
# until someone cleans up, so a check that fails on any fault anywhere fails
# every other run over work it did not create and cannot see.
# Throwaway issues carry a label saying whose they are, and are nobody elses.
def repo_own: map(select(_has("track:selftest") | not));

def top_blockers:
  [.[] | select(is_blocked) | .blockers[]]
  | group_by(.) | map({num: .[0], n: length}) | sort_by(-.n, .num);

# A reader brings one question to an epic: what do I do about this? Its children
# answer in the order the answers apply -- take it, someone is on it, someone has
# to merge it, nothing to do here yet. Number order answers nothing and buries
# the only startable row among the ones nobody can act on.
def board_order:
  sort_by({"ready": 0, "claimed": 1, "review": 2, "waiting": 3}[.stance] // 4, .num);

# The plan is a chain, not a forest: epics come off each other in one order, so
# peeling the ones nothing holds up, over and over, is the order to read them
# in. Only edges between epics count -- an epic waiting on a loose bug is still
# next. Whatever survives the peel is in a cycle, and goes last rather than
# vanishing out of the list.
def epic_order:
  ([ .[] | select(.size == "l") | .num ]) as $enums
  | ([ .[] | select(.size == "l")
       | . + {ebl: [ .blockers[] | select(IN($enums[])) ]} ]) as $init
  | ( reduce range(0; ($init | length)) as $_
        ({rest: $init, out: []};
          ([ .rest[] | select((.ebl | length) == 0) ] | sort_by(.num)) as $free
          | ([ $free[] | .num ]) as $freed
          | if ($free | length) == 0 then .
            else { out: (.out + $free),
                   rest: [ .rest[] | select((.ebl | length) > 0)
                           | .ebl = (.ebl - $freed) ] }
            end) )
  | (.out + .rest);
'

# Claim ownership lives in HTML-comment markers on issue comments. They arrive on
# the same `gh issue view` call as everything else, so reading them is free.
JQ_CLAIM='
def markers:
  [ (.comments // [])[]
    | (.body | capture("<!-- track:(?<ev>claim|release|done) agent=(?<agent>[^ ]+) -->")?)
      as $m
    | select($m != null)
    | {ev: $m.ev, agent: $m.agent, at: .createdAt} ];
def holder:
  (markers | last) as $m
  | if $m == null or $m.ev != "claim" then null
    else {agent: $m.agent, since: $m.at} end;

# Read only under the label, which is what a release or a merge clears. A marker
# outlives the claim cycle that wrote it, so an issue released and taken again
# would otherwise come back already in review.
def submitted:
  (((.labels // []) | map(.name) | index("review")) != null) as $lbl
  | ( [ (.comments // [])[]
        | (.body | capture("<!-- track:submit agent=(?<agent>[^ ]+)( pr=(?<pr>[0-9]+))? -->")?)
          as $m
        | select($m != null)
        | {agent: $m.agent, pr: $m.pr, at: .createdAt} ] | last ) as $s
  | if ($lbl | not) or $s == null then null
    else {agent: $s.agent,
          pr: (if $s.pr == null then null else ($s.pr | tonumber) end),
          since: $s.at} end;
'

# ------------------------------------------------------------ read paths ----
# The only three places this script reads issue data. No search query anywhere:
# `find` matches locally over fetch_all for the same reason readiness is derived
# locally — the legacy index answers `is:blocked` wrongly with a 200, and there is
# no reason to trust its title matching any further than that.
fetch_open()  {
  if [ -n "$ST_FIXTURE" ]; then printf '%s' "$ST_FIXTURE"; return 0; fi
  gh issue list --state open --limit "$LIST_LIMIT" --json "$ISSUE_FIELDS"
}
fetch_all()   { gh issue list --state all  --limit "$LIST_LIMIT" --json "$ISSUE_FIELDS"; }
fetch_issue() { gh issue view "$1" --json "$ISSUE_FIELDS,body,comments"; }

# Readiness is inherited (see `with_gates`), and `claim` reads one issue rather
# than the whole set, so it walks the parent chain itself instead. Bounded at a
# depth no real tree reaches, so a parent cycle stops here rather than spinning,
# and it stops at a closed ancestor because a finished epic gates nothing.
# Prints the nearest open ancestor carrying a blocker, or nothing.
gating_ancestor() {
  local num="$1" depth=0 info
  while [ -n "$num" ] && [ "$depth" -lt 8 ]; do
    info="$(fetch_issue "$num" | jq -c "$JQ_LIB"' shape')"
    if [ "$(printf '%s' "$info" | jq -r '.blockers | length')" -gt 0 ]; then
      printf '%s' "$num"
      return 0
    fi
    num="$(printf '%s' "$info" | jq -r '
      if (.parent != null) and (.parent.state == "OPEN") then .parent.num else "" end')"
    depth=$((depth + 1))
  done
  return 0
}

# `add --parent` and `dep --child` hand a number to `gh`, which takes a closed
# issue as a parent without complaint. Readiness is inherited through the parent
# and every walk stops at a closed ancestor -- correctly, since a finished epic
# gates nothing -- so the child comes out gated by nothing and startable ahead of
# the work it belongs to, and `plan`, which lists the children of open epics,
# does not show it under any epic at all.
#
# Silent when the parent is open. A number that does not resolve is a different
# mistake from one that resolves to closed work, and says so; which of "no such
# issue" and "GitHub is unreachable" it was is left to gh's own error, since the
# two are not separable here without guessing at its prose.
require_open_parent() {   # require_open_parent <num> <ctx>
  local n="$1" ctx="$2" info state title
  info="$(fetch_issue "$n" | jq -c "$JQ_LIB"' shape')" \
    || die "$ctx: could not read #$n — see the gh error above.
A parent has to be an open issue, and this number did not resolve to one.
  scripts/track.sh show $n"
  state="$(printf '%s' "$info" | jq -r '.state')"
  title="$(printf '%s' "$info" | jq -r '.title')"
  [ "$state" = OPEN ] || die "$ctx: #$n is $state — \"$title\".
Readiness is inherited through the parent and the walk stops at a closed
ancestor, so work under #$n is gated by nothing and startable ahead of the chain
it belongs to, and \`plan\` lists the children of open epics only, so it appears
under no epic at all.
Point it at an open issue, or reopen this one if it was closed by mistake:
  scripts/track.sh plan        the chain, and which epics are startable
  scripts/track.sh reopen $n   if its work is not in main after all"
  return 0
}

# Claim markers live on comments, which `fetch_open` does not carry. Fetching
# every open issue to find them would cost one call per issue; the wip label is
# already in the list payload, so the walk is bounded by the number of live
# claims instead of the size of the backlog.
claimed_issues() {
  local nums n
  nums="$(fetch_open | jq -r '.[] | select((.labels // []) | map(.name) | index("wip")) | .number')"
  for n in $nums; do
    fetch_issue "$n" | jq -c "$JQ_LIB$JQ_CLAIM"' shape + {claim: holder, submit: submitted}'
  done
  return 0
}

# GitHub gives an issue one parent, so `--drop-child` does not move a child --
# it removes the only parent it has. Readiness is inherited through that parent,
# so a non-epic dropped out of its chain is gated by nothing and startable ahead
# of the work it belongs after: exactly the state `add` refuses to file, reached
# from the other side. An epic is a root and takes no parent, so dropping one is
# not that mistake and is left alone.
#
# Silent when the child is an epic. A number that does not resolve is a
# different mistake from one that resolves to work in a chain, and says so.
require_epic_to_drop() {   # require_epic_to_drop <child>
  local n="$1" info size title
  info="$(fetch_issue "$n" | jq -c "$JQ_LIB"' shape')" \
    || die "dep --drop-child: could not read #$n — see the gh error above.
  scripts/track.sh show $n"
  size="$(printf '%s' "$info" | jq -r '.size')"
  title="$(printf '%s' "$info" | jq -r '.title')"
  [ "$size" != l ] || return 0
  die "dep --drop-child: #$n is not an epic — \"$title\".
GitHub gives an issue one parent, so this removes the only one #$n has rather
than moving it: readiness is inherited through the parent, so it would come out
gated by nothing and startable ahead of the chain it belongs to — the state
\`add\` refuses to file.
Re-point it instead, which moves it in one write and never leaves it loose:
  scripts/track.sh dep $n --parent <epic>
  scripts/track.sh plan   the chain, and which epics are startable"
}

# The chain `plan` reads, rendered for a refusal: every open epic in the order
# they come off each other, marked by whichever predicate the refusal is about —
# startable for one missing --parent, an open end for one missing --blocked-by.
# Marked rather than filtered, because a caller shown only the marked rows hangs
# the work off whichever epic happens to carry the mark — which is the queue jump
# the refusal exists to stop, wearing a legitimate edge. Non-zero means the read
# failed, which is a different answer from a chain that has no head yet.
epic_chain() {
  local mark="${1:-epic_is_current}"
  fetch_open | jq -r "$JQ_LIB"'
    [.[] | shape] | with_gates | with_ends | epic_order
    | .[] | "  \(if '"$mark"' then "▸" else " " end) #\(.num)  \(.title)"'
}

# The refusal an epic filed with no --blocked-by earns, given that chain with its
# open ends marked. Returned rather than died on, so the decision can be reached
# with a chain this repository cannot be put into: no open end at all is an empty
# tracker, where the first root is the one epic that does come off nothing.
epic_head_refusal() {
  local title="$1" ends="$2"
  [ -n "$ends" ] || return 0
  cat <<REFUSAL
add: '$title' is an epic, so it needs --blocked-by.
The chain is the ordering mechanism: an epic that names nothing it comes off is
gated by nothing, and neither is anything filed under it.
The chain, in the order the epics come off each other — ▸ is an open end:
$ends
Re-run with --blocked-by <n> for the epic this one follows, an open end unless it
belongs mid-chain.
  scripts/track.sh plan   the same chain, with the work under each epic
REFUSAL
  return 1
}

# GNU date takes -d, BSD date takes -j -f. A timestamp that parses under neither
# yields 0, which every caller reads as "unknown age" and skips.
iso_epoch() {
  date -u -d "$1" +%s 2>/dev/null \
    || date -u -j -f '%Y-%m-%dT%H:%M:%SZ' "$1" +%s 2>/dev/null \
    || echo 0
}

# Points left on this hour's GraphQL budget and the epoch it turns over at, as
# "<remaining> <reset>". Asking is free: the rate_limit endpoint is charged
# against no limit, and it is not the issue search index this file's header
# forbids.
graphql_budget() {
  if [ -n "$ST_RATE" ]; then printf '%s' "$ST_RATE"; return 0; fi
  gh api rate_limit --jq '.resources.graphql | "\(.remaining) \(.reset)"'
}

# When a rate limit window turns over, as the phrase that follows "resets".
# Rounded up, because the number is read as how long there is to wait.
until_reset() {   # until_reset <reset-epoch> <now-epoch>
  local left at
  left=$(( $1 - $2 ))
  if [ "$left" -le 0 ]; then printf 'now'; return 0; fi
  at="$(date -u -d "@$1" +%H:%MZ 2>/dev/null \
        || date -u -r "$1" +%H:%MZ 2>/dev/null \
        || printf '??:??Z')"
  printf '%s (in %dm)' "$at" $(( (left + 59) / 60 ))
}

# Refuse a selftest run this hour's budget cannot cover, while it has still
# filed nothing. A run that exhausts the budget half way through dies where
# cleanup needs the points it has just spent, so it leaves its throwaway issues
# behind -- and every assertion after the failure reports against an issue that
# was never created, which reads exactly like a concurrent run deleting
# fixtures rather than like a rate limit.
budget_preflight() {
  local b remaining reset
  b="$(graphql_budget)" || b=""
  remaining="${b%% *}"
  reset="${b##* }"
  case "$remaining" in ''|*[!0-9]*) die "cannot read the GraphQL rate limit: '$b'" ;; esac
  case "$reset"     in ''|*[!0-9]*) die "cannot read the GraphQL rate limit: '$b'" ;; esac
  [ "$remaining" -ge "$SELFTEST_COST" ] && return 0
  die "this hour's GraphQL budget will not cover a selftest run.
$remaining point(s) left, and a run costs about $SELFTEST_COST. Nothing has been filed.
The budget resets $(until_reset "$reset" "$(date -u +%s)").
Or set TRACK_SELFTEST_COST to a cost you have measured yourself."
}

# --------------------------------------------------------------- commands ---
cmd_ready() {
  local shaped payload total n nblocked nwip ncont tb cyc split trunc
  shaped="$(fetch_open | jq "$JQ_LIB"' [.[] | shape] | with_gates')"
  total="$(printf '%s' "$shaped" | jq 'length')"
  payload="$(printf '%s' "$shaped" | jq "$JQ_LIB"'
    [.[] | select(is_ready)] | sort_by(-(.unblocks | length), .num)')"

  if [ "$AS_JSON" = 1 ]; then printf '%s\n' "$payload"; return 0; fi

  # A cycle can strand part of the backlog while other work is still ready, so
  # this is reported whether or not the queue is empty.
  cyc="$(printf '%s' "$shaped" | jq -r "$JQ_LIB"'cycle_nodes | map("#\(.)") | join(" ")')"

  n="$(printf '%s' "$payload" | jq 'length')"
  printf 'ready %s/%s open\n' "$n" "$total"

  if [ "$n" -gt 0 ]; then
    printf '%s' "$payload" | jq -r --argjson m "$TITLE_MAX" '
      .[] | [ .num, (.size // ""), (.area // ""), (.kind // ""), (.unblocks | length),
              (if (.title | length) > $m then (.title[0:$m] + "…") else .title end)
            ] | @tsv' \
    | awk -F'\t' '{
        u = ($5 + 0 > 0) ? sprintf("  (unblocks %d)", $5) : "";
        printf "  #%-4s %-1s  %-7s %-6s %s%s\n",
               $1, ($2 == "" ? "-" : $2), ($3 == "" ? "-" : $3), ($4 == "" ? "-" : $4), $6, u;
      }'
  else
    # Empty queue: say why, on stdout, where the agent will actually read it.
    # Containers are counted separately -- an open, unclaimed, unblocked issue
    # with open children is in neither of the other buckets.
    nblocked="$(printf '%s' "$shaped" | jq "$JQ_LIB"'[.[] | select(is_blocked or is_gated)] | length')"
    nwip="$(printf '%s' "$shaped" | jq '[.[] | select(.wip)] | length')"
    ncont="$(printf '%s' "$shaped" | jq "$JQ_LIB"'[.[] | select(is_container)] | length')"
    printf '  nothing is ready. %s blocked, %s claimed, %s waiting on sub-issues.\n' \
      "$nblocked" "$nwip" "$ncont"
    tb="$(printf '%s' "$shaped" | jq -r "$JQ_LIB"'
          top_blockers[0:5] | map("#\(.num) (\(.n))") | join("  ")')"
    [ -n "$tb" ] && printf '  top blockers: %s\n' "$tb"
  fi

  # size:l is excluded from `ready` because `claim` refuses it. Say so, or the
  # queue looks empty while the work sits there.
  split="$(printf '%s' "$shaped" | jq -r "$JQ_LIB"'[.[] | select(needs_split) | .num]
           | map("#\(.)") | join(" ")')"
  [ -n "$split" ] && printf '  SPLIT: %s are size:l — break them up with add --parent.\n' "$split"
  trunc="$(printf '%s' "$shaped" | jq -r '[.[] | select(.trunc) | .num] | map("#\(.)") | join(" ")')"
  [ -n "$trunc" ] && printf '  TRUNCATED: %s have more relations than gh returns; treat as unknown.\n' "$trunc"
  [ -n "$cyc" ] && printf '  CYCLE: %s can never become ready. Run: scripts/track.sh graph\n' "$cyc"
  [ "$total" -ge "$LIST_LIMIT" ] && printf '  LIMIT: %s open issues fetched; raise TRACK_LIMIT, work may be hidden.\n' "$total"
  return 0
}

cmd_blocked() {
  local shaped payload total n tb
  shaped="$(fetch_open | jq "$JQ_LIB"' [.[] | shape] | with_gates')"
  total="$(printf '%s' "$shaped" | jq 'length')"
  payload="$(printf '%s' "$shaped" | jq "$JQ_LIB"'
    [.[] | select(is_blocked or is_gated)] | sort_by(.num)')"

  if [ "$AS_JSON" = 1 ]; then printf '%s\n' "$payload"; return 0; fi

  n="$(printf '%s' "$payload" | jq 'length')"
  printf 'blocked %s/%s open\n' "$n" "$total"
  printf '%s' "$payload" | jq -r --argjson m "$TITLE_MAX" '
    .[] | [ .num, (.size // ""), (.area // ""), (.kind // ""),
            (if (.title | length) > $m then (.title[0:$m] + "…") else .title end),
            (((.blockers | map("#\(.)"))
              + (if .gated_by != null then ["via #\(.gated_by)"] else [] end))
             | join(" "))
          ] | @tsv' \
  | awk -F'\t' '{
      printf "  #%-4s %-1s  %-7s %-6s %s  <- %s\n",
             $1, ($2 == "" ? "-" : $2), ($3 == "" ? "-" : $3), ($4 == "" ? "-" : $4), $5, $6;
    }'
  tb="$(printf '%s' "$shaped" | jq -r "$JQ_LIB"'
        top_blockers[0:5] | map("#\(.num) (\(.n))") | join("  ")')"
  [ -n "$tb" ] && printf 'top blockers: %s\n' "$tb"
  return 0
}

cmd_plan() {
  local all=0 shaped payload total nready
  while [ $# -gt 0 ]; do
    case "$1" in
      --all) all=1; shift ;;
      *)     die "usage: track.sh plan [--all]" ;;
    esac
  done

  shaped="$(fetch_open | jq "$JQ_LIB"' [.[] | shape] | with_gates')"
  total="$( printf '%s' "$shaped" | jq 'length')"
  nready="$(printf '%s' "$shaped" | jq "$JQ_LIB"'[.[] | select(is_ready)] | length')"

  payload="$(printf '%s' "$shaped" | jq "$JQ_LIB"'
    . as $open
    | epic_order
    | map(. as $e
        | ( [ $open[] | select((.parent != null) and (.parent.num == $e.num)) ]
            | sort_by(.num) ) as $kids
        | { num: $e.num, title: $e.title, area: $e.area,
            done: $e.subs.completed, of: $e.subs.total,
            review: ([ $kids[] | select(.review) ] | length),
            waits: $e.blockers,
            current: ($e | epic_is_current),
            children:
              ( $kids
                | map(. as $c
                      | { num: $c.num, size: $c.size, title: $c.title,
                          stance: (if $c.review then "review"
                                   elif $c.wip then "claimed"
                                   elif ($c | is_ready) then "ready"
                                   else "waiting" end),
                          waits: ((($c.blockers | map("#\(.)"))
                                   + (if ($c.gated_by != null)
                                         and (($c.blockers | index($c.gated_by)) == null)
                                      then ["via #\($c.gated_by)"] else [] end))
                                  | join(" ")) })
                | board_order ) })')"

  if [ "$AS_JSON" = 1 ]; then printf '%s\n' "$payload"; return 0; fi

  printf 'plan (%s open, %s ready)\n' "$total" "$nready"
  printf '%s' "$payload" | jq -r --argjson all "$all" --argjson m "$TITLE_MAX" '
    .[]
    | ([ "E", (.num | tostring),
         (if (.title | length) > $m then (.title[0:$m] + "…") else .title end),
         (.done | tostring), (.of | tostring),
         (.waits | map("#\(.)") | join(" ")),
         (if .current then "yes" else "no" end),
         (.review | tostring) ] | @tsv),
      ( if (.current or ($all == 1))
        then (.children[]
              | [ "C", (.num | tostring), (.size // "-"), .stance,
                  (if (.title | length) > $m then (.title[0:$m] + "…") else .title end),
                  .waits ] | @tsv)
        else empty end )' \
  | awk -F'\t' -v all="$all" '
      $1 == "E" {
        open_here = ($7 == "yes" || all == 1);
        mark = ($7 == "yes") ? "\342\226\270" : " ";
        prog = ($5 + 0 > 0) ? sprintf("%s/%s done", $4, $5) : "";
        if ($8 + 0 > 0)
          prog = prog ((prog == "") ? "" : ", ") $8 " in review";
        waits = ($6 == "") ? "" : sprintf("  waits on %s", $6);
        line = sprintf("%s #%-4s %-44s %-9s%s", mark, $2, $3, prog, waits);
        sub(/[ \t]+$/, "", line);
        printf "%s%s\n", (open_here ? "\n" : ""), line;
      }
      $1 == "C" {
        w = ($6 == "") ? "" : sprintf("  <- %s", $6);
        printf "     %-8s #%-4s %-1s  %s%s\n", $4, $2, ($3 == "" ? "-" : $3), $5, w;
      }'
  return 0
}

cmd_find() {
  [ $# -ge 1 ] || die "usage: track.sh find <term>"
  local term="$1" payload n total
  payload="$(fetch_all | jq "$JQ_LIB"' [.[] | shape]')"
  total="$(printf '%s' "$payload" | jq 'length')"
  payload="$(printf '%s' "$payload" | jq --arg t "$term" '
    [.[] | select(.title | ascii_downcase | contains($t | ascii_downcase))]
    | sort_by(.num) | reverse')"

  if [ "$AS_JSON" = 1 ]; then printf '%s\n' "$payload"; return 0; fi

  n="$(printf '%s' "$payload" | jq 'length')"
  printf 'find %s match(es) for "%s"\n' "$n" "$term"
  printf '%s' "$payload" | jq -r --argjson m "$TITLE_MAX" '
    .[] | [ .num, .state, (.size // ""), (.area // ""),
            (if (.title | length) > $m then (.title[0:$m] + "…") else .title end) ]
        | @tsv' \
  | awk -F'\t' '{ printf "  #%-4s %-6s %-1s  %-7s %s\n",
                  $1, tolower($2), ($3 == "" ? "-" : $3), ($4 == "" ? "-" : $4), $5 }'
  [ "$total" -ge "$LIST_LIMIT" ] && printf '  LIMIT: %s issues fetched; raise TRACK_LIMIT, matches may be hidden.\n' "$total"
  return 0
}

cmd_mine() {
  local all held n ids where elsewhere
  ids="$(held_agent_ids | jq -R . | jq -sc 'unique')"
  all="$(claimed_issues | jq -sc '.')"
  held="$(printf '%s' "$all" | jq -c --argjson ids "$ids" \
          '[.[] | select(.claim.agent as $a | $ids | index($a))]')"

  if [ "$AS_JSON" = 1 ]; then printf '%s\n' "$held"; return 0; fi

  n="$(printf '%s' "$held" | jq 'length')"
  where="${TITAN_AGENT:-$(git rev-parse --show-toplevel 2>/dev/null || true)}"
  printf 'mine %s held in %s\n' "$n" "${where:-this checkout}"
  printf '%s' "$held" | jq -r --argjson m "$TITLE_MAX" '
    .[] | [ .num, (.size // ""), (.area // ""), .claim.since,
            (if (.title | length) > $m then (.title[0:$m] + "…") else .title end),
            (if .review then "review" else "building" end),
            (if (.submit.pr // null) != null then "  pr #\(.submit.pr)" else "" end) ]
        | @tsv' \
  | awk -F'\t' '{ printf "  #%-4s %-1s  %-7s %-8s %s  since %s%s\n",
                  $1, ($2 == "" ? "-" : $2), ($3 == "" ? "-" : $3), $6, $5, $4, $7 }'

  # A claim held from another worktree looks exactly like one held by an agent
  # that crashed there, and nothing local can tell them apart. Saying where they
  # are reported beats answering 0 and stopping.
  elsewhere="$(printf '%s' "$all" | jq --argjson ids "$ids" \
               '[.[] | select(.claim.agent as $a | $ids | index($a) | not)] | length')"
  if [ "$n" = 0 ] && [ "$elsewhere" != 0 ]; then
    printf '  %s claim(s) held elsewhere; doctor lists them with age.\n' "$elsewhere"
  fi
  return 0
}

# The branch name doubles as the agent id, so it is built from the character set
# the claim marker parses with: anything else would produce a claim that can
# never be matched back.
slugify() {
  printf '%s' "$1" | tr '[:upper:]' '[:lower:]' \
    | sed -e 's/[^a-z0-9]\{1,\}/-/g' -e 's/^-//' -e 's/-$//' | cut -c1-40 | sed -e 's/-$//'
}

branch_for() {   # branch_for <kind> <num> <title>
  local kind="$1" num="$2" title="$3"
  printf '%s/%s-%s' "${kind:-task}" "$num" "$(slugify "$title")"
}

cmd_start() {
  [ $# -ge 1 ] || die "usage: track.sh start <n>"
  local n="${1#\#}" info kind title branch created=0 rc=0

  git rev-parse --git-dir >/dev/null 2>&1 || die "not inside a git repository."
  git diff --quiet && git diff --cached --quiet \
    || die "working tree has uncommitted changes. Finish or stash them first."
  git show-ref --verify --quiet refs/heads/main || die "no local 'main' to branch from."

  info="$(fetch_issue "$n" | jq -c "$JQ_LIB"' shape')"
  kind="$( printf '%s' "$info" | jq -r '.kind  // ""')"
  title="$(printf '%s' "$info" | jq -r '.title // ""')"
  [ -n "$title" ] || die "#$n has no title — is it a real issue?"
  branch="$(branch_for "$kind" "$n" "$title")"

  # Claim before branching, not after. `claim` only needs the branch name to
  # derive an agent id, and TITAN_AGENT supplies that directly — so a contended
  # issue never leaves a branch behind to roll back, which is the common case
  # whenever two agents reach for the same row.
  # cmd_claim exits on a fatal error, so it runs in a subshell.
  rc=0
  ( export TITAN_AGENT="$branch"; cmd_claim "$n" ) || rc=$?
  [ "$rc" -eq 0 ] || exit "$rc"

  if git show-ref --verify --quiet "refs/heads/$branch"; then
    git switch "$branch" >/dev/null 2>&1 || rc=$?
  else
    git switch -c "$branch" main >/dev/null 2>&1 || rc=$?
    created=1
  fi
  if [ "$rc" -ne 0 ]; then
    ( export TITAN_AGENT="$branch"; cmd_release "$n" ) >/dev/null 2>&1 || true
    die "claimed #$n but could not switch to $branch — the claim has been released."
  fi

  printf 'started #%s on %s\n' "$n" "$branch"
  [ "$created" = 1 ] || note "note: $branch already existed; it was not recreated from main."
  return 0
}

cmd_show() {
  [ $# -ge 1 ] || die "usage: track.sh show <n>"
  local payload
  payload="$(fetch_issue "${1#\#}" | jq "$JQ_LIB$JQ_CLAIM"'
    shape + {body: (.body // ""), claim: holder, submit: submitted}')"

  if [ "$AS_JSON" = 1 ]; then printf '%s\n' "$payload"; return 0; fi

  printf '%s' "$payload" | jq -r '
    def line($k; $v): if ($v | length) > 0
                      then "\($k)\(" " * (8 - ($k | length)))\($v)" else empty end;
    "#\(.num)  \(.state)  area=\(if .area == "" then "-" else .area end) kind=\(if .kind == "" then "-" else .kind end) size=\(if .size == "" then "-" else .size end)",
    line("title"; .title),
    line("url";   .url),
    (if .claim != null then "claim   \(.claim.agent)  since \(.claim.since)" else empty end),
    (if .submit != null
       then "review  built, waiting on a human merge" +
            (if .submit.pr != null then " of #\(.submit.pr)" else "" end) +
            "  since \(.submit.since)"
       else empty end),
    (if .parent != null then "parent  #\(.parent.num) \(.parent.title)" else empty end),
    (if .subs.total > 0
       then "subs    \(.subs.completed)/\(.subs.total) done" +
            (if (.subs_open | length) > 0
               then ", open: " + (.subs_open | map("#\(.)") | join(" ")) else "" end)
       else empty end),
    ("needs   " +
      (if (.blockers | length) > 0 then (.blockers | map("#\(.)") | join(" "))
       else "(none open)" end) +
      (if (.done_deps | length) > 0
         then "   done: " + (.done_deps | map("#\(.)") | join(" ")) else "" end)),
    (if (.unblocks | length) > 0
       then "blocks  " + (.unblocks | map("#\(.)") | join(" ")) else empty end),
    "--- body",
    .body'
  return 0
}

cmd_claim() {
  local force=0 n="" me info state wip size blockers subs holder since gate rc=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --force) force=1; shift ;;
      -*)      die "unknown flag for claim: $1" ;;
      *)       n="${1#\#}"; shift ;;
    esac
  done
  [ -n "$n" ] || die "usage: track.sh claim <n> [--force]"
  me="$(agent_id)"

  lock_acquire                        # held across read+write => real CAS

  info="$(fetch_issue "$n" | jq -c "$JQ_LIB$JQ_CLAIM"' shape + {claim: holder}')"
  state="$(   printf '%s' "$info" | jq -r '.state')"
  wip="$(     printf '%s' "$info" | jq -r '.wip')"
  size="$(    printf '%s' "$info" | jq -r '.size // ""')"
  blockers="$(printf '%s' "$info" | jq -r '.blockers  | map("#\(.)") | join(" ")')"
  subs="$(    printf '%s' "$info" | jq -r '.subs_open | map("#\(.)") | join(" ")')"
  holder="$(  printf '%s' "$info" | jq -r '.claim.agent // ""')"

  [ "$state" = "OPEN" ] || { lock_release; die "#$n is $state — nothing to claim."; }
  [ -z "$blockers" ]    || { lock_release; die "#$n is blocked by $blockers. Work on a blocker instead."; }
  [ -z "$subs" ]        || { lock_release; die "#$n is a container (open sub-issues: $subs). Claim a sub-issue."; }

  gate="$(gating_ancestor "$(printf '%s' "$info" | jq -r '
    if (.parent != null) and (.parent.state == "OPEN") then .parent.num else "" end')")"
  [ -z "$gate" ] || { lock_release
    die "#$n sits under #$gate, which is blocked. Work on what blocks #$gate instead."; }

  if [ "$wip" = "true" ]; then
    if [ "$holder" = "$me" ]; then
      lock_release
      printf 'claimed #%s agent=%s (already yours)\n' "$n" "$me"
      return 0
    fi
    lock_release
    since="$(printf '%s' "$info" | jq -r '.claim.since // "unknown"')"
    note "#$n is claimed by ${holder:-unknown} since $since."
    note "If that claim is abandoned: scripts/track.sh release $n --force"
    printf 'busy #%s holder=%s\n' "$n" "${holder:-unknown}"
    return 2
  fi

  if [ "$size" = "l" ] && [ "$force" = 0 ]; then
    lock_release
    die "#$n is size:l — too big for one session. Split it:
  scripts/track.sh add -t '<part>' --parent $n --area <a> --kind <k> --size s
Then claim a sub-issue. Override with --force only if it really is one session."
  fi

  note "claiming #$n as $me …"
  rc=0
  gh_write issue edit "$n" --add-label wip >/dev/null || rc=$?
  [ "$rc" -eq 0 ] || { lock_release; die "could not label #$n — claim abandoned."; }
  # If the marker write fails the label is already on, so take it back off:
  # a wip label with no marker is a claim nobody can identify or release.
  rc=0
  gh_write issue comment "$n" --body "<!-- track:claim agent=$me -->
Claimed by \`$me\` via \`scripts/track.sh claim\`." >/dev/null || rc=$?
  if [ "$rc" -ne 0 ]; then
    gh_write issue edit "$n" --remove-label wip >/dev/null 2>&1 || true
    lock_release
    die "could not record the claim marker on #$n — claim rolled back."
  fi
  lock_release

  printf 'claimed #%s agent=%s\n' "$n" "$me"
  return 0
}

cmd_release() {
  local force=0 n="" me holder ids rc=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --force) force=1; shift ;;
      -*)      die "unknown flag for release: $1" ;;
      *)       n="${1#\#}"; shift ;;
    esac
  done
  [ -n "$n" ] || die "usage: track.sh release <n> [--force]"
  ids="$(held_agent_ids)"

  lock_acquire
  holder="$(fetch_issue "$n" | jq -r "$JQ_CLAIM"' holder.agent // ""')"
  me="$(acting_agent "$holder" "$ids")"
  if [ -z "$me" ]; then
    lock_release
    die "#$n is held by ${holder:-nobody}, and no branch here carries that claim.
Work on a branch, or set TITAN_AGENT."
  fi
  if [ -n "$holder" ] && [ "$holder" != "$me" ] && [ "$force" = 0 ]; then
    lock_release
    die "#$n is held by $holder, not $me. Use --force to take it back."
  fi
  # Not `|| true`: a silently failed removal leaves wip set forever, which
  # drops the issue out of `ready` with nothing anywhere reporting why.
  rc=0
  gh_write issue edit "$n" --remove-label wip --remove-label review >/dev/null || rc=$?
  [ "$rc" -eq 0 ] || { lock_release; die "could not clear wip on #$n — it is still claimed."; }
  gh_write issue comment "$n" --body "<!-- track:release agent=$me -->
Released by \`$me\`." >/dev/null || true
  lock_release
  printf 'released #%s\n' "$n"
  return 0
}

# The state between a draft pull request going up and a human merging it. The
# claim deliberately stays: dropping it would hand finished work back to `ready`
# for a second agent to build again, and readiness is the one thing that must
# not move here. What changes is *why* the issue is held -- the part nothing
# outside the session holding it could see before.
cmd_submit() {
  local force=0 n="" pr="" me holder ids info state branch rc=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --force) force=1; shift ;;
      --pr)    [ $# -ge 2 ] || die "--pr needs a pull request number"; pr="${2#\#}"; shift 2 ;;
      -*)      die "unknown flag for submit: $1" ;;
      *)       n="${1#\#}"; shift ;;
    esac
  done
  [ -n "$n" ] || die "usage: track.sh submit <n> [--pr N] [--force]"
  if [ -n "$pr" ]; then
    case "$pr" in *[!0-9]*) die "--pr takes a pull request number, not '$pr'." ;; esac
  fi
  ids="$(held_agent_ids)"

  # `start` derives the branch and the claim marker from one string, so the
  # branch is the agent id and the open pull request off it is the one whose
  # merge settles this issue. Finding none is not an error: the state is the
  # point, and the number only says where a human has to go.
  if [ -z "$pr" ]; then
    branch="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || true)"
    if [ -n "$branch" ] && [ "$branch" != "HEAD" ]; then
      pr="$(gh pr list --head "$branch" --state open --limit 1 --json number \
            --jq '.[0].number // ""' 2>/dev/null || true)"
    fi
  fi

  lock_acquire
  info="$(fetch_issue "$n" | jq -c "$JQ_LIB$JQ_CLAIM"' shape + {claim: holder}')"
  state="$( printf '%s' "$info" | jq -r '.state')"
  holder="$(printf '%s' "$info" | jq -r '.claim.agent // ""')"

  me="$(acting_agent "$holder" "$ids")"
  if [ -z "$me" ]; then
    lock_release
    die "#$n is held by ${holder:-nobody}, and no branch here carries that claim.
Work on a branch, or set TITAN_AGENT."
  fi
  [ "$state" = "OPEN" ] \
    || { lock_release; die "#$n is $state — a merge has already settled it."; }
  if [ "$holder" != "$me" ] && [ "$force" = 0 ]; then
    lock_release
    die "#$n is held by ${holder:-nobody}, not $me. Claim it first, or --force."
  fi

  rc=0
  gh_write issue edit "$n" --add-label review >/dev/null || rc=$?
  [ "$rc" -eq 0 ] \
    || { lock_release; die "could not label #$n — it still reads as work in progress."; }

  # Rolled back the way `claim` rolls back a missing claim marker: a label with
  # no marker behind it is a state nothing can attribute or explain.
  gh_write issue comment "$n" --body "<!-- track:submit agent=$me${pr:+ pr=$pr} -->
Built by \`$me\`, waiting on a human merge${pr:+ of #$pr}." >/dev/null || rc=$?
  if [ "$rc" -ne 0 ]; then
    gh_write issue edit "$n" --remove-label review >/dev/null 2>&1 || true
    lock_release
    die "could not record the submit marker on #$n — the label has been rolled back."
  fi
  lock_release

  printf 'submitted #%s%s\n' "$n" "${pr:+  waiting on a merge of #$pr}"
  [ -n "$pr" ] || note "note: no open pull request off this branch — recorded without one."
  return 0
}

# What the claim check is for is one agent not closing another's work, so a
# *held* issue is what it protects, and `release` has drawn that line since the
# start. An unheld one is what a merge reaches: `tracking.yml` sets TITAN_AGENT
# to the head ref, which matches the claim on the issue the branch was started
# from and no other, so every later number on a `Tracks` line arrives here held
# by nobody. Refusing those left the run red and the issue open.
cmd_done() {
  local force=0 n="" msg="" me info state subs holder ids was rc=0 freed
  while [ $# -gt 0 ]; do
    case "$1" in
      --force)      force=1; shift ;;
      -m|--message) [ $# -ge 2 ] || die "-m needs a message"; msg="$2"; shift 2 ;;
      -*)           die "unknown flag for done: $1" ;;
      *)            n="${1#\#}"; shift ;;
    esac
  done
  [ -n "$n" ] || die "usage: track.sh done <n> [-m MSG] [--force]"
  ids="$(held_agent_ids)"

  lock_acquire
  info="$(fetch_issue "$n" | jq -c "$JQ_LIB$JQ_CLAIM"' shape + {claim: holder}')"
  state="$( printf '%s' "$info" | jq -r '.state')"
  subs="$(  printf '%s' "$info" | jq -r '.subs_open | map("#\(.)") | join(" ")')"
  holder="$(printf '%s' "$info" | jq -r '.claim.agent // ""')"
  was="$(   printf '%s' "$info" | jq -r '.unblocks  | map("#\(.)") | join(" ")')"

  me="$(acting_agent "$holder" "$ids")"
  if [ -z "$me" ]; then
    lock_release
    die "#$n is held by ${holder:-nobody}, and no branch here carries that claim.
Work on a branch, or set TITAN_AGENT."
  fi

  [ "$state" = "OPEN" ] || { lock_release; die "#$n is already $state."; }
  if [ -n "$subs" ] && [ "$force" = 0 ]; then
    lock_release; die "#$n still has open sub-issues: $subs. Close those first, or --force."
  fi
  if [ -n "$holder" ] && [ "$holder" != "$me" ] && [ "$force" = 0 ]; then
    lock_release
    die "#$n is held by $holder, not $me. Claim it first, or --force."
  fi

  gh_write issue edit "$n" --remove-label wip --remove-label review >/dev/null 2>&1 || true
  gh_write issue comment "$n" --body "<!-- track:done agent=$me -->
${msg:-Completed by \`$me\`.}" >/dev/null || true
  rc=0
  gh_write issue close "$n" --reason completed >/dev/null || rc=$?
  lock_release
  [ "$rc" -eq 0 ] || die "could not close #$n — see the gh error above."

  # Report only what is genuinely actionable now. An issue this one was blocking
  # may still have other open blockers; naming it as unblocked sends the caller
  # into a claim that exits 1, which AGENTS.md tells agents to treat as fatal.
  freed=""
  if [ -n "$was" ]; then
    freed="$(fetch_open | jq -r "$JQ_LIB"'[.[] | shape] | with_gates
              | map(select(is_ready) | .num) | map("#\(.)") | join(" ")')"
    freed="$(printf '%s\n%s\n' "$was" "$freed" | tr ' ' '\n' | sort | uniq -d | tr '\n' ' ')"
    freed="${freed% }"
  fi
  if [ -n "$freed" ]; then
    printf 'done #%s  unblocked: %s\n' "$n" "$freed"
  else
    printf 'done #%s\n' "$n"
  fi
  return 0
}

# The other reading of a closed issue, and the one `require_open_parent` had no
# move for. `done` takes a number and closes it, and an epic's `Done when` is a
# judgement about `main` rather than a count of its sub-issues, so closing one
# before its chain has landed is a mistake a careful agent still makes. Without
# this the only correction is `gh` directly, which AGENTS.md forbids.
#
# No claim is required and none is restored. `done` cleared wip on the way past,
# so there is no holder to match and nothing to put back; what comes back is
# open, unheld work, and `claim` is how it is taken again.
cmd_reopen() {
  local n="" msg="" me info state was ready_now regated rc=0
  while [ $# -gt 0 ]; do
    case "$1" in
      -m|--message) [ $# -ge 2 ] || die "-m needs a message"; msg="$2"; shift 2 ;;
      -*)           die "unknown flag for reopen: $1" ;;
      *)            n="${1#\#}"; shift ;;
    esac
  done
  [ -n "$n" ] || die "usage: track.sh reopen <n> [-m MSG]"
  # A comment body containing a track: marker would be parsed as claim state.
  case "$msg" in *'<!-- track:'*) die "reopen message may not contain a '<!-- track:' marker." ;; esac
  me="$(agent_id_or_empty)"

  lock_acquire
  info="$(fetch_issue "$n" | jq -c "$JQ_LIB"' shape')"
  state="$(printf '%s' "$info" | jq -r '.state')"
  was="$(  printf '%s' "$info" | jq -r '.unblocks | map("#\(.)") | join(" ")')"
  [ "$state" = "CLOSED" ] \
    || { lock_release; die "#$n is $state — there is nothing to reopen."; }

  # Read before the write, where `done` reads what it freed after one: once #$n
  # is open again everything it blocks is already gated, so the mirror of that
  # report would come back empty however much the reopen re-blocked.
  regated=""
  if [ -n "$was" ]; then
    ready_now="$(fetch_open | jq -r "$JQ_LIB"'[.[] | shape] | with_gates
                  | map(select(is_ready) | .num) | map("#\(.)") | join(" ")')"
    regated="$(printf '%s\n%s\n' "$was" "$ready_now" | tr ' ' '\n' | sort | uniq -d | tr '\n' ' ')"
    regated="${regated% }"
  fi

  rc=0
  gh_write issue reopen "$n" >/dev/null || rc=$?
  [ "$rc" -eq 0 ] || { lock_release; die "could not reopen #$n — see the gh error above."; }
  # After the reopen, never before it: a marker on an issue that is still closed
  # tells the next reader the tracker moved when it did not.
  gh_write issue comment "$n" --body "<!-- track:reopen${me:+ agent=$me} -->
${msg:-Reopened${me:+ by \`$me\`} — its work is not in \`main\` after all.}" >/dev/null || true
  lock_release

  # The mirror of `done`'s report, and needed for the same reason: an issue that
  # has stopped being ready is one a caller must not be left pointed at, since
  # the claim it would go and make exits 1.
  if [ -n "$regated" ]; then
    printf 'reopened #%s  re-blocked: %s\n' "$n" "$regated"
  else
    printf 'reopened #%s\n' "$n"
  fi
  return 0
}

# ----------------------------------------------------------------- refs -----
# A pull request body is written by hand and kept largely as the template left
# it, so the two things it reliably contains are template instructions inside
# HTML comments and prose that mentions other issues in passing. Neither may
# settle an issue, so only `Tracks` lines are read, and comments are removed
# first.
strip_html_comments() {
  awk '
    { line = $0
      while (1) {
        if (open) {
          i = index(line, "-->")
          if (i == 0) { line = ""; break }
          line = substr(line, i + 3); open = 0
        } else {
          i = index(line, "<!--")
          if (i == 0) break
          rest = substr(line, i + 4)
          j = index(rest, "-->")
          if (j == 0) { line = substr(line, 1, i - 1); open = 1; break }
          line = substr(line, 1, i - 1) substr(rest, j + 3)
        }
      }
      print line
    }'
}

# Every number on the line, not just the one after the keyword: GitHub binds a
# closing keyword to a single number, so `Closes #98, #99, #100, #101` on #96
# closed #98 and left three issues open with `wip` still set.
cmd_refs() {
  local body="" nums
  while [ $# -gt 0 ]; do
    case "$1" in
      -F|--file) [ $# -ge 2 ] || die "-F needs a file"; body="$(cat "$2")"; shift 2 ;;
      *)         die "usage: track.sh refs [-F FILE]" ;;
    esac
  done
  [ -n "$body" ] || body="$(cat)"

  nums="$(printf '%s\n' "$body" | strip_html_comments \
          | grep -iE '^[[:space:]]*tracks\b' \
          | grep -oE '#[0-9]+' | tr -d '#' | sort -n -u || true)"
  [ -n "$nums" ] && printf '%s\n' "$nums"
  return 0
}

add_flag_list() {   # $1 = flag, $2 = comma list; appends to GH_ARGS
  local flag="$1" list="$2" v
  [ -n "$list" ] || return 0
  local IFS=','
  for v in $list; do
    v="$(printf '%s' "$v" | tr -d '[:space:]')"
    v="${v#\#}"
    [ -n "$v" ] || continue
    case "$v" in *[!0-9]*) die "$flag: '$v' is not an issue number." ;; esac
    GH_ARGS[${#GH_ARGS[@]}]="$flag"
    GH_ARGS[${#GH_ARGS[@]}]="$v"
  done
  return 0
}

cmd_add() {
  local title="" body="" bodyfile="" area="" kind="" size=""
  local bby="" bing="" parent="" selftest=0 url num rc=0 chains ends guide
  while [ $# -gt 0 ]; do
    case "$1" in
      -t|--title)     [ $# -ge 2 ] || die "-t needs a value"; title="$2"; shift 2 ;;
      -b|--body)      [ $# -ge 2 ] || die "-b needs a value"; body="$2"; shift 2 ;;
      -F|--body-file) [ $# -ge 2 ] || die "-F needs a path";  bodyfile="$2"; shift 2 ;;
      --area)         [ $# -ge 2 ] || die "--area needs a value"; area="$2"; shift 2 ;;
      --kind)         [ $# -ge 2 ] || die "--kind needs a value"; kind="$2"; shift 2 ;;
      --size)         [ $# -ge 2 ] || die "--size needs a value"; size="$2"; shift 2 ;;
      --blocked-by)   [ $# -ge 2 ] || die "--blocked-by needs a value"; bby="$2"; shift 2 ;;
      --blocking)     [ $# -ge 2 ] || die "--blocking needs a value"; bing="$2"; shift 2 ;;
      --parent)       [ $# -ge 2 ] || die "--parent needs a value"; parent="${2#\#}"
                      case "$parent" in *[!0-9]*) die "--parent: '$2' is not an issue number." ;; esac
                      shift 2 ;;
      --selftest)     selftest=1; shift ;;
      *)              die "unknown flag for add: $1" ;;
    esac
  done
  [ -n "$title" ] || die "add requires -t/--title"
  [ -n "$area" ] && [ -n "$kind" ] && [ -n "$size" ] \
    || die "add requires --area, --kind and --size."
  valid_label "area:$area" || die "unknown area '$area'. Valid: $(label_values area)"
  valid_label "kind:$kind" || die "unknown kind '$kind'. Valid: $(label_values kind)"
  valid_label "size:$size" || die "unknown size '$size'. Valid: $(label_values size)"

  # Readiness is inherited through the parent, so this is the same shape of
  # failure as the legacy index answering `is:blocked` with a 200: the issue is
  # filed and well formed, and it has quietly jumped the queue. Refuse before
  # the write, so the silent success is unreachable rather than reported.
  if [ "$size" != l ] && [ -z "$parent" ]; then
    if chains="$(epic_chain)"; then
      if [ -n "$chains" ]; then
        guide="The chain, in the order the epics come off each other — ▸ is startable now:
$chains
Re-run with --parent <n> for the one this work belongs under, startable or not.
  scripts/track.sh plan   the same chain, with the work under each epic"
      else
        guide="No epic is open, so the chain has no head to file under yet. An epic is a
root and takes no parent, so file the head first:
  scripts/track.sh add -t '<the chain this belongs to>' --area $area --kind $kind --size l
then re-run this with --parent <that number>."
      fi
    else
      guide="The open chains could not be read just now.
  scripts/track.sh plan   the chain, and which epics are startable
Re-run with --parent <n> for the one this work belongs under."
    fi
    die "add: '$title' is not an epic, so it needs --parent.
Readiness is inherited through the parent: without one this is gated by nothing,
and startable ahead of the whole chain it belongs to.
$guide"
  fi

  # A chain has to start somewhere, which is the whole of why an epic is exempt
  # from --parent. Left there the exemption is one command wide: an epic naming
  # nothing it comes off is gated by nothing, and so is everything filed under
  # it — the same queue jump, reached in two commands instead of one.
  if [ "$size" = l ] && [ -z "$bby" ]; then
    ends="$(epic_chain epic_is_end)" || die "add: the open chains could not be read just now.
  scripts/track.sh plan   the chain this epic has to come off
Re-run with --blocked-by <n> for the epic this one follows."
    guide="$(epic_head_refusal "$title" "$ends")" || die "$guide"
  fi

  [ -z "$parent" ] || require_open_parent "$parent" "add --parent"

  if [ "$selftest" = 1 ]; then
    [ -n "${ST_RUN:-}" ] || die "add --selftest needs a run marker.
Without one the issue carries no sign of which run filed it, and cleanup cannot
tell it from a concurrent run's."
    title="$title [$ST_RUN]"
  fi

  GH_ARGS=(issue create --title "$title"
           --label "area:$area" --label "kind:$kind" --label "size:$size")
  [ "$selftest" = 1 ] && GH_ARGS[${#GH_ARGS[@]}]="--label" && GH_ARGS[${#GH_ARGS[@]}]="track:selftest"
  if [ -n "$bodyfile" ]; then
    GH_ARGS[${#GH_ARGS[@]}]="--body-file"; GH_ARGS[${#GH_ARGS[@]}]="$bodyfile"
  else
    GH_ARGS[${#GH_ARGS[@]}]="--body"; GH_ARGS[${#GH_ARGS[@]}]="${body:-_No description._}"
  fi
  add_flag_list --blocked-by "$bby"
  add_flag_list --blocking   "$bing"
  if [ -n "$parent" ]; then
    GH_ARGS[${#GH_ARGS[@]}]="--parent"; GH_ARGS[${#GH_ARGS[@]}]="$parent"
  fi

  note "creating issue …"
  # Acquire in THIS shell, not inside the substitution below: `$( )` runs in a
  # subshell, so a lock_acquire in there sets LOCK_HELD only for the subshell and
  # our lock_release would leave the lock directory behind.
  lock_acquire
  rc=0
  url="$(gh_write "${GH_ARGS[@]}")" || rc=$?
  lock_release
  [ "$rc" -eq 0 ] || die "add failed — see the gh error above."
  num="${url##*/}"
  case "$num" in ''|*[!0-9]*) die "add: could not parse an issue number from '$url'";; esac
  if [ "$AS_JSON" = 1 ]; then
    jq -nc --arg n "$num" --arg u "$url" '{num: ($n | tonumber), url: $u}'
  else
    printf 'created #%s %s\n' "$num" "$url"
  fi
  return 0
}

cmd_dep() {
  [ $# -ge 1 ] || die "usage: track.sh dep <n> [--needs N] [--drop-needs N] [--parent N] [--child N] [--drop-child N]"
  local n="${1#\#}" desc="" dropped="" child="" new_parent="" c p rc=0
  shift
  GH_ARGS=(issue edit "$n")
  while [ $# -gt 0 ]; do
    case "$1" in
      --needs)      [ $# -ge 2 ] || die "--needs needs a value"
                    GH_ARGS[${#GH_ARGS[@]}]="--add-blocked-by";    GH_ARGS[${#GH_ARGS[@]}]="${2#\#}"
                    desc="$desc needs #${2#\#}"; shift 2 ;;
      --drop-needs) [ $# -ge 2 ] || die "--drop-needs needs a value"
                    GH_ARGS[${#GH_ARGS[@]}]="--remove-blocked-by"; GH_ARGS[${#GH_ARGS[@]}]="${2#\#}"
                    desc="$desc drop-needs #${2#\#}"; shift 2 ;;
      --parent)     [ $# -ge 2 ] || die "--parent needs a value"
                    GH_ARGS[${#GH_ARGS[@]}]="--parent";            GH_ARGS[${#GH_ARGS[@]}]="${2#\#}"
                    new_parent="$new_parent ${2#\#}"
                    desc="$desc parent #${2#\#}"; shift 2 ;;
      --child)      [ $# -ge 2 ] || die "--child needs a value"
                    GH_ARGS[${#GH_ARGS[@]}]="--add-sub-issue";     GH_ARGS[${#GH_ARGS[@]}]="${2#\#}"
                    child="$child ${2#\#}"
                    desc="$desc child #${2#\#}"; shift 2 ;;
      --drop-child) [ $# -ge 2 ] || die "--drop-child needs a value"
                    GH_ARGS[${#GH_ARGS[@]}]="--remove-sub-issue";  GH_ARGS[${#GH_ARGS[@]}]="${2#\#}"
                    dropped="$dropped ${2#\#}"
                    desc="$desc drop-child #${2#\#}"; shift 2 ;;
      *)            die "unknown flag for dep: $1" ;;
    esac
  done
  [ "${#GH_ARGS[@]}" -gt 3 ] || die "dep needs at least one of --needs/--drop-needs/--parent/--child/--drop-child"

  # The number checked is the parent a write would install: `n` for --child, and
  # the flag's own value for --parent -- the one-write move `--drop-child`'s
  # refusal recommends, and so the third place a parent reaches the tracker. A
  # blocker that is closed is the normal state of finished work, so --needs is
  # left alone.
  for p in $new_parent; do
    require_open_parent "$p" "dep --parent"
  done
  [ -z "$child" ] || require_open_parent "$n" "dep --child"

  # Checked before the write, so the orphan is unreachable rather than reported
  # after the fact -- the same reason `add`'s refusal lands before its create.
  for c in $dropped; do
    require_epic_to_drop "$c"
  done

  # Explicit status check: when this function is called as `cmd_dep … || rc=$?`,
  # bash disables `set -e` for the whole body, so a failed write would otherwise
  # fall through to the success message below. GitHub rejects a direct 2-cycle
  # here, and that rejection must reach the caller.
  rc=0
  gh_write "${GH_ARGS[@]}" >/dev/null || rc=$?
  lock_release
  [ "$rc" -eq 0 ] || die "dep failed on #$n — see the gh error above."
  printf 'dep #%s%s\n' "$n" "$desc"
  return 0
}

cmd_note() {
  [ $# -ge 1 ] || die "usage: track.sh note <n> -m MSG"
  local n="${1#\#}" msg="" rc=0
  shift
  while [ $# -gt 0 ]; do
    case "$1" in
      -m|--message) [ $# -ge 2 ] || die "-m needs a message"; msg="$2"; shift 2 ;;
      *)            die "unknown flag for note: $1" ;;
    esac
  done
  [ -n "$msg" ] || die "note requires -m MSG"
  # A comment body containing a track: marker would be parsed as claim state.
  case "$msg" in *'<!-- track:'*) die "note body may not contain a '<!-- track:' marker." ;; esac
  rc=0
  gh_write issue comment "$n" --body "$msg" >/dev/null || rc=$?
  lock_release
  [ "$rc" -eq 0 ] || die "note failed on #$n — see the gh error above."
  printf 'noted #%s\n' "$n"
  return 0
}

cmd_graph() {
  local shaped cyc
  shaped="$(fetch_open | jq "$JQ_LIB"' [.[] | shape] | with_gates')"
  if [ "$AS_JSON" = 1 ]; then printf '%s\n' "$shaped"; return 0; fi

  printf 'graph (%s open)\n' "$(printf '%s' "$shaped" | jq 'length')"
  cyc="$(printf '%s' "$shaped" | jq -r "$JQ_LIB"'cycle_nodes | map("#\(.)") | join(" ")')"
  printf '%s' "$shaped" | jq -r --argjson m 60 '
    ([.[] | {key: (.num | tostring), value: .}] | from_entries) as $idx
    | def pad($d): if $d == 0 then "" else ("  " * $d) end;
      def walk($n; $d):
        ($idx[$n | tostring]) as $i
        | if $i == null then empty
          elif $d > 5 then "\(pad($d))#\($n) …"
          else "\(pad($d))#\($n) \(if $i.area == "" then "-" else $i.area end) \(
                 if ($i.title | length) > $m then ($i.title[0:$m] + "…") else $i.title end
               )\(if $i.wip then "  [wip]" else "" end)",
               ($i.unblocks[]? | walk(.; $d + 1))
          end;
      [.[] | select(.blockers | length == 0) | .num] as $roots
      | ($roots[] | walk(.; 0))'
  [ -n "$cyc" ] && printf 'CYCLE: %s — unreachable from any root, they block each other.\n' "$cyc"
  return 0
}

cmd_labels_init() {
  local name color desc total
  total="$(label_names | wc -l | tr -d ' ')"
  note "creating/updating $total labels (paced) …"
  while IFS='|' read -r name color desc; do
    [ -n "$name" ] || continue
    gh_write label create "$name" --color "$color" --description "$desc" --force >/dev/null
    note "  $name"
  done <<< "$LABEL_SPEC"
  lock_release
  printf 'labels-init ok (%s labels)\n' "$total"
  return 0
}

# ----------------------------------------------------------------- doctor ---
DOC_FAIL=0
DOC_JSON='[]'
chk() {   # $1 = ok|FAIL|warn, $2 = name, $3 = detail
  DOC_JSON="$(printf '%s' "$DOC_JSON" | jq -c --arg s "$1" --arg n "$2" --arg d "$3" \
              '. + [{check: $n, status: $s, detail: $d}]')"
  [ "$1" = "FAIL" ] && DOC_FAIL=$((DOC_FAIL + 1))
  [ "$AS_JSON" = 1 ] || printf '  %-5s %-22s %s\n' "$1" "$2" "$3"
  return 0
}

ver_ge() {   # ver_ge 2.97.0 2.94.0  — `sort -V` is not reliable on BSD
  local have="$1" want="$2" h1 h2 h3 w1 w2 w3
  h1="${have%%.*}"; have="${have#*.}"; h2="${have%%.*}"; h3="${have#*.}"; h3="${h3%%[!0-9]*}"
  w1="${want%%.*}"; want="${want#*.}"; w2="${want%%.*}"; w3="${want#*.}"; w3="${w3%%[!0-9]*}"
  [ "${h1:-0}" -gt "${w1:-0}" ] && return 0
  [ "${h1:-0}" -lt "${w1:-0}" ] && return 1
  [ "${h2:-0}" -gt "${w2:-0}" ] && return 0
  [ "${h2:-0}" -lt "${w2:-0}" ] && return 1
  [ "${h3:-0}" -ge "${w3:-0}" ]
}

# Whether the repository's own graph holds a cycle, over `gh issue list` JSON on
# stdin. `graph` and `ready` report every cycle they can see, throwaway ones
# included, because a run wants its own fixtures diagnosed. This is the narrower
# question a gate asks, where another run's open fixture is not an answer.
own_cycle() {
  jq "$JQ_LIB"'repo_own | [.[] | shape] | has_cycle' 2>/dev/null || printf 'false\n'
}

cmd_doctor() {
  local ghv st who scopes repo nwo issues have missing L me cyc total
  local stale waiting c num who2 since at age
  [ "$AS_JSON" = 1 ] || printf 'doctor\n'

  if command -v jq >/dev/null 2>&1; then chk ok jq "$(jq --version)"
  else chk FAIL jq "not installed — brew install jq"; fi

  if command -v gh >/dev/null 2>&1; then
    ghv="$(gh --version | awk 'NR==1{print $3}')"
    if ver_ge "$ghv" "2.94.0"; then chk ok gh "$ghv (>= 2.94)"
    else chk FAIL gh "$ghv — need >= 2.94 for --blocked-by/--parent. brew upgrade gh"; fi
  else chk FAIL gh "not installed"; fi

  st="$(gh auth status 2>&1 || true)"
  if printf '%s' "$st" | grep -q 'Logged in'; then
    who="$(gh api user --jq .login 2>/dev/null || echo '?')"
    scopes="$(printf '%s' "$st" | grep -o "Token scopes:.*" | awk 'NR==1')"
    if printf '%s' "$scopes" | grep -q "'repo'"; then chk ok auth "$who; $scopes"
    else chk FAIL auth "$who has no 'repo' scope — gh auth refresh -s repo"; fi
  else chk FAIL auth "not logged in — gh auth login"; fi

  repo="$(gh repo view --json nameWithOwner,hasIssuesEnabled 2>/dev/null || true)"
  if [ -n "$repo" ]; then
    nwo="$(   printf '%s' "$repo" | jq -r .nameWithOwner)"
    issues="$(printf '%s' "$repo" | jq -r .hasIssuesEnabled)"
    if [ "$issues" = "true" ]; then chk ok repo "$nwo (issues enabled)"
    else chk FAIL repo "$nwo has Issues disabled — enable in repo settings"; fi
  else chk FAIL repo "cannot resolve repo from git remote"; fi

  if gh issue list --limit 1 --json number,blockedBy,subIssues >/dev/null 2>&1; then
    chk ok dep-api "blockedBy/subIssues readable"
  else chk FAIL dep-api "cannot read dependency fields"; fi

  total="$(label_names | wc -l | tr -d ' ')"
  have="$(gh label list --limit 200 --json name --jq '.[].name' 2>/dev/null || true)"
  missing=""
  while IFS= read -r L; do
    [ -n "$L" ] || continue
    printf '%s\n' "$have" | grep -qx -- "$L" || missing="$missing $L"
  done <<< "$(label_names)"
  if [ -z "$missing" ]; then chk ok labels "all $total present"
  else chk FAIL labels "missing:$missing — run: scripts/track.sh labels-init"; fi

  me="$(agent_id_or_empty)"
  if [ -n "$me" ]; then chk ok agent "$me"
  else chk warn agent "on main/detached — claim will refuse. Branch, or set TITAN_AGENT."; fi

  if repo_key >/dev/null 2>&1; then
    state_init
    if mkdir -p "$STATE_DIR" 2>/dev/null && [ -w "$STATE_DIR" ]; then
      chk ok lockdir "$STATE_DIR"
    else chk FAIL lockdir "$STATE_DIR not writable"; fi
  else chk FAIL lockdir "not inside a git repository"; fi

  cyc="$(fetch_open | own_cycle)"
  if [ "$cyc" = "true" ]; then chk FAIL graph "dependency cycle — run: scripts/track.sh graph"
  else chk ok graph "no dependency cycle"; fi

  # Branches here live hours, so a claim older than a day is a strong signal.
  # It is a warning and never a failure: a slow task and a dead one look
  # identical from here, and only a human can tell them apart.
  stale=""
  waiting=""
  while IFS= read -r c; do
    [ -n "$c" ] || continue
    num="$(  printf '%s' "$c" | jq -r '.num')"
    who2="$( printf '%s' "$c" | jq -r '.claim.agent // ""')"
    since="$(printf '%s' "$c" | jq -r '.claim.since // ""')"
    if [ "$(printf '%s' "$c" | jq -r '.review')" = "true" ]; then
      waiting="$waiting #$num"
      continue
    fi
    [ -n "$since" ] || continue
    at="$(iso_epoch "$since")"
    [ "$at" -gt 0 ] || continue
    age=$(( ( $(date -u +%s) - at ) / 3600 ))
    [ "$age" -ge "$STALE_HOURS" ] && stale="$stale #$num($who2, ${age}h)"
  done <<< "$(claimed_issues 2>/dev/null || true)"
  if [ -n "$stale" ]; then
    chk warn claims "stale:$stale — release with: scripts/track.sh release <n> --force"
  else chk ok claims "no claim older than ${STALE_HOURS}h"; fi

  if [ -n "$waiting" ]; then
    chk ok merges "built, waiting on a human merge:$waiting"
  else chk ok merges "nothing waiting on a merge"; fi

  if [ "$AS_JSON" = 1 ]; then
    printf '%s' "$DOC_JSON" | jq -c --argjson f "$DOC_FAIL" '{checks: ., failed: $f}'
  elif [ "$DOC_FAIL" -eq 0 ]; then printf 'doctor ok\n'
  else printf '%s check(s) failed\n' "$DOC_FAIL"; fi
  [ "$DOC_FAIL" -eq 0 ] || exit 1
  return 0
}

# --------------------------------------------------------------- selftest ---
ST_PASS=0
ST_FAIL=0
ST_SCRATCH=""
ST_ORPHAN_BRANCH=""
ST_NUM=""
ST_RUN=""
ST_FOREIGN_RUN=""
st_ok()   { ST_PASS=$((ST_PASS + 1)); note "  ok    $*"; return 0; }
st_bad()  { ST_FAIL=$((ST_FAIL + 1)); note "  FAIL  $*"; return 0; }
st_assert() { if [ "$1" = 0 ]; then st_ok "$2"; else st_bad "$2"; fi; }

# The throwaway issues of one run, or of every run. The marker lives on the
# issue rather than in a list held here: a run that dies between the write and
# the assignment would leave one that nothing could attribute, and a snapshot of
# the label taken before a run cannot tell two runs apart at all.
#
# There is deliberately no empty-means-everything. An unset marker reaching here
# would delete a live run's fixtures, which is the fault this scoping removes.
st_delete_run() {   # st_delete_run <marker>|--all
  local marker="${1:-}" scope rows nums n deleted=0 held="$LOCK_HELD"
  if [ -z "$marker" ]; then
    note "  cleanup: no run marker — nothing removed"
    printf '0\n'
    return 0
  fi
  scope="run $marker"
  [ "$marker" = "--all" ] && scope="every run"
  rows="$(gh issue list --state all --label track:selftest --limit "$LIST_LIMIT" \
          --json number,title 2>/dev/null \
          | jq -r '.[] | "\(.number)\t\(.title | gsub("[\t\r\n]"; " "))"' || true)"
  if [ "$marker" = "--all" ]; then
    nums="$(printf '%s\n' "$rows" | awk -F'\t' 'NF{print $1}')"
  else
    nums="$(printf '%s\n' "$rows" | grep -F "$marker" | awk -F'\t' 'NF{print $1}' || true)"
  fi
  # Paired here rather than left to the caller: gh_write acquires and does not
  # release, so this running inside a $( ) would set LOCK_HELD in the subshell
  # only and leave the lock directory behind — recorded as held by a pid that is
  # the parent's, which then waits on itself for ever.
  lock_acquire
  for n in $nums; do
    if gh_write issue delete "$n" --yes >/dev/null 2>&1; then
      deleted=$((deleted + 1))
    else
      gh_write issue close "$n" --reason "not planned" >/dev/null 2>&1 || true
      deleted=$((deleted + 1))
      note "  (could not delete #$n — closed instead; needs admin to delete)"
    fi
  done
  [ "$held" = 1 ] || lock_release
  note "  cleanup: removed $deleted throwaway issue(s) from $scope"
  printf '%s\n' "$deleted"
  return 0
}

st_cleanup() {
  note "  cleaning up …"
  # Both outlive a failed assertion, and an abandoned orphan branch is one this
  # very change would then read as work this checkout holds.
  if [ -n "$ST_SCRATCH" ]; then rm -rf "$ST_SCRATCH"; ST_SCRATCH=""; fi
  if [ -n "$ST_ORPHAN_BRANCH" ]; then
    git branch -D "$ST_ORPHAN_BRANCH" >/dev/null 2>&1 || true
    ST_ORPHAN_BRANCH=""
  fi
  st_delete_run "$ST_RUN" >/dev/null
  lock_release
  return 0
}

# A marker unique to one run, on the same shasum idiom as repo_key.
st_run_id() {
  printf 'st%s' \
    "$(printf '%s %s %s' "$(date -u +%s)" "$$" "$RANDOM$RANDOM$RANDOM" | shasum | cut -c1-8)"
}

st_num() { printf '%s' "$1" | awk '{print $2}' | tr -d '#'; }

# Files one throwaway issue and leaves its number in ST_NUM.
#
# `add` is the step the rest of the run is built on, and the one whose refusal
# used to vanish rather than end the run: `die` exits the substitution, the
# assignment around it takes st_num's status instead, and every issue below is
# filed against an empty number. Handing the number back through a global rather
# than stdout is what keeps the assertion out of a subshell, where the counters
# would not survive it.
st_add() {
  local label="$1" out rc=0
  shift
  out="$(AS_JSON=0 cmd_add "$@" --selftest)" || rc=$?
  ST_NUM="$(st_num "$out")"
  [ -n "$ST_NUM" ] || rc=1
  st_assert "$rc" "$label"
  return 0
}

# Whether lock_acquire takes a lock this run left behind rather than waiting on
# it, against a scratch state directory so a parallel run's lock is never touched.
#
# Bounded, because the failure it asserts against is an endless wait: called
# straight, a regression would hang the run this is meant to record one FAIL in.
st_lock_recovers() {
  local held_state="$STATE_DIR" held_lock="$LOCK" held_stamp="$STAMP"
  local scratch watched waited=0 rc=0
  scratch="$(mktemp -d)"
  STATE_DIR="$scratch"; LOCK="$scratch/lock"; STAMP="$scratch/last-write"
  mkdir -p "$LOCK"
  printf '%s' "$$" > "$LOCK/pid"
  ( LOCK_HELD=0; lock_acquire ) >/dev/null 2>&1 &
  watched=$!
  while kill -0 "$watched" 2>/dev/null && [ "$waited" -lt 50 ]; do
    sleep 0.2
    waited=$((waited + 1))
  done
  if kill -0 "$watched" 2>/dev/null; then
    kill -9 "$watched" 2>/dev/null || true
    rc=1
  fi
  wait "$watched" 2>/dev/null || true
  STATE_DIR="$held_state"; LOCK="$held_lock"; STAMP="$held_stamp"
  rm -rf "$scratch"
  return "$rc"
}

# An open set shaped as `gh issue list` returns it, built from compact rows so a
# graph states only what it is about. A row is {n}, plus any of: size, wip, blk
# (numbers that block it), sub (its open children), par (its parent), closed,
# lbl (extra labels), trunc (more relations than gh returned).
#
# `blocking` is inverted from `blk` over the set rather than given, so a fixture
# cannot state an edge in one direction only -- `unblocks` reads that side, and
# a hand-written half-edge would sort `ready` by a number no real payload can
# hold. Everything else the derivations touch is spelled out, because a default
# that happens to suit today's assertion is one the next reader has to go and
# discover.
st_fixture() {   # st_fixture <rows-json>
  jq -cn --argjson rows "$1" '
    def relation($ns; $extra):
      {totalCount: (($ns | length) + $extra),
       nodes: [$ns[] | {number: ., state: "OPEN"}]};
    (INDEX($rows[]; .n | tostring)) as $by
    | $rows
    | map((.n) as $me
        | ([$rows[] | select((.blk // []) | index($me)) | .n]) as $blocking
        | (if .trunc then 1 else 0 end) as $extra
        | { number: $me,
            title: "fixture \($me)",
            state: (if .closed then "CLOSED" else "OPEN" end),
            url: "",
            labels: ([{name: "area:infra"}, {name: "kind:chore"}]
                     + (if .size then [{name: "size:\(.size)"}] else [] end)
                     + (if .wip then [{name: "wip"}] else [] end)
                     + ((.lbl // []) | map({name: .}))),
            blockedBy:  relation((.blk // []); $extra),
            blocking:   relation($blocking; 0),
            subIssues:  relation((.sub // []); 0),
            subIssuesSummary: {total: ((.sub // []) | length), completed: 0},
            parent: (if .par == null then null
                     else {number: .par,
                           title: "fixture \(.par)",
                           state: (if ($by[.par | tostring].closed // false)
                                   then "CLOSED" else "OPEN" end)} end) })'
}

# Three issues in a 3-cycle, carrying the labels given as JSON. A cycle among
# the repository's own issues has to stay a doctor failure, and filing one to
# prove it would leave a real fault in a real graph for every other run to read.
st_cycle_fixture() {   # st_cycle_fixture <labels-json>
  local lbl="$1"
  st_fixture "$(jq -cn --argjson l "$lbl" '
    [ {n: 9000, lbl: []},
      {n: 9001, blk: [9003], lbl: $l},
      {n: 9002, blk: [9001], lbl: $l},
      {n: 9003, blk: [9002], lbl: $l} ]
    | map(.lbl = (.lbl | map(.name)))')"
}

# Files one throwaway issue under the marker of a second, imaginary run. What
# cleanup has to leave alone is another run's work, and the only way to have any
# without starting a second run is to file it under a second marker.
st_add_foreign() {
  local keep="$ST_RUN"
  ST_RUN="$ST_FOREIGN_RUN"
  st_add "$@"
  ST_RUN="$keep"
  return 0
}

# What the throwaway chain comes off. An epic names the epic it follows, and for
# the selftest that one has to be CLOSED: an open blocker would gate every issue
# filed under the root, and half the run would fail on a queue it never meant to
# test. A run closes throwaway issues as it goes, so the newest closed issue in
# the repository is often one a concurrent run is about to delete — and a chain
# hung off that is the same death by a second route.
st_chain_head() {
  gh issue list --state closed --limit "$LIST_LIMIT" --json number,labels \
    | jq -r '[.[] | select((.labels | map(.name) | index("track:selftest")) == null)]
             | .[0].number // empty'
}

# main checked out, one branch held by a second worktree, one held by nothing.
st_scratch_repo() {
  local d="$1"
  git init -q -b main "$d"
  git -C "$d" -c user.email=selftest@titan -c user.name=selftest \
      commit -q --allow-empty -m "root"
  git -C "$d" branch held/orphan
  git -C "$d" branch held/foreign
  git -C "$d" worktree add -q "$d/.wt-foreign" held/foreign
  git -C "$d" switch -q -c held/current
  return 0
}

# Every command below runs inside a subshell — `( … )` for a step, `$( … )` for
# a capture — and every status is caught with `|| rc=$?` and handed to
# st_assert. A refusal is then one FAIL with the rest of the run still going,
# rather than a `die` that `||` cannot catch or a status `set -e` acts on
# first, either of which loses every assertion after it. scripts/check-style.sh
# holds the whole function to it, so the file decides this and not the caller.
# What makes the subshell safe is that a command takes and drops the write lock
# inside its own body, refusal included, so neither outlives the containment.
cmd_selftest() {
  local clean="--all"
  case "${1:-}" in
    --clean)
      shift
      case "${1:-}" in --yes|'') ;; *) clean="$1"; shift ;; esac
      [ "${1:-}" = "--yes" ] || die "selftest --clean deletes throwaway issues for real — every run's by
default, including a run in progress somewhere else.
Re-run with:  scripts/track.sh selftest --clean --yes
Or name one run's marker:  scripts/track.sh selftest --clean <marker> --yes"
      printf 'cleaned %s throwaway issue(s)\n' "$(st_delete_run "$clean")"
      return 0
      ;;
    --yes) ;;
    *) die "selftest creates and deletes real issues in this repo.
Re-run with:  scripts/track.sh selftest --yes" ;;
  esac

  local t0 A B C D E F G H J K L M N Q R S T U V X1 X2 Y Z
  local out rc rc2 loc adv dt bn ob scratch ids head now
  t0="$(date +%s)"
  ST_RUN="$(st_run_id)"
  # Drawn fresh rather than derived from $ST_RUN: anything sharing a prefix with
  # it would be matched by the grep that scopes a delete, silently re-merging
  # the two sets. A constant would be worse still — every run would file its
  # stand-in under it and delete every other run's, which is this bug.
  ST_FOREIGN_RUN="$(st_run_id)"
  note "selftest $ST_RUN"
  note "  preflight: doctor"
  ( AS_JSON=0 cmd_doctor >/dev/null ) || die "doctor failed — fix that first."
  note "  preflight: budget"
  budget_preflight

  trap 'st_cleanup; st_delete_run "$ST_FOREIGN_RUN" >/dev/null; lock_release' EXIT
  # Without an explicit exit, bash runs the handler and then RESUMES, so a Ctrl-C
  # would delete the throwaway issues and carry on asserting against them.
  trap 'st_cleanup; st_delete_run "$ST_FOREIGN_RUN" >/dev/null; lock_release; exit 130' INT TERM

  # The write lock, before anything that takes it. These need no issues and no
  # network: what they assert is the reading of a pid the lock has always
  # recorded, and a run that waits on that pid waits on itself.
  note "  lock recovery …"
  rc=0; lock_is_ours_to_break "$$" || rc=$?
  st_assert "$rc" "a lock recorded against this run is one to break, not wait on"
  rc=0; if lock_is_ours_to_break 1; then rc=1; fi
  st_assert "$rc" "a lock recorded against another live process is waited on"
  rc=0; if lock_is_ours_to_break ""; then rc=1; fi
  st_assert "$rc" "a lock with no readable holder is left to the delayed path"
  rc=0; st_lock_recovers || rc=$?
  st_assert "$rc" "lock_acquire takes a lock this run left behind"

  # The budget preflight, over a rate-limit payload rather than the live one: a
  # real budget low enough to assert a refusal against is one no run could then
  # be made from, and the point of the refusal is that it never gets there.
  note "  budget preflight …"
  now="$(date -u +%s)"
  rc=0; out="$( ST_RATE="10 $((now + 1800))"; budget_preflight 2>&1 )" || rc=$?
  st_assert "$([ "$rc" = 1 ] && echo 0 || echo 1)" \
    "selftest refuses a budget a run's cost would outrun (got $rc)"
  rc=0; printf '%s' "$out" | grep -q "10 point" || rc=1
  st_assert "$rc" "the refusal says how little is left"
  rc=0; printf '%s' "$out" | grep -q "in 30m" || rc=1
  st_assert "$rc" "the refusal names when the window resets"
  rc=0; ( ST_RATE="$SELFTEST_COST $((now + 1800))"; budget_preflight >/dev/null 2>&1 ) || rc=$?
  st_assert "$rc" "a budget of exactly a run's cost is one to start on"
  rc=0; ( ST_RATE="5000 $((now + 1800))"; budget_preflight >/dev/null 2>&1 ) || rc=$?
  st_assert "$rc" "a full budget starts a run"
  rc=0; out="$( ST_RATE="not a payload"; budget_preflight 2>&1 )" || rc=$?
  st_assert "$([ "$rc" = 1 ] && echo 0 || echo 1)" \
    "a rate limit it cannot read refuses rather than compares (got $rc)"

  rc=0; out="$( until_reset "$((now + 1800))" "$now" )" || rc=$?
  rc2=0; printf '%s' "$out" | grep -q "(in 30m)" || rc2=1
  st_assert "$rc2" "a window still open reads as the minutes left on it"
  rc=0; out="$( until_reset "$((now - 60))" "$now" )" || rc=$?
  rc2=0; [ "$out" = "now" ] || rc2=1
  st_assert "$rc2" "a window already turned over reads as now (got '$out')"

  # ------------------------------------------------------------ derivation ---
  # Everything below is a fact about the shape `gh issue list` returns, so it is
  # stated as a shape and nothing is filed. A fixture also holds graphs the live
  # one cannot be asked for: relations gh truncated, and a cycle, which filed for
  # real is a fault left in a real graph for every other run to read.
  note "  deriving over fixtures …"

  ST_FIXTURE="$(st_fixture '[
    {"n": 9101, "size": "s", "sub": [9102]},
    {"n": 9102, "size": "s", "par": 9101},
    {"n": 9103, "size": "s", "blk": [9101]},
    {"n": 9104, "size": "l"},
    {"n": 9105, "size": "s", "blk": [9101], "trunc": true},
    {"n": 9106, "size": "s", "wip": true}
  ]')"

  rc=0; out="$(AS_JSON=1 cmd_ready)" || rc=$?
  st_assert "$rc" "ready derives a queue from a fixture, with nothing filed"
  rc=0; printf '%s' "$out" | jq -e 'any(.num == 9102)' >/dev/null || rc=1
  st_assert "$rc" "ready includes the leaf under an unblocked parent"
  rc=0; printf '%s' "$out" | jq -e 'all(.num != 9101)' >/dev/null || rc=1
  st_assert "$rc" "ready excludes a container with an open child"
  rc=0; printf '%s' "$out" | jq -e 'all(.num != 9103)' >/dev/null || rc=1
  st_assert "$rc" "ready excludes a blocked issue"
  rc=0; printf '%s' "$out" | jq -e 'all(.num != 9104)' >/dev/null || rc=1
  st_assert "$rc" "ready excludes a size:l issue claim would refuse"
  rc=0; printf '%s' "$out" | jq -e 'all(.num != 9106)' >/dev/null || rc=1
  st_assert "$rc" "ready excludes an issue another agent holds"

  rc=0; out="$(AS_JSON=0 cmd_ready)" || rc=$?
  rc2=0; printf '%s' "$out" | grep -q "SPLIT:.*#9104" || rc2=1
  st_assert "$rc2" "ready names the size:l issue under SPLIT"
  # Truncation has no live assertion at all: gh returns every relation of an
  # issue this size, so the case can only be stated as a shape.
  rc2=0; printf '%s' "$out" | grep -q "TRUNCATED:.*#9105" || rc2=1
  st_assert "$rc2" "ready flags an issue whose relations gh truncated"

  rc=0; out="$(AS_JSON=1 cmd_blocked)" || rc=$?
  rc2=0; printf '%s' "$out" \
    | jq -e 'any(.num == 9103 and (.blockers | index(9101) != null))' >/dev/null || rc2=1
  st_assert "$rc2" "blocked lists #9103 <- #9101"

  # Inherited readiness, at a depth the live graph is never built to: the leaf
  # is clear, its parent is clear, and the blocker is two levels up.
  ST_FIXTURE="$(st_fixture '[
    {"n": 9201, "size": "s"},
    {"n": 9202, "size": "l", "blk": [9201]},
    {"n": 9203, "size": "s", "par": 9202},
    {"n": 9204, "size": "s", "par": 9203}
  ]')"

  rc=0; out="$(AS_JSON=1 cmd_ready)" || rc=$?
  rc2=0; printf '%s' "$out" | jq -e 'all(.num != 9204)' >/dev/null || rc2=1
  st_assert "$rc2" "ready excludes a leaf gated two levels up"
  rc=0; out="$(AS_JSON=1 cmd_blocked)" || rc=$?
  rc2=0; printf '%s' "$out" | jq -e 'any(.num == 9204 and .gated_by == 9202)' >/dev/null || rc2=1
  st_assert "$rc2" "blocked names the ancestor carrying the blocker, not the blocker"
  rc2=0; printf '%s' "$out" | jq -e 'any(.num == 9203 and .gated_by == 9202)' >/dev/null || rc2=1
  st_assert "$rc2" "blocked gates the intermediate parent through the same ancestor"

  # A cycle has to be found while unrelated work is still ready — exactly the
  # case a naive "no source anywhere" check misses.
  ST_FIXTURE="$(st_fixture '[
    {"n": 9301, "size": "s"},
    {"n": 9302, "size": "s", "blk": [9304]},
    {"n": 9303, "size": "s", "blk": [9302]},
    {"n": 9304, "size": "s", "blk": [9303]}
  ]')"

  # NOTE: the library and the expression must be ONE argument. Passing them as
  # two makes jq treat the second as an input filename.
  rc=0; out="$(AS_JSON=1 cmd_graph)" || rc=$?
  rc2=0; printf '%s' "$out" | jq -e "$JQ_LIB"'cycle_nodes == [9302, 9303, 9304]' \
    >/dev/null 2>&1 || rc2=1
  st_assert "$rc2" "cycle_nodes finds the 3-cycle and nothing else"

  # Capture first: piping into `grep -q` closes the pipe early and SIGPIPEs the
  # producer under `set -o pipefail`.
  rc=0; out="$(AS_JSON=0 cmd_ready)" || rc=$?
  rc2=0; printf '%s' "$out" | grep -q "CYCLE:.*#9302" || rc2=1
  st_assert "$rc2" "ready reports the cycle on stdout"
  rc2=0; printf '%s' "$out" | grep -q "^  #9301 " || rc2=1
  st_assert "$rc2" "unrelated work is still ready despite the cycle"

  ST_FIXTURE=""

  note "  creating throwaway issues …"
  head="$(st_chain_head)"
  [ -n "$head" ] || die "selftest: no closed issue for the throwaway chain to come off."
  st_add "add files a size:l epic off closed #$head, with no parent" \
    -t "selftest root epic" --area infra --kind chore --size l --blocked-by "$head"
  Z="$ST_NUM"
  # Nearly everything below hangs off $Z, directly or through something that
  # does. Empty, those would all be refused for the want of a parent, and one
  # real failure would read as most of the run.
  [ -n "$Z" ] || die "selftest: the root epic was not created — nothing below can run."

  # Readiness is inherited, so an issue filed with no parent is startable ahead
  # of the whole chain it belongs to — filed, well formed, and at the front of
  # `ready`. The refusal has to land before the write, or the silent success it
  # exists to prevent has already happened by the time it is reported.
  rc=0; out="$(AS_JSON=0 cmd_add -t "selftest parentless" --area infra --kind chore --size s --selftest 2>&1 >/dev/null)" || rc=$?
  st_assert "$([ "$rc" = 1 ] && echo 0 || echo 1)" "add refuses a non-epic with no parent (got $rc)"
  rc=0; printf '%s' "$out" | grep -q " #$Z  " || rc=1
  st_assert "$rc" "add's refusal names open epic #$Z as a chain to file under"
  rc=0; printf '%s' "$out" | grep -q "track.sh plan" || rc=1
  st_assert "$rc" "add's refusal says how to look the chain up"
  rc=0; AS_JSON=1 cmd_find "selftest parentless" | jq -e 'length == 0' >/dev/null || rc=1
  st_assert "$rc" "add creates nothing when it refuses"

  # An epic exempt from --parent and naming nothing it comes off is the same
  # queue jump reached in two commands: nothing gates it, so nothing gates the
  # work filed under it either. #$Z is open, so the chain has an end to come off.
  rc=0; out="$(AS_JSON=0 cmd_add -t "selftest second root" --area infra --kind chore --size l --selftest 2>&1 >/dev/null)" || rc=$?
  st_assert "$([ "$rc" = 1 ] && echo 0 || echo 1)" "add refuses an epic with no --blocked-by (got $rc)"
  rc=0; printf '%s' "$out" | grep -q -- "--blocked-by" || rc=1
  st_assert "$rc" "add's refusal names the flag the epic is missing"
  rc=0; AS_JSON=1 cmd_find "selftest second root" | jq -e 'length == 0' >/dev/null || rc=1
  st_assert "$rc" "add creates nothing when it refuses an epic"

  # The decision, apart from the read that feeds it. A tracker with no open epic
  # is the one this repository cannot be put into, and it is the case the whole
  # exemption exists for: the first root does come off nothing.
  rc=0; out="$(epic_head_refusal "selftest first root" "")" || rc=$?
  st_assert "$([ "$rc" = 0 ] && [ -z "$out" ] && echo 0 || echo 1)" \
    "the first root in an empty tracker is still filable (got $rc)"

  rc=0; out="$(epic_head_refusal "selftest second root" "  ▸ #$Z  selftest root epic")" || rc=$?
  st_assert "$([ "$rc" = 1 ] && echo 0 || echo 1)" \
    "an epic is refused once the chain has an open end (got $rc)"
  rc=0; printf '%s' "$out" | grep -q "#$Z" || rc=1
  st_assert "$rc" "the refusal names open end #$Z"

  st_add "add files the parent under #$Z" \
    -t "selftest parent" --area infra --kind chore --size s --parent "$Z"
  A="$ST_NUM"
  st_add "add files a child under #$A" \
    -t "selftest child of $A" --area infra --kind chore --size s --parent "$A"
  C="$ST_NUM"
  st_add "add files an issue blocked by #$A" \
    -t "selftest blocked by $A" --area infra --kind chore --size s --blocked-by "$A" --parent "$Z"
  B="$ST_NUM"

  # A second run, simulated: filed through the same path under another marker,
  # and filed here rather than beside the assertions that read them — the label
  # listing lags a write by a second or two, and a run that cannot see its own
  # issues yet reports the wrong count.
  st_add_foreign "add files a stand-in for another run" \
    -t "selftest crashed litter one" --area infra --kind chore --size s --parent "$Z"
  X1="$ST_NUM"
  st_add_foreign "add files a second stand-in for that run" \
    -t "selftest crashed litter two" --area infra --kind chore --size s --parent "$Z"
  X2="$ST_NUM"

  rc=0; out="$(AS_JSON=1 cmd_ready)" || rc=$?
  st_assert "$rc" "ready runs against the repository's own open set"

  # Readiness is inherited, so a leaf under a blocked epic is not startable even
  # though nothing points at it. This is the whole of the epic ordering: the
  # chain is stated between epics and the work under them has to feel it.
  st_add "add files a parent gated by #$A" \
    -t "selftest gated parent" --area infra --kind chore --size s --blocked-by "$A" --parent "$Z"
  L="$ST_NUM"
  st_add "add files a child under gated #$L" \
    -t "selftest child of gated $L" --area infra --kind chore --size s --parent "$L"
  M="$ST_NUM"

  rc=0; ( TITAN_AGENT=selftest-3 cmd_claim "$M" ) >/dev/null 2>&1 || rc=$?
  st_assert "$([ "$rc" = 1 ] && echo 0 || echo 1)" \
    "claim refuses #$M under a blocked ancestor (got $rc)"

  rc=0; out="$(AS_JSON=1 cmd_find "selftest child of")" || rc=$?
  printf '%s' "$out" | jq -e --argjson n "$C" 'any(.num == $n)' >/dev/null || rc=1
  st_assert "$rc" "find matches #$C by title"

  rc=0; ( TITAN_AGENT=selftest-1 cmd_claim "$C" ) >/dev/null || rc=$?
  st_assert "$rc" "claim #$C as selftest-1"

  rc=0; ( TITAN_AGENT=selftest-2 cmd_claim "$C" ) >/dev/null 2>&1 || rc=$?
  st_assert "$([ "$rc" = 2 ] && echo 0 || echo 1)" "second claim rejected with exit 2 (got $rc)"

  rc=0; ( TITAN_AGENT=selftest-1 cmd_claim "$C" ) >/dev/null 2>&1 || rc=$?
  st_assert "$([ "$rc" = 0 ] && echo 0 || echo 1)" "re-claim by owner is idempotent"

  rc=0; out="$(TITAN_AGENT=selftest-1 AS_JSON=1 cmd_mine)" || rc=$?
  printf '%s' "$out" | jq -e --argjson n "$C" 'any(.num == $n)' >/dev/null || rc=1
  st_assert "$rc" "mine lists #$C for the agent holding it"

  rc=0; out="$(TITAN_AGENT=selftest-2 AS_JSON=1 cmd_mine)" || rc=$?
  printf '%s' "$out" | jq -e --argjson n "$C" 'all(.num != $n)' >/dev/null || rc=1
  st_assert "$rc" "mine excludes #$C for a different agent"

  # Branch ownership is asserted in a scratch repository: the real one cannot
  # have main checked out twice, and must not have branches appear and vanish
  # under another agent working in a sibling worktree.
  scratch="$(mktemp -d)"
  ST_SCRATCH="$scratch"
  st_scratch_repo "$scratch"
  ids="$( cd "$scratch" && held_agent_ids )"
  rc=0; printf '%s\n' "$ids" | grep -qx 'held/current' || rc=1
  st_assert "$rc" "held_agent_ids includes the branch checked out here"
  rc=0; printf '%s\n' "$ids" | grep -qx 'held/orphan' || rc=1
  st_assert "$rc" "held_agent_ids includes a branch no worktree holds"
  rc=0; printf '%s\n' "$ids" | grep -qx 'held/foreign' && rc=1
  st_assert "$rc" "held_agent_ids excludes a branch another worktree holds"
  rc=0; printf '%s\n' "$ids" | grep -qx 'main' && rc=1
  st_assert "$rc" "held_agent_ids excludes main"

  # `next` runs `mine` on the line after `git switch main`, which exits 1 today.
  rc=0; ( cd "$scratch" && git switch -q main && held_agent_ids ) >/dev/null 2>&1 || rc=1
  st_assert "$rc" "held_agent_ids succeeds on main"

  # The failure mode this pair exists for takes the whole script down silently,
  # so `doctor` and `release` stop on main with nothing on stderr to report.
  rc=0; out="$( cd "$scratch" && agent_id_or_empty )" || rc=$?
  st_assert "$([ "$rc" = 0 ] && [ -z "$out" ] && echo 0 || echo 1)" \
    "agent_id_or_empty yields nothing on main rather than exiting"
  rc=0; out="$( cd "$scratch" && acting_agent nobody "" )" || rc=$?
  st_assert "$([ "$rc" = 0 ] && [ -z "$out" ] && echo 0 || echo 1)" \
    "acting_agent yields nothing on main rather than exiting"

  # Detached HEAD holds no branch, so every worktree branch belongs to someone
  # else and only orphaned work is left.
  ids="$( cd "$scratch" && git switch -q --detach && held_agent_ids )"
  rc=0; printf '%s\n' "$ids" | grep -qx 'held/orphan' || rc=1
  st_assert "$rc" "held_agent_ids still finds orphaned work on a detached HEAD"

  # `git branch` names the detached state as though it were a branch.
  rc=0; printf '%s\n' "$ids" | grep -q '^(' && rc=1
  st_assert "$rc" "held_agent_ids reports no pseudo-branch on a detached HEAD"
  rm -rf "$scratch"; ST_SCRATCH=""

  # The crash-recovery case end to end: the branch that recorded the claim is
  # not checked out, so an id taken from HEAD can never match it.
  st_add "add files the issue a vanished branch will claim" \
    -t "selftest orphan claim" --area infra --kind chore --size s --parent "$Z"
  K="$ST_NUM"
  ob="selftest-orphan-$$"
  ST_ORPHAN_BRANCH="$ob"
  git branch "$ob"
  rc=0; ( TITAN_AGENT="$ob" cmd_claim "$K" ) >/dev/null || rc=$?
  st_assert "$rc" "claim #$K as a branch that is not checked out"
  rc=0; out="$(AS_JSON=1 cmd_mine)" || rc=$?
  printf '%s' "$out" | jq -e --argjson n "$K" 'any(.num == $n)' >/dev/null || rc=1
  st_assert "$rc" "mine finds #$K claimed by a local branch that is not checked out"

  # `next` tells the agent to finish or release whatever `mine` lists, so a
  # claim this checkout owns has to be settleable without --force.
  rc=0; ( cmd_release "$K" ) >/dev/null 2>&1 || rc=$?
  st_assert "$rc" "release settles #$K held by a local branch, without --force"
  git branch -D "$ob"; ST_ORPHAN_BRANCH=""

  # A claim whose branch is gone entirely is somebody else's, or nobody's.
  rc=0; ( TITAN_AGENT=selftest-vanished cmd_claim "$K" ) >/dev/null || rc=$?
  st_assert "$rc" "claim #$K as an agent no local branch carries"
  rc=0; out="$(AS_JSON=1 cmd_mine)" || rc=$?
  printf '%s' "$out" | jq -e --argjson n "$K" 'all(.num != $n)' >/dev/null || rc=1
  st_assert "$rc" "mine excludes #$K once no local branch carries its claim"
  rc=0; ( cmd_release "$K" ) >/dev/null 2>&1 || rc=$?
  st_assert "$([ "$rc" != 0 ] && echo 0 || echo 1)" "release still refuses #$K held elsewhere (got $rc)"

  # Holding nothing while claims exist elsewhere is the answer that reads as a
  # dead end, so it has to say where those claims are reported.
  rc=0; out="$(TITAN_AGENT=selftest-nobody AS_JSON=0 cmd_mine)" || rc=$?
  printf '%s' "$out" | grep -q "held elsewhere" || rc=1
  st_assert "$rc" "mine points at doctor when it holds nothing but claims exist"
  rc=0; ( TITAN_AGENT=selftest-vanished cmd_release "$K" ) >/dev/null || rc=$?
  st_assert "$rc" "release settles #$K for the agent that holds it"

  rc=0; AS_JSON=1 cmd_ready | jq -e --argjson n "$C" 'all(.num != $n)' >/dev/null || rc=1
  st_assert "$rc" "ready excludes claimed #$C"

  rc=0; ( TITAN_AGENT=selftest-1 cmd_release "$C" ) >/dev/null || rc=$?
  st_assert "$rc" "release settles #$C"
  rc=0; AS_JSON=1 cmd_ready | jq -e --argjson n "$C" 'any(.num == $n)' >/dev/null || rc=1
  st_assert "$rc" "release returns #$C to ready"

  rc=0; ( TITAN_AGENT=selftest-1 cmd_claim "$C" ) >/dev/null || rc=$?
  st_assert "$rc" "claim #$C back for the done path"
  rc=0; ( TITAN_AGENT=selftest-1 cmd_done "$C" ) >/dev/null || rc=$?
  st_assert "$rc" "done settles #$C"
  rc=0; AS_JSON=1 cmd_show "$C" | jq -e '.state == "CLOSED" and (.wip | not)' >/dev/null || rc=1
  st_assert "$rc" "done closes #$C and clears wip"

  # The reason find exists: a duplicate check that cannot see closed issues is
  # exactly the check that lets a closed issue be filed again.
  rc=0; out="$(AS_JSON=1 cmd_find "selftest child of")" || rc=$?
  printf '%s' "$out" \
    | jq -e --argjson n "$C" 'any(.num == $n and .state == "CLOSED")' >/dev/null || rc=1
  st_assert "$rc" "find still matches #$C once it is closed"

  rc=0; AS_JSON=1 cmd_ready | jq -e --argjson n "$A" 'any(.num == $n)' >/dev/null || rc=1
  st_assert "$rc" "#$A leaves container state once #$C closes"

  rc=0; out="$(TITAN_AGENT=selftest-1 cmd_done "$A" --force)" || rc=$?
  printf '%s' "$out" | grep -q "unblocked: #$B" || rc=1
  st_assert "$rc" "done #$A reports 'unblocked: #$B'"

  rc=0; AS_JSON=1 cmd_ready | jq -e --argjson n "$B" 'any(.num == $n)' >/dev/null || rc=1
  st_assert "$rc" "#$B becomes ready when its blocker closes"

  # A closed ancestor gates nothing, which is what makes finishing an epic
  # release the work under the next one without re-pointing any of it.
  rc=0; AS_JSON=1 cmd_ready | jq -e --argjson n "$M" 'any(.num == $n)' >/dev/null || rc=1
  st_assert "$rc" "#$M becomes ready once the ancestor gating it is unblocked"

  # The merge path, and the gate behind two blockers, on #$K -- which the
  # section above handed back open and unheld. `tracking.yml` sets TITAN_AGENT
  # to the head ref, which is the agent id `start` derived the branch from, so
  # it matches the claim on the issue the branch was started from and no other.
  # Every later number on a `Tracks` line therefore reaches `done` held by
  # nobody -- and refusing those leaves the run red and the issue open for a
  # second agent to rebuild.
  #
  # The half that must not move with it: what the claim check is for is one
  # agent not closing another's work, and that is a held issue, not an unheld
  # one. `release` draws the same line on the unmerged path. #$K leaves this
  # open and unheld, the way it arrived.
  rc=0; ( TITAN_AGENT=selftest-1 cmd_claim "$K" ) >/dev/null || rc=$?
  st_assert "$rc" "claim #$K as the agent that holds it"
  rc=0; ( TITAN_AGENT=selftest-2 cmd_done "$K" ) >/dev/null 2>&1 || rc=$?
  st_assert "$([ "$rc" != 0 ] && echo 0 || echo 1)" "done still refuses #$K held by another agent (got $rc)"
  rc=0; ( TITAN_AGENT=selftest-1 cmd_release "$K" ) >/dev/null || rc=$?
  st_assert "$rc" "release returns #$K to nobody"
  rc=0; ( TITAN_AGENT=selftest-merge cmd_release "$K" ) >/dev/null || rc=$?
  st_assert "$rc" "release settles unheld #$K, the way an unmerged close reaches it"

  # An issue with two blockers must not be announced when only one closes: the
  # caller would claim it and get a fatal exit 1. #$K stands behind them rather
  # than a third issue filed to be pointed at, borrowed for the two closes and
  # handed back with both edges dropped. `claim` refuses a blocked issue, which
  # is why the borrow above runs before the edges go on and not after.
  #
  # #$G is closed by an agent holding nothing, which is the merge's own half of
  # the path above: a squash merge reaches every number on a `Tracks` line that
  # way, and a close that refused them would leave the run red.
  st_add "add files the first of two blockers" \
    -t "selftest gate G" --area infra --kind chore --size s --parent "$Z"
  G="$ST_NUM"
  st_add "add files the second of two blockers" \
    -t "selftest gate H" --area infra --kind chore --size s --parent "$Z"
  H="$ST_NUM"
  rc=0; ( cmd_dep "$K" --needs "$G" --needs "$H" ) >/dev/null || rc=$?
  st_assert "$rc" "dep puts #$K behind both #$G and #$H"
  rc=0; AS_JSON=1 cmd_show "$K" | jq -e --argjson g "$G" --argjson h "$H" \
    '.blockers | (index($g) != null) and (index($h) != null)' >/dev/null || rc=1
  st_assert "$rc" "#$K stands behind both edges, not just the last one written"
  rc=0; out="$(TITAN_AGENT=selftest-merge cmd_done "$G")" || rc=$?
  st_assert "$rc" "done settles unheld #$G, the way a merge reaches it"
  rc=0; AS_JSON=1 cmd_show "$G" | jq -e '.state == "CLOSED"' >/dev/null || rc=1
  st_assert "$rc" "done closes unheld #$G"
  rc=0; printf '%s' "$out" | grep -q "unblocked:" && rc=1
  st_assert "$rc" "done #$G stays quiet: #$K still needs #$H"
  rc=0; ( TITAN_AGENT=selftest-1 cmd_claim "$H" ) >/dev/null || rc=$?
  st_assert "$rc" "claim #$H"
  rc=0; out="$(TITAN_AGENT=selftest-1 cmd_done "$H")" || rc=$?
  printf '%s' "$out" | grep -q "unblocked: #$K" || rc=1
  st_assert "$rc" "done #$H reports #$K once its last blocker closes"
  rc=0; ( cmd_dep "$K" --drop-needs "$G" --drop-needs "$H" ) >/dev/null || rc=$?
  st_assert "$rc" "dep hands #$K back with both edges dropped"
  rc=0; AS_JSON=1 cmd_show "$K" | jq -e --argjson g "$G" --argjson h "$H" \
    '(.blockers + .done_deps) | (index($g) == null) and (index($h) == null)' \
    >/dev/null || rc=1
  st_assert "$rc" "#$K carries neither edge once both are dropped"

  # size:l is refused by claim, so it must not be offered by ready.
  st_add "add files a size:l epic off closed #$head" \
    -t "selftest oversized" --area infra --kind chore --size l --blocked-by "$head"
  J="$ST_NUM"
  rc=0; AS_JSON=1 cmd_ready | jq -e --argjson n "$J" 'all(.num != $n)' >/dev/null || rc=1
  st_assert "$rc" "ready excludes size:l #$J"
  rc=0; out="$(AS_JSON=0 cmd_ready)" || rc=$?
  printf '%s' "$out" | grep -q "SPLIT:.*#$J" || rc=1
  st_assert "$rc" "ready reports #$J under SPLIT rather than hiding it"
  rc=0; ( TITAN_AGENT=selftest-1 cmd_claim "$J" ) >/dev/null 2>&1 || rc=$?
  st_assert "$([ "$rc" = 1 ] && echo 0 || echo 1)" "claim refuses size:l #$J (got $rc)"

  # `plan` reads the chain rather than the forest: epics in the order they come
  # off each other, the ones nothing blocks expanded, everything else a row.
  st_add "add files an epic behind #$J" \
    -t "selftest epic behind $J" --area infra --kind chore --size l --blocked-by "$J"
  N="$ST_NUM"
  st_add "add files a child under epic #$N" \
    -t "selftest child of epic $N" --area infra --kind chore --size s --parent "$N"
  R="$ST_NUM"
  rc=0; out="$(AS_JSON=1 cmd_plan)" || rc=$?
  st_assert "$rc" "plan runs with epics #$J and #$N in the chain"

  rc=0; printf '%s' "$out" | jq -e --argjson j "$J" --argjson n "$N" '
    (map(.num) | index($j)) as $a | (map(.num) | index($n)) as $b
    | ($a != null) and ($b != null) and ($a < $b)' >/dev/null || rc=1
  st_assert "$rc" "plan orders #$J before the epic it blocks, #$N"

  rc=0; printf '%s' "$out" | jq -e --argjson j "$J" \
    'any(.num == $j and .current)' >/dev/null || rc=1
  st_assert "$rc" "plan marks unblocked epic #$J as current"

  rc=0; printf '%s' "$out" | jq -e --argjson n "$N" \
    'any(.num == $n and (.current | not))' >/dev/null || rc=1
  st_assert "$rc" "plan leaves epic #$N behind a blocker uncurrent"

  rc=0; printf '%s' "$out" | jq -e --argjson n "$N" --argjson r "$R" '
    any(.num == $n and (.children | any(.num == $r and .stance == "waiting")))' >/dev/null || rc=1
  st_assert "$rc" "plan carries #$R under #$N as waiting on its gated epic"

  # An open end is where the chain stops, so #$N is one and #$J stopped being
  # one the moment #$N came off it. Being blocked has nothing to do with it: a
  # new epic follows the last epic filed, not the last one startable.
  out="$(epic_chain epic_is_end)"
  rc=0; printf '%s' "$out" | grep -q "▸ #$N  " || rc=1
  st_assert "$rc" "the chain marks blocked epic #$N as an open end"
  rc=0; printf '%s' "$out" | grep -q "▸ #$J  " && rc=1
  st_assert "$rc" "the chain unmarks #$J once #$N comes off it"

  # #$N is an epic behind a blocker, so it is real work with a real place in the
  # chain that nobody can start yet. A refusal listing only the startable epics
  # would send work belonging under it to whichever epic happens to be
  # unblocked, which is the queue jump the refusal exists to stop.
  rc=0; out="$(AS_JSON=0 cmd_add -t "selftest parentless again" --area infra --kind chore --size s --selftest 2>&1 >/dev/null)" || rc=$?
  st_assert "$([ "$rc" = 1 ] && echo 0 || echo 1)" "add still refuses once a blocked epic exists (got $rc)"
  rc=0; printf '%s' "$out" | grep -q " #$N  " || rc=1
  st_assert "$rc" "add's refusal lists blocked epic #$N, not just the startable ones"
  rc=0; printf '%s' "$out" | grep -q "▸ #$N  " && rc=1
  st_assert "$rc" "add's refusal leaves blocked epic #$N unmarked"
  rc=0; printf '%s' "$out" | grep -q "▸ #$J  " || rc=1
  st_assert "$rc" "add's refusal marks startable epic #$J"

  # Between a draft pull request going up and a human merging it, the work is
  # finished and the issue is still held. "Leave it alone" and "there is nothing
  # left to do here" are opposite answers, so they must not read the same.
  #
  # The board is read off #$J rather than an epic filed to be read off. It is
  # unblocked, so its children are claimable, and it is childless until the
  # `dep` moves below hand it #$K and #$R -- which is why this sits here and not
  # after them: the board is an assertion about the whole of an epic's children,
  # so it has to be the whole of them.
  st_add "add files the child that will be submitted" \
    -t "selftest submitted child of $J" --area infra --kind chore --size s --parent "$J"
  S="$ST_NUM"
  st_add "add files a child waiting behind #$S" \
    -t "selftest behind submitted $S" --area infra --kind chore --size s --parent "$J" --blocked-by "$S"
  T="$ST_NUM"
  st_add "add files a ready child under #$J" \
    -t "selftest ready child of $J" --area infra --kind chore --size s --parent "$J"
  U="$ST_NUM"
  st_add "add files a child to be claimed under #$J" \
    -t "selftest claimed child of $J" --area infra --kind chore --size s --parent "$J"
  V="$ST_NUM"
  rc=0; ( TITAN_AGENT=selftest-1 cmd_claim "$S" ) >/dev/null || rc=$?
  st_assert "$rc" "claim #$S before submitting it"
  rc=0; ( TITAN_AGENT=selftest-2 cmd_claim "$V" ) >/dev/null || rc=$?
  st_assert "$rc" "claim #$V as a second agent"
  rc=0; ( TITAN_AGENT=selftest-1 cmd_submit "$S" --pr 4242 ) >/dev/null || rc=$?
  st_assert "$rc" "submit puts #$S into review"

  rc=0; AS_JSON=1 cmd_show "$S" \
    | jq -e '.review and .wip and .submit.pr == 4242' >/dev/null || rc=1
  st_assert "$rc" "show reports #$S in review on #4242, and still claimed"

  # Work is unblocked when its blocker reaches main, not when a draft exists.
  rc=0; AS_JSON=1 cmd_ready | jq -e --argjson n "$T" 'all(.num != $n)' >/dev/null || rc=1
  st_assert "$rc" "ready still excludes #$T behind submitted #$S"

  rc=0; AS_JSON=1 cmd_blocked | jq -e --argjson n "$T" --argjson s "$S" \
      'any(.num == $n and (.blockers | index($s) != null))' >/dev/null || rc=1
  st_assert "$rc" "blocked still lists #$T <- #$S"

  rc=0; out="$(TITAN_AGENT=selftest-1 AS_JSON=1 cmd_mine)" || rc=$?
  printf '%s' "$out" | jq -e --argjson n "$S" \
      'any(.num == $n and .review)' >/dev/null || rc=1
  st_assert "$rc" "mine separates submitted #$S from work still being built"

  rc=0; out="$(AS_JSON=1 cmd_plan)" || rc=$?
  st_assert "$rc" "plan runs with #$S in review under #$J"
  rc=0; printf '%s' "$out" | jq -e --argjson j "$J" --argjson s "$S" '
    any(.num == $j and (.children | any(.num == $s and .stance == "review")))' >/dev/null || rc=1
  st_assert "$rc" "plan carries #$S under #$J as in review"

  rc=0; printf '%s' "$out" \
    | jq -e --argjson j "$J" --argjson s "$S" --argjson t "$T" \
           --argjson u "$U" --argjson v "$V" '
        any(.num == $j and ([.children[].num] == [$u, $v, $s, $t]))' >/dev/null || rc=1
  st_assert "$rc" "plan boards #$J ready, claimed, review, waiting — not by number"

  # An epic reading n/n done while nothing has reached main is the tracker
  # asserting exactly what `next` forbids an agent from asserting.
  rc=0; printf '%s' "$out" | jq -e --argjson j "$J" \
      'any(.num == $j and .review == 1 and .done == 0)' >/dev/null || rc=1
  st_assert "$rc" "plan counts #$S in review rather than done"

  rc=0; ( TITAN_AGENT=selftest-2 cmd_submit "$T" ) >/dev/null 2>&1 || rc=$?
  st_assert "$([ "$rc" = 1 ] && echo 0 || echo 1)" \
    "submit refuses #$T, which nobody holds (got $rc)"

  rc=0; ( TITAN_AGENT=selftest-1 cmd_release "$S" ) >/dev/null || rc=$?
  st_assert "$rc" "release settles submitted #$S"
  rc=0; AS_JSON=1 cmd_show "$S" | jq -e '(.review | not) and (.wip | not)' >/dev/null || rc=1
  st_assert "$rc" "release clears review from #$S"

  rc=0; AS_JSON=1 cmd_ready | jq -e --argjson n "$S" 'any(.num == $n)' >/dev/null || rc=1
  st_assert "$rc" "release returns submitted #$S to ready"

  rc=0; ( TITAN_AGENT=selftest-1 cmd_claim "$S" ) >/dev/null || rc=$?
  st_assert "$rc" "claim #$S back for the done path"
  rc=0; ( TITAN_AGENT=selftest-1 cmd_submit "$S" --pr 4242 ) >/dev/null || rc=$?
  st_assert "$rc" "submit #$S again before closing it"
  rc=0; ( TITAN_AGENT=selftest-1 cmd_done "$S" ) >/dev/null || rc=$?
  st_assert "$rc" "done settles submitted #$S"
  rc=0; AS_JSON=1 cmd_show "$S" \
    | jq -e '.state == "CLOSED" and (.review | not)' >/dev/null || rc=1
  st_assert "$rc" "done clears review and closes #$S"

  rc=0; ( TITAN_AGENT=selftest-1 cmd_submit "$S" --pr 4242 ) >/dev/null 2>&1 || rc=$?
  st_assert "$([ "$rc" = 1 ] && echo 0 || echo 1)" "submit refuses closed #$S (got $rc)"

  # A closed parent is not a parent: the walk stops there, so #$K would be gated
  # by nothing and startable ahead of the chain, and `plan` would list it under
  # no epic. #466 made --parent mandatory for a non-epic, which turned a closed
  # number into the way to satisfy a required flag without joining a chain. The
  # refusal has to land before the write in both commands, or the state it
  # exists to prevent is already on the tracker by the time it is reported.
  rc=0; out="$(AS_JSON=0 cmd_add -t "selftest closed parent" --area infra --kind chore --size s --parent "$A" --selftest 2>&1 >/dev/null)" || rc=$?
  st_assert "$([ "$rc" = 1 ] && echo 0 || echo 1)" "add refuses closed #$A as a parent (got $rc)"
  rc=0; printf '%s' "$out" | grep -q "#$A is CLOSED" || rc=1
  st_assert "$rc" "add's refusal says #$A is closed"
  rc=0; AS_JSON=1 cmd_find "selftest closed parent" | jq -e 'length == 0' >/dev/null || rc=1
  st_assert "$rc" "add creates nothing when the parent is closed"

  # Re-pointing the child is the right move when the child was aimed at the
  # wrong epic, and the wrong one when the epic was closed by mistake. A
  # refusal that offers only the first reading sends an agent to `gh`, which
  # AGENTS.md forbids, because this script had no move for the second.
  rc=0; printf '%s' "$out" | grep -q "track.sh reopen" || rc=1
  st_assert "$rc" "add's refusal offers reopen as the other reading"

  # `dep <n> --child C` parents C under n, so n is the number that has to be
  # open -- not the one the flag names.
  rc=0; out="$( ( cmd_dep "$A" --child "$K" ) 2>&1 >/dev/null )" || rc=$?
  st_assert "$([ "$rc" = 1 ] && echo 0 || echo 1)" "dep refuses closed #$A as a parent (got $rc)"
  rc=0; printf '%s' "$out" | grep -q "#$A is CLOSED" || rc=1
  st_assert "$rc" "dep's refusal says #$A is closed"
  rc=0; AS_JSON=1 cmd_show "$K" | jq -e --argjson z "$Z" '.parent.num == $z' >/dev/null || rc=1
  st_assert "$rc" "dep writes nothing when the parent is closed: #$K still under #$Z"

  rc=0; ( cmd_dep "$J" --child "$K" ) >/dev/null || rc=$?
  st_assert "$rc" "dep still moves #$K under open #$J"
  rc=0; AS_JSON=1 cmd_show "$K" | jq -e --argjson j "$J" '.parent.num == $j' >/dev/null || rc=1
  st_assert "$rc" "show reports #$K under its new parent #$J"

  # `dep --parent` is the third place a parent reaches the tracker, and it is the
  # move `--drop-child`'s refusal recommends -- so without this the hole is open
  # through the very command that exists to keep work in a chain.
  rc=0; out="$( ( cmd_dep "$K" --parent "$A" ) 2>&1 >/dev/null )" || rc=$?
  st_assert "$([ "$rc" = 1 ] && echo 0 || echo 1)" "dep --parent refuses closed #$A (got $rc)"
  rc=0; printf '%s' "$out" | grep -q "#$A is CLOSED" || rc=1
  st_assert "$rc" "dep --parent's refusal says #$A is closed"
  rc=0; AS_JSON=1 cmd_show "$K" | jq -e --argjson j "$J" '.parent.num == $j' >/dev/null || rc=1
  st_assert "$rc" "dep --parent writes nothing when it refuses: #$K still under #$J"

  # The half that must NOT fire. A closed issue is a legitimate blocker -- that
  # is what finished work looks like -- so the check is on `--child` alone, not
  # on `n`. Hoisted above the argument loop, or keyed on `n` whatever the flag,
  # it would refuse every closed blocker and every assertion above would still
  # pass.
  rc=0; ( cmd_dep "$K" --needs "$C" ) >/dev/null || rc=$?
  st_assert "$rc" "dep still takes closed #$C as a blocker for #$K"

  # The move those refusals point at. An issue that is already open has nothing
  # to restore, and reopening it would write a marker saying otherwise.
  rc=0; ( cmd_reopen "$Z" ) >/dev/null 2>&1 || rc=$?
  st_assert "$([ "$rc" = 1 ] && echo 0 || echo 1)" "reopen refuses open #$Z (got $rc)"

  # `done` cleared wip on the way past, so what comes back is open, unheld work
  # -- reopening restores the issue, not the claim that was on it.
  rc=0; out="$(cmd_reopen "$A")" || rc=$?
  st_assert "$rc" "reopen runs on closed #$A"
  rc=0; AS_JSON=1 cmd_show "$A" | jq -e '.state == "OPEN" and (.wip | not)' >/dev/null || rc=1
  st_assert "$rc" "reopen returns #$A to open, unheld work"

  # The mirror of `done #$A reports 'unblocked: #$B'`. Closing announced what it
  # released; reopening has to announce what it gates again, or a caller stays
  # pointed at a row whose claim now exits 1. Matched exactly, since `#$B` is a
  # prefix of every longer number the list could also hold.
  rc=0; printf '%s' "$out" | sed -n 's/.*re-blocked: //p' | tr ' ' '\n' \
    | grep -qx "#$B" || rc=1
  st_assert "$rc" "reopen #$A names #$B as re-blocked"

  rc=0; AS_JSON=1 cmd_ready | jq -e --argjson n "$B" 'all(.num != $n)' >/dev/null || rc=1
  st_assert "$rc" "#$B leaves ready again once #$A reopens"

  # Readiness is inherited, so the restored gate has to carry down the same walk
  # the close released #$M through -- #$L blocked by #$A once more.
  rc=0; AS_JSON=1 cmd_ready | jq -e --argjson n "$M" 'all(.num != $n)' >/dev/null || rc=1
  st_assert "$rc" "#$M is gated again through reopened #$A"

  # What the refusal sent the caller here to do. A reopen that leaves the parent
  # still unusable has restored the state and not the capability.
  st_add "add takes reopened #$A as a parent" \
    -t "selftest child of reopened $A" --area infra --kind chore --size s --parent "$A"
  Q="$ST_NUM"

  # #$A is borrowed from the block above, and assertions further down still read
  # #$B as ready. Handing it back closed is what keeps that true -- left open,
  # this fails a cycle assertion two hundred lines below, for a reason nothing
  # there could explain. The round trip is worth stating anyway: an undo that
  # cannot be undone again is a one-way door.
  rc=0; ( TITAN_AGENT=selftest-1 cmd_done "$A" --force ) >/dev/null || rc=$?
  st_assert "$rc" "done closes reopened #$A again"
  rc=0; AS_JSON=1 cmd_ready | jq -e --argjson n "$B" 'any(.num == $n)' >/dev/null || rc=1
  st_assert "$rc" "#$B returns to ready once #$A closes again"

  # `add` refuses to file a non-epic with no parent; dropping one out of its
  # epic reaches the same state from the other side. GitHub gives an issue one
  # parent, so `--drop-child` removes the only one #$R has rather than moving
  # it, and it comes out gated by nothing and startable ahead of the chain it
  # belongs to. The refusal has to land before the write, or the orphan it
  # exists to prevent is already on the tracker by the time it is reported.
  rc=0; out="$( cmd_dep "$N" --drop-child "$R" 2>&1 >/dev/null )" || rc=$?
  st_assert "$([ "$rc" = 1 ] && echo 0 || echo 1)" "dep refuses to orphan non-epic #$R (got $rc)"
  rc=0; printf '%s' "$out" | grep -q "#$R is not an epic" || rc=1
  st_assert "$rc" "dep's refusal names the child it would orphan, #$R"
  rc=0; printf '%s' "$out" | grep -q -- "--parent" || rc=1
  st_assert "$rc" "dep's refusal says to re-point #$R with --parent"
  rc=0; AS_JSON=1 cmd_show "$R" | jq -e --argjson n "$N" '.parent.num == $n' >/dev/null || rc=1
  st_assert "$rc" "dep writes nothing when it refuses: #$R still under #$N"

  # The way out of the refusal, and what makes it one: re-shaping an epic is
  # moving a child, which is one write that never leaves it loose.
  rc=0; ( cmd_dep "$R" --parent "$J" ) >/dev/null 2>&1 || rc=$?
  st_assert "$rc" "dep --parent moves #$R from #$N to #$J"
  rc=0; AS_JSON=1 cmd_show "$R" | jq -e --argjson j "$J" '.parent.num == $j' >/dev/null || rc=1
  st_assert "$rc" "show reports #$R under its new parent #$J"

  # An epic is a root and takes no parent, so dropping one is not the orphaning
  # this refuses. The rule is about non-epics, not about `--drop-child`.
  rc=0; ( cmd_dep "$J" --child "$N" ) >/dev/null 2>&1 || rc=$?
  st_assert "$rc" "dep parents epic #$N under epic #$J"
  rc=0; ( cmd_dep "$J" --drop-child "$N" ) >/dev/null 2>&1 || rc=$?
  st_assert "$rc" "dep still drops epic #$N out of #$J"
  rc=0; AS_JSON=1 cmd_show "$N" | jq -e '.parent == null' >/dev/null || rc=1
  st_assert "$rc" "show reports epic #$N back at the head of its own chain"

  # A whitespace agent id would produce an unmatchable claim marker.
  rc=0; ( TITAN_AGENT="bad id" cmd_claim "$J" ) >/dev/null 2>&1 || rc=$?
  st_assert "$([ "$rc" = 1 ] && echo 0 || echo 1)" "claim rejects a whitespace agent id (got $rc)"

  # start derives the agent id from the title, so the slug has to survive
  # whatever punctuation a title carries.
  bn="$(branch_for feat 74 'Lock-free SPSC ring: for the "audio" boundary!')"
  rc=0
  case "$bn" in feat/74-*) ;; *) rc=1 ;; esac
  case "$bn" in *[!A-Za-z0-9._/-]*) rc=1 ;; esac
  st_assert "$rc" "branch_for builds a claimable agent id ($bn)"
  rc=0; ( validate_agent "$bn" ) >/dev/null 2>&1 || rc=1
  st_assert "$rc" "branch_for output passes validate_agent"

  # #96 is the case this exists for: `Closes #98, #99, #100, #101` closed only
  # #98, because a GitHub keyword binds to the number directly after it.
  rc=0; out="$(printf 'Tracks #98, #99, #100, #101\n' | cmd_refs)" || rc=$?
  [ "$out" = "$(printf '98\n99\n100\n101')" ] || rc=1
  st_assert "$rc" "refs takes every number on a Tracks line"

  rc=0; out="$(printf 'Unlike #96, this parses the body itself.\n\nTracks #116\n' | cmd_refs)" || rc=$?
  [ "$out" = "116" ] || rc=1
  st_assert "$rc" "refs leaves a mention outside a Tracks line alone"

  # The template carries its own instructions in an HTML comment, so a body that
  # keeps them must not settle whatever issue the example names.
  rc=0; out="$(printf '<!--\n  Link the issue: Tracks #12\n-->\nTracks #116\n' | cmd_refs)" || rc=$?
  [ "$out" = "116" ] || rc=1
  st_assert "$rc" "refs ignores a Tracks line inside an HTML comment"

  rc=0; out="$(printf 'Tracks #7\n\nTracks #7 as well\n' | cmd_refs)" || rc=$?
  [ "$out" = "7" ] || rc=1
  st_assert "$rc" "refs reports each issue once"

  rc=0; out="$(printf 'A pull request that tracks nothing.\n' | cmd_refs)" || rc=1
  [ -z "$out" ] || rc=1
  st_assert "$rc" "refs succeeds and stays silent when nothing is tracked"

  # GitHub rejects a direct 2-cycle server-side but does NOT check transitively,
  # so a 3-cycle is reachable and is what we must detect. Verified 2026-08-03.
  # The 3-cycle is derived over a fixture above: filed for real it is a fault
  # left in the repository's own graph until cleanup, and every run started
  # inside that window reads it as one of its own.
  #
  # What is left here is the half a fixture cannot state — that the server
  # refuses the direct case at all. It runs over two issues this run already
  # filed, so proving it costs an edge rather than three more issues.
  rc=0; ( cmd_dep "$L" --needs "$M" ) >/dev/null 2>&1 || rc=$?
  st_assert "$rc" "dep points #$L at #$M, leaving the reverse edge a 2-cycle"

  rc=0; out="$(st_cycle_fixture '[]' | own_cycle)" || rc=1
  [ "$out" = true ] || rc=1
  st_assert "$rc" "a cycle among the repository's own issues still fails doctor"

  rc=0; out="$(st_cycle_fixture '[{"name":"track:selftest"}]' | own_cycle)" || rc=1
  [ "$out" = false ] || rc=1
  st_assert "$rc" "a cycle among throwaway issues is not the repository's"

  # A direct 2-cycle is refused by the server; the wrapper must surface that.
  rc=0; ( cmd_dep "$M" --needs "$L" ) >/dev/null 2>&1 || rc=$?
  st_assert "$([ "$rc" != 0 ] && echo 0 || echo 1)" "server rejects a direct 2-cycle, wrapper exits non-zero (got $rc)"

  # ---------------------------------------------------------- run scoping ---
  # Cleanup used to delete every issue carrying track:selftest, repo-wide, so
  # two runs at once destroyed each other's fixtures and the loser died on a
  # number that no longer resolved. A marker on the issue is what makes a run's
  # own work nameable: a snapshot of the label taken before the run cannot tell
  # two runs apart, and refusing to start while any exist would let one crashed
  # run's litter block every run after it.
  #
  # Every call below is wrapped, because `die` calls `exit` and `||` does not
  # catch an exit: an unwrapped positive-path call ends the run instead of
  # recording one FAIL.
  rc=0; ( AS_JSON=1 cmd_show "$A" ) \
    | jq -e --arg m "$ST_RUN" '.title | contains($m)' >/dev/null || rc=1
  st_assert "$rc" "add stamps this run's marker on #$A"

  rc=0; ( ST_RUN="" cmd_add -t "selftest unmarked" --area infra --kind chore \
          --size s --parent "$Z" --selftest ) >/dev/null 2>&1 || rc=$?
  st_assert "$([ "$rc" = 1 ] && echo 0 || echo 1)" \
    "add --selftest refuses to file with no run marker (got $rc)"
  rc=0; ( AS_JSON=1 cmd_find "selftest unmarked" ) | jq -e 'length == 0' >/dev/null || rc=1
  st_assert "$rc" "add creates nothing when it has no run marker"

  out="$(st_delete_run "$ST_FOREIGN_RUN" 2>&1)" || true
  rc=0; printf '%s' "$out" | grep -q "removed 2 throwaway" || rc=1
  st_assert "$rc" "cleanup counts only the run it names"
  rc=0; ( AS_JSON=1 cmd_find "$ST_FOREIGN_RUN" ) \
    | jq -e '[.[] | select(.state == "OPEN")] | length == 0' >/dev/null || rc=1
  st_assert "$rc" "cleanup removes every issue of the run it names"
  rc=0; ( AS_JSON=1 cmd_find "$ST_RUN" ) \
    | jq -e --argjson n "$Z" 'any(.num == $n)' >/dev/null || rc=1
  st_assert "$rc" "cleanup leaves a concurrent run's fixtures alone"

  # A run closes throwaway issues as it goes, so by now the newest closed issue
  # in the repository is one — and a run that hangs its whole chain off it dies
  # when the run that owns it cleans up.
  head="$( st_chain_head )"
  rc=0; ( AS_JSON=1 cmd_show "$head" ) \
    | jq -e '.title | startswith("selftest ") | not' >/dev/null || rc=1
  st_assert "$rc" "the chain head is not a throwaway issue (#$head)"

  # Informational canary: the search index is expected to disagree (it lags writes).
  # This documents WHY local derivation is the primary path. Never fails the run.
  loc="$(AS_JSON=1 cmd_ready | jq -r '[.[].num] | sort | join(",")' 2>/dev/null || echo 'n/a')"
  adv="$(gh issue list --search 'is:open -is:blocked' --limit 100 --json number \
         --jq '[.[].number] | sort | join(",")' 2>/dev/null || echo 'n/a')"
  if [ "$adv" != "$loc" ]; then
    note "  note  advanced-search ready set differs from locally-derived set"
    note "        local:  $loc"
    note "        search: $adv"
    note "        (expected — the search index lags writes. Local derivation wins.)"
  else
    note "  note  advanced-search agrees with local derivation"
  fi

  st_add_foreign "add files what a crashed run would leave behind" \
    -t "selftest surviving litter" --area infra --kind chore --size s --parent "$Z"
  Y="$ST_NUM"

  # Re-armed rather than cleared: a live stand-in fixture and the lock both
  # outlive this point, so a Ctrl-C here would leak one and strand the other.
  # They come off after the last delete.
  trap 'st_delete_run "$ST_FOREIGN_RUN" >/dev/null; lock_release' EXIT
  trap 'st_delete_run "$ST_FOREIGN_RUN" >/dev/null; lock_release; exit 130' INT TERM
  st_cleanup
  rc=0; ( AS_JSON=1 cmd_find "$ST_FOREIGN_RUN" ) \
    | jq -e --argjson n "$Y" 'any(.num == $n)' >/dev/null || rc=1
  st_assert "$rc" "cleanup leaves a crashed run's litter behind"

  rc=0; out="$( cmd_selftest --clean 2>&1 )" || rc=$?
  st_assert "$([ "$rc" = 1 ] && echo 0 || echo 1)" \
    "selftest --clean refuses without --yes (got $rc)"
  rc=0; printf '%s' "$out" | grep -q "every run" || rc=1
  st_assert "$rc" "the refusal says --clean takes every run's issues"

  # The command itself, against the one set it can take without reaching a
  # concurrent run's live fixtures. Naming a marker is also what lets a person
  # clear a crashed run without stopping the run beside it.
  rc=0; out="$( cmd_selftest --clean "$ST_FOREIGN_RUN" --yes )" || rc=$?
  st_assert "$rc" "selftest --clean <marker> --yes clears that run (got $rc)"
  rc=0; printf '%s' "$out" | grep -q "^cleaned 1 throwaway" || rc=1
  st_assert "$rc" "the explicit clean reports what it removed"
  rc=0; ( AS_JSON=1 cmd_find "$ST_FOREIGN_RUN" ) \
    | jq -e '[.[] | select(.state == "OPEN")] | length == 0' >/dev/null || rc=1
  st_assert "$rc" "clearing a crashed run's litter is an explicit action"
  trap - EXIT INT TERM

  dt=$(( $(date +%s) - t0 ))
  if [ "$ST_FAIL" -eq 0 ]; then
    printf 'selftest passed %s/%s in %ss\n' "$ST_PASS" "$((ST_PASS + ST_FAIL))" "$dt"
  else
    printf 'selftest FAILED %s/%s in %ss\n' "$ST_FAIL" "$((ST_PASS + ST_FAIL))" "$dt"
    exit 1
  fi
  return 0
}

# --------------------------------------------------------------- dispatch ---
AS_JSON=0
ARGS=()
for a in "$@"; do
  if [ "$a" = "--json" ]; then AS_JSON=1; else ARGS[${#ARGS[@]}]="$a"; fi
done
set -- ${ARGS[@]+"${ARGS[@]}"}
[ $# -gt 0 ] || usage
CMD="$1"; shift

case "$CMD" in
  ready)        cmd_ready "$@" ;;
  refs)         cmd_refs "$@" ;;
  blocked)      cmd_blocked "$@" ;;
  plan)         cmd_plan "$@" ;;
  find)         cmd_find "$@" ;;
  show)         cmd_show "$@" ;;
  start)        cmd_start "$@" ;;
  claim)        cmd_claim "$@" ;;
  mine)         cmd_mine "$@" ;;
  submit)       cmd_submit "$@" ;;
  release)      cmd_release "$@" ;;
  done)         cmd_done "$@" ;;
  reopen)       cmd_reopen "$@" ;;
  add)          cmd_add "$@" ;;
  dep)          cmd_dep "$@" ;;
  note)         cmd_note "$@" ;;
  graph)        cmd_graph "$@" ;;
  labels-init)  cmd_labels_init "$@" ;;
  doctor)       cmd_doctor "$@" ;;
  selftest)     cmd_selftest "$@" ;;
  -h|--help)    usage ;;
  *)            note "unknown command: $CMD"; usage ;;
esac
