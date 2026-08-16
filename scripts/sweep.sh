#!/usr/bin/env bash
# scripts/sweep.sh — remove local branches and worktrees whose work has landed.
#
#   scripts/sweep.sh            what would go, and what stays and why
#   scripts/sweep.sh --yes      apply it
#   scripts/sweep.sh --selftest exercise the rules on a throwaway repository
#
# Removal needs evidence that the work is in main. A squash merge rewrites the
# branch into a single new commit, so the branch keeps commits that are not
# ancestors of main and `git branch --merged` lists nothing at all — which is
# why an upstream marked `gone` is not accepted here as evidence. Gone means
# the remote branch was deleted, and a remote branch can be deleted at any time
# for any reason, including abandoning the work.
#
# Structured output -> stdout.  Progress and diagnostics -> stderr.
# Exit codes:
#   0  success — including a dry run that found nothing
#   1  error
#
# Written for bash 3.2 (macOS /bin/bash): no associative arrays, no mapfile.

set -euo pipefail

die()  { printf 'ERROR: %s\n' "$*" >&2; exit 1; }
note() { printf '%s\n' "$*" >&2; }

usage() {
  cat >&2 <<'USAGE'
scripts/sweep.sh [--yes] [--no-fetch]

  (no flags)   print what would be removed and what is kept, and change nothing
  --yes        apply the plan
  --no-fetch   skip `git fetch --prune`, and judge against the refs on disk
  --selftest   run the rules against a throwaway repository

A branch is removed only on evidence that its work is in main: a merged pull
request, an ancestor of main, or every file it touched already matching main.
Never removed: main, a branch checked out in a worktree that is staying, a
worktree holding uncommitted changes, and a locked worktree.
USAGE
  exit 1
}

# ------------------------------------------------------------------ setup ---
# The main worktree, not $0's directory: run from inside a worktree, the thing
# to sweep is the repository that worktree belongs to.
repo_root() {
  git worktree list --porcelain 2>/dev/null | awk 'NR==1{print substr($0,10); exit}'
}

# Prefer the remote ref: it is what "landed" means. A local main can sit behind
# it, which would leave merged work looking unmerged, or ahead of it with
# commits nobody else has, which would call unmerged work landed.
main_ref() {
  local ref
  for ref in origin/main origin/master main master; do
    if git rev-parse --verify --quiet "$ref^{commit}" >/dev/null; then
      printf '%s' "$ref"
      return 0
    fi
  done
  return 1
}

# Branch names of merged pull requests. Absent gh, or offline, this is empty
# and the ancestor and tree rules carry the decision on their own.
#
# SWEEP_MERGED_BRANCHES overrides the query with a newline-separated list, so
# the selftest can exercise this rule without a network or a real repository.
merged_pull_request_branches() {
  if [ -n "${SWEEP_MERGED_BRANCHES:-}" ]; then
    printf '%s\n' "$SWEEP_MERGED_BRANCHES"
    return 0
  fi
  command -v gh >/dev/null 2>&1 || { note "gh not found — judging by ancestry and tree only"; return 0; }
  gh pr list --state merged --limit "${SWEEP_PR_LIMIT:-100}" \
    --json headRefName --jq '.[].headRefName' 2>/dev/null \
    || note "could not read pull requests — judging by ancestry and tree only"
}

# path <tab> branch <tab> locked, one worktree per line, main worktree first.
worktree_table() {
  git worktree list --porcelain | awk '
    /^worktree /  { path = substr($0, 10); branch = ""; locked = 0 }
    /^branch /    { branch = substr($0, 8); sub(/^refs\/heads\//, "", branch) }
    /^locked/     { locked = 1 }
    /^$/          { if (path != "") print path "\t" branch "\t" locked; path = "" }
    END           { if (path != "") print path "\t" branch "\t" locked }
  '
}

# --------------------------------------------------------------- evidence ---
# Prints why the branch may go, or nothing at all. Order matters only for the
# message: the cheapest and most specific reason wins.
landed_reason() {
  local branch="$1" main="$2" merged_list="$3" base paths

  if [ "$(git rev-parse "$branch")" = "$(git rev-parse "$main")" ]; then
    return 0
  fi
  if printf '%s\n' "$merged_list" | grep -qxF -- "$branch"; then
    printf 'pull request merged'
    return 0
  fi
  if git merge-base --is-ancestor "$branch" "$main" 2>/dev/null; then
    printf 'already in %s' "$main"
    return 0
  fi

  # What the branch changed, judged only where it changed it. Comparing whole
  # trees instead would call a squash-merged branch unmerged the moment any
  # other pull request lands on main, which is the common case rather than the
  # corner: main moves between the merge and the sweep.
  base="$(git merge-base "$main" "$branch" 2>/dev/null)" || return 0
  paths="$(git diff --name-only "$base" "$branch" 2>/dev/null)"
  if [ -n "$paths" ]; then
    if printf '%s\n' "$paths" | tr '\n' '\0' |
       xargs -0 git diff --quiet "$main" "$branch" -- 2>/dev/null; then
      printf 'everything it changed is in %s' "$main"
      return 0
    fi
  fi
  return 0
}

protected_branch() {
  case "$1" in
    main|master) return 0 ;;
    *)           return 1 ;;
  esac
}

# ------------------------------------------------------------------- plan ---
PLAN_WORKTREES=""
PLAN_BRANCHES=""
KEPT=""

plan_worktree() { PLAN_WORKTREES="${PLAN_WORKTREES}$1	$2	$3
"; }
plan_branch()   { PLAN_BRANCHES="${PLAN_BRANCHES}$1	$2
"; }
keep()          { KEPT="${KEPT}$1	$2	$3
"; }

build_plan() {
  local main="$1" merged_list="$2" root="$3"
  local path branch locked reason staying="" going=""

  while IFS=$'\t' read -r path branch locked; do
    [ -n "$path" ] || continue
    if [ "$path" = "$root" ]; then
      staying="${staying}${branch}
"
      continue
    fi
    if [ "$locked" = "1" ]; then
      keep worktree "$path" "locked"
      staying="${staying}${branch}
"
      continue
    fi
    if [ -z "$branch" ]; then
      keep worktree "$path" "detached HEAD — nothing to judge it by"
      continue
    fi
    if [ -n "$(git -C "$path" status --porcelain 2>/dev/null)" ]; then
      keep worktree "$path" "uncommitted changes"
      staying="${staying}${branch}
"
      continue
    fi
    if protected_branch "$branch"; then
      keep worktree "$path" "holds $branch"
      staying="${staying}${branch}
"
      continue
    fi
    reason="$(landed_reason "$branch" "$main" "$merged_list")"
    if [ -n "$reason" ]; then
      plan_worktree "$path" "$branch" "$reason"
      going="${going}${branch}
"
    else
      keep worktree "$path" "$branch has not landed"
      staying="${staying}${branch}
"
    fi
  done <<EOF
$(worktree_table)
EOF

  while IFS= read -r branch; do
    [ -n "$branch" ] || continue
    if protected_branch "$branch"; then
      continue
    fi
    if printf '%s' "$staying" | grep -qxF -- "$branch"; then
      keep branch "$branch" "checked out in a worktree that is staying"
      continue
    fi
    # Already queued for deletion alongside its worktree. Listing it twice
    # deletes it twice, and the second delete fails on a branch that is gone.
    if printf '%s' "$going" | grep -qxF -- "$branch"; then
      continue
    fi
    if [ "$(git rev-parse "$branch")" = "$(git rev-parse "$main")" ]; then
      keep branch "$branch" "no commits of its own"
      continue
    fi
    reason="$(landed_reason "$branch" "$main" "$merged_list")"
    if [ -n "$reason" ]; then
      plan_branch "$branch" "$reason"
    else
      keep branch "$branch" "not merged"
    fi
  done <<EOF
$(git for-each-ref --format='%(refname:short)' refs/heads)
EOF
}

report() {
  local path branch reason kind name
  local removals=0 keeps=0

  while IFS=$'\t' read -r path branch reason; do
    [ -n "$path" ] || continue
    printf 'remove worktree  %s\n         branch  %s  (%s)\n' "$path" "$branch" "$reason"
    removals=$((removals + 1))
  done <<EOF
$PLAN_WORKTREES
EOF

  while IFS=$'\t' read -r branch reason; do
    [ -n "$branch" ] || continue
    printf 'remove branch    %s  (%s)\n' "$branch" "$reason"
    removals=$((removals + 1))
  done <<EOF
$PLAN_BRANCHES
EOF

  while IFS=$'\t' read -r kind name reason; do
    [ -n "$name" ] || continue
    printf 'keep   %-9s %s  (%s)\n' "$kind" "$name" "$reason"
    keeps=$((keeps + 1))
  done <<EOF
$KEPT
EOF

  printf '\n%d to remove, %d kept.\n' "$removals" "$keeps"
  [ "$removals" -gt 0 ] || return 0
  return 0
}

apply_plan() {
  local path branch reason status=0

  while IFS=$'\t' read -r path branch reason; do
    [ -n "$path" ] || continue
    note "removing worktree $path"
    git worktree remove "$path" || { note "could not remove $path"; status=1; continue; }
    note "deleting branch $branch"
    git branch -D "$branch" >/dev/null || status=1
  done <<EOF
$PLAN_WORKTREES
EOF

  while IFS=$'\t' read -r branch reason; do
    [ -n "$branch" ] || continue
    note "deleting branch $branch"
    git branch -D "$branch" >/dev/null || status=1
  done <<EOF
$PLAN_BRANCHES
EOF

  git worktree prune
  return "$status"
}

cmd_sweep() {
  local apply=0 fetch=1 root main merged_list
  while [ $# -gt 0 ]; do
    case "$1" in
      -y|--yes)   apply=1; shift ;;
      --no-fetch) fetch=0; shift ;;
      -h|--help)  usage ;;
      *)          die "unknown argument: $1" ;;
    esac
  done

  git rev-parse --git-dir >/dev/null 2>&1 || die "not inside a git repository."
  root="$(repo_root)"
  [ -n "$root" ] || die "could not locate the main worktree."
  cd "$root"

  if [ "$fetch" = 1 ] && git remote | grep -qx origin; then
    note "fetching …"
    git fetch --prune origin >/dev/null 2>&1 || note "fetch failed — judging against the refs on disk"
  fi

  main="$(main_ref)" || die "no main or master to judge against."
  merged_list="$(merged_pull_request_branches)"

  build_plan "$main" "$merged_list" "$root"

  if [ -z "$PLAN_WORKTREES$PLAN_BRANCHES" ]; then
    report
    note "nothing to sweep."
    return 0
  fi

  report
  if [ "$apply" = 0 ]; then
    note ""
    note "dry run — nothing was changed. Re-run with --yes to apply."
    return 0
  fi
  apply_plan
}

# --------------------------------------------------------------- selftest ---
ST_FAILURES=0

# The trap runs after cmd_selftest has returned, where a `local` no longer
# exists and `set -u` turns the cleanup itself into the error.
ST_TMP=""
st_cleanup() { [ -n "$ST_TMP" ] && rm -rf "$ST_TMP"; return 0; }

st_assert() {
  if [ "$1" = 0 ]; then
    printf '  ok    %s\n' "$2" >&2
  else
    printf '  FAIL  %s\n' "$2" >&2
    ST_FAILURES=$((ST_FAILURES + 1))
  fi
}

st_branch_exists() { git -C "$1" show-ref --verify --quiet "refs/heads/$2"; }

st_has_worktree() { git -C "$1" worktree list --porcelain | grep -qxF "worktree $2"; }

# Each assertion runs its check inside an `if`, where a non-zero exit is a
# result rather than something for `set -e` to abort the run over — otherwise
# the first failure hides every assertion after it.
st_gone()       { local rc=0; if   st_branch_exists "$1" "$2"; then rc=1; fi; st_assert "$rc" "$3"; }
st_present()    { local rc=0; if ! st_branch_exists "$1" "$2"; then rc=1; fi; st_assert "$rc" "$3"; }
st_wt_gone()    { local rc=0; if   st_has_worktree  "$1" "$2"; then rc=1; fi; st_assert "$rc" "$3"; }
st_wt_present() { local rc=0; if ! st_has_worktree  "$1" "$2"; then rc=1; fi; st_assert "$rc" "$3"; }

# A branch whose content reached main as a different commit — what a squash
# merge leaves behind, and the case `git branch --merged` cannot see.
st_squash_landed() {
  local work="$1" branch="$2" file="$3"
  git -C "$work" switch -c "$branch" main >/dev/null 2>&1
  printf 'landed\n' > "$work/$file"
  git -C "$work" add "$file" >/dev/null
  git -C "$work" commit -qm "work on $branch" >/dev/null
  git -C "$work" switch main >/dev/null 2>&1
  printf 'landed\n' > "$work/$file"
  git -C "$work" add "$file" >/dev/null
  git -C "$work" commit -qm "$branch (squashed)" >/dev/null
  git -C "$work" push -q origin main >/dev/null 2>&1
}

cmd_selftest() {
  local script tmp work origin rc
  script="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
  # pwd -P, because macOS hands back a /var path that git reports as /private/var,
  # and every path assertion below would compare two spellings of the same
  # directory and quietly pass.
  ST_TMP="$(cd "$(mktemp -d)" && pwd -P)"
  tmp="$ST_TMP"
  trap st_cleanup EXIT

  origin="$tmp/origin.git"
  work="$tmp/work"
  git init -q --bare "$origin"
  git init -q -b main "$work"
  git -C "$work" remote add origin "$origin"
  git -C "$work" config user.email sweep@example.com
  git -C "$work" config user.name sweep
  printf 'base\n' > "$work/base.txt"
  git -C "$work" add base.txt >/dev/null
  git -C "$work" commit -qm "base" >/dev/null
  git -C "$work" push -q -u origin main >/dev/null 2>&1

  note "selftest"

  st_squash_landed "$work" landed/squashed squashed.txt
  st_squash_landed "$work" landed/in-worktree worktree.txt
  st_squash_landed "$work" landed/dirty dirty.txt
  st_squash_landed "$work" landed/locked locked.txt

  git -C "$work" branch landed/ancestor main~1
  git -C "$work" switch -q -c open/work main >/dev/null 2>&1
  printf 'unmerged\n' > "$work/open.txt"
  git -C "$work" add open.txt >/dev/null
  git -C "$work" commit -qm "unmerged work" >/dev/null
  git -C "$work" switch -q -c pr/merged main >/dev/null 2>&1
  printf 'merged by pr\n' > "$work/pr.txt"
  git -C "$work" add pr.txt >/dev/null
  git -C "$work" commit -qm "work behind a merged pull request" >/dev/null
  git -C "$work" switch -q -c fresh/start main >/dev/null 2>&1
  git -C "$work" switch -q main >/dev/null 2>&1

  git -C "$work" worktree add -q "$tmp/wt-landed" landed/in-worktree >/dev/null 2>&1
  git -C "$work" worktree add -q "$tmp/wt-dirty" landed/dirty >/dev/null 2>&1
  printf 'scratch\n' > "$tmp/wt-dirty/scratch.txt"
  git -C "$work" worktree add -q "$tmp/wt-open" open/work >/dev/null 2>&1
  git -C "$work" worktree add -q --lock "$tmp/wt-locked" landed/locked >/dev/null 2>&1

  cd "$work"

  rc=0
  SWEEP_MERGED_BRANCHES="pr/merged" "$script" --no-fetch >"$tmp/dry.log" 2>&1 || rc=$?
  st_assert "$rc" "a dry run succeeds"
  [ "$rc" -eq 0 ] || cat "$tmp/dry.log" >&2
  st_present "$work" landed/squashed "a dry run changes nothing"

  rc=0
  SWEEP_MERGED_BRANCHES="pr/merged" "$script" --yes --no-fetch >"$tmp/apply.log" 2>&1 || rc=$?
  st_assert "$rc" "applying the plan succeeds"
  [ "$rc" -eq 0 ] || cat "$tmp/apply.log" >&2

  st_gone    "$work" landed/squashed    "a squash-merged branch goes"
  st_gone    "$work" landed/ancestor    "a branch already in main goes"
  st_gone    "$work" pr/merged          "a branch with a merged pull request goes"
  st_present "$work" open/work          "unmerged work stays"
  st_present "$work" main               "main stays"
  st_present "$work" fresh/start        "a branch with no commits of its own stays"

  st_wt_gone    "$work" "$tmp/wt-landed" "a worktree holding landed work goes"
  st_gone       "$work" landed/in-worktree "its branch goes with it"
  st_wt_present "$work" "$tmp/wt-dirty"  "a worktree with uncommitted changes stays"
  st_present    "$work" landed/dirty     "the branch of a dirty worktree stays with it"
  st_wt_present "$work" "$tmp/wt-locked" "a locked worktree stays"
  st_present    "$work" landed/locked    "the branch of a locked worktree stays with it"
  st_wt_present "$work" "$tmp/wt-open"   "a worktree holding unmerged work stays"

  if [ "$ST_FAILURES" -eq 0 ]; then
    note "selftest passed"
    return 0
  fi
  die "$ST_FAILURES selftest assertion(s) failed"
}

case "${1:-}" in
  --selftest) shift; cmd_selftest "$@" ;;
  -h|--help)  usage ;;
  *)          cmd_sweep "$@" ;;
esac
