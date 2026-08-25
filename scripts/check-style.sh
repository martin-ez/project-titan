#!/usr/bin/env bash
#
# Mechanical checks for the rules in AGENTS.md that rustc and clippy cannot
# express. Run by CI and safe to run locally at any time.
#
#   scripts/check-style.sh

set -euo pipefail

cd "$(dirname "$0")/.."

status=0

# AGENTS.md 1.1, in the units the rule is written in. Changing a number here
# changes the rule, so change the rule too.
ITEM_DOC_BUDGET=8
MODULE_DOC_BUDGET=12

report() {
	printf '\n\033[31mFAIL\033[0m  %s\n' "$1"
	printf '%s\n' "$2" | sed 's/^/      /'
	status=1
}

pass() {
	printf '\033[32mok\033[0m    %s\n' "$1"
}

roots=()
for dir in src tests benches examples; do
	[ -d "$dir" ] && roots+=("$dir")
done

if [ ${#roots[@]} -gt 0 ]; then
	# --- AGENTS.md 1.1 — a doc comment has a budget ----------------------
	#
	# The budget is in prose lines, because that is what a reader pays: the
	# blank separators and the fenced examples are not what makes a doc
	# comment unreadable, and an example is a test (1.5), so charging for it
	# would price the wrong thing.
	#
	# Two budgets rather than one. An item doc answers what a signature does
	# and what it promises; a module doc is the only place this project puts
	# the shape of a folder, since 1.6 leaves it nowhere else to go.
	found=$(git ls-files '*.rs' | tr '\n' '\0' | xargs -0 awk '
		function report() {
			if (kind != "" && count > budget)
				printf "%s:%d: %d prose lines in a %s comment, budget %d\n",
					file, start, count, kind, budget
			kind = ""
		}
		FILENAME != seen { report(); seen = FILENAME }
		{
			if ($0 !~ /^[ \t]*\/\/[\/!]/) { report(); next }
			k = ($0 ~ /^[ \t]*\/\/!/) ? "//!" : "///"
			if (k != kind) {
				report()
				kind = k; file = FILENAME; start = FNR
				count = 0; fenced = 0
				budget = (k == "//!") ? '"$MODULE_DOC_BUDGET"' : '"$ITEM_DOC_BUDGET"'
			}
			text = $0
			sub(/^[ \t]*\/\/[\/!]/, "", text)
			gsub(/^[ \t]+|[ \t]+$/, "", text)
			if (text ~ /^```/) { fenced = !fenced; next }
			if (fenced || text == "") next
			count++
		}
		END { report() }
	' || true)
	if [ -n "$found" ]; then
		report "doc comment over budget (AGENTS.md 1.1)" "$found
A doc comment says what the item does and what it promises, then stops. Past
that budget it is describing the implementation, and the implementation is
already there to read. Cut it to the contract, or make the code carry what the
prose was carrying — a name, or a function whose signature says it instead."
	else
		pass "no doc comment over budget (1.1)"
	fi

	# --- AGENTS.md 1.4 — no inline comments ------------------------------
	#
	# /// and //! are documentation and are allowed. // is not.
	#
	# Both exclusions have to skip past grep's own "file:line:" prefix. A
	# leading // comment turns that prefix into the literal substring
	# ":8://", so a naive URL filter silently discards every comment at the
	# start of a line — which is most of them.
	found=$(grep -rnE --include='*.rs' '//' "${roots[@]}" 2>/dev/null |
		grep -vE '^[^:]+:[0-9]+:[[:space:]]*(///|//!)' |
		grep -vE '^[^:]+:[0-9]+:.*://' || true)
	if [ -n "$found" ]; then
		report "inline comment (AGENTS.md 1.4)" "$found
Delete it, or make the code say it: name the value, extract the step into a
function whose name is the sentence you were about to write. Where the fact is
genuinely not derivable from the code — a constant taken from a paper, a value
tuned against play testing — it belongs in the doc comment of the item that
uses it, so that a reader of \`cargo doc\` sees it too."
	else
		pass "no inline comments (1.4)"
	fi

	# --- AGENTS.md 3.4 — hidden work sits behind a named feature ----------
	found=$(grep -rnE --include='*.rs' '\b(TODO|FIXME|XXX|HACK)\b' "${roots[@]}" 2>/dev/null || true)
	if [ -n "$found" ]; then
		report "TODO marker (AGENTS.md 3.4)" "$found
Incomplete work goes behind a Cargo feature and into a GitHub issue, where it
is visible and can block other work. A marker in a source file is neither."
	else
		pass "no TODO markers (3.4)"
	fi

	# --- AGENTS.md invariant 2 — the tick reads no frame ------------------
	#
	# The three failure modes a test can catch — two runs disagreeing, two
	# frame rates disagreeing, a junction served in storage order — are
	# caught by the determinism trace in src/rover.rs. The fourth cannot be:
	# a world position is derived through `sin` and `cos`, which agree with
	# themselves on the machine that wrote the test and differ across
	# platforms and standard libraries. A tick that decides anything from
	# one gives different traffic elsewhere while every test here passes, so
	# the line is held here instead: the simulation stays in arc-length
	# space, and a `Vec3` is presentation.
	#
	# Invariant 4 puts a system in the same file as the plugin registering
	# it, so the systems on the tick are read out of the FixedUpdate
	# registrations in each file and matched against the functions it
	# defines. A body is the lines from its `fn` to the `}` at that
	# indentation, which is what rustfmt guarantees and nothing else here
	# needs to parse.
	found=$(git ls-files '*.rs' | tr '\n' '\0' | xargs -0 awk '
		function countchar(s, c,   i, k) {
			k = 0
			for (i = 1; i <= length(s); i++)
				if (substr(s, i, 1) == c) k++
			return k
		}
		function scan(   i, j, k, name, rest, line, region, depth, start, indent, closes) {
			if (n == 0) return
			delete defined
			for (i = 1; i <= n; i++) {
				if (L[i] !~ /^[ \t]*fn [a-z_]/) continue
				name = L[i]
				sub(/^[ \t]*fn /, "", name)
				sub(/[^A-Za-z0-9_].*$/, "", name)
				defined[name] = i
			}
			delete ticking
			for (i = 1; i <= n; i++) {
				j = index(L[i], "add_systems(")
				if (j == 0) continue
				rest = substr(L[i], j + 12)
				if (rest ~ /^[ \t]*$/ && i < n) rest = L[i + 1]
				if (rest !~ /^[ \t]*FixedUpdate([,)]|$)/) continue
				region = ""
				depth = 0
				for (k = i; k <= n; k++) {
					line = (k == i) ? substr(L[i], j + 11) : L[k]
					region = region " " line
					depth += countchar(line, "(") - countchar(line, ")")
					if (depth <= 0) break
				}
				while (match(region, /[A-Za-z_][A-Za-z0-9_]*/)) {
					name = substr(region, RSTART, RLENGTH)
					region = substr(region, RSTART + RLENGTH)
					if (name in defined) ticking[name] = 1
				}
			}
			for (name in ticking) {
				start = defined[name]
				match(L[start], /^[ \t]*/)
				indent = substr(L[start], 1, RLENGTH)
				closes = indent "}"
				for (k = start; k <= n; k++) {
					if (L[k] ~ /delta_secs|elapsed_secs|world_position|\.translation|Transform|Time<Virtual>|Time<Real>/)
						printf "%s:%d: %s runs on the tick and reads what only a frame knows\n",
							file, k, name
					if (k > start && L[k] == closes) break
				}
			}
		}
		FILENAME != file { scan(); file = FILENAME; n = 0 }
		{ L[++n] = $0 }
		END { scan() }
	' 2>&1 || true)
	if [ -n "$found" ]; then
		report "a system on the tick reading a frame (AGENTS.md invariant 2)" "$found
A tick measures in ticks and in distances along an arc. Real time belongs to
presentation, and so does a world position: it is derived through \`sin\` and
\`cos\`, which are not identical across platforms, so a decision taken from one
gives different traffic on another machine while every test here still passes.
Move the reading into an \`Update\` system, or decide it from arc length."
	else
		pass "no system on the tick reads a frame (invariant 2)"
	fi
fi

# --- AGENTS.md 1.6 — the only prose outside the code is a folder README ------
#
# Four root files are exempt because they are the project's contract rather
# than documentation of it, and GitHub looks for them by name. Templates under
# .github/ are configuration, and so are agent skills under .claude/ — a skill
# is a procedure that fails when it goes stale, not prose that is believed.
#
# docs/ is exempt because what lives there is game design — the production
# tree, recipes, costs, progression. That is the content the game is made of,
# not prose about how the code works, and it has no item to hang a doc comment
# on. Design that has become code documents itself in the code; design that
# has not is still design.
found=$(git ls-files '*.md' '*.markdown' |
	grep -vE '^(README|AGENTS|CLAUDE|CONTRIBUTING)\.md$' |
	grep -vE '^\.github/' |
	grep -vE '^\.claude/' |
	grep -vE '^docs/' |
	grep -vE '(^|/)README\.md$' || true)
if [ -n "$found" ]; then
	report "documentation outside the code that is not a folder README (AGENTS.md 1.6)" "$found
Rename it to README.md in the folder it describes, or move it into a doc
comment on the code it explains. A folder gets one README; it does not get a
library."
else
	pass "prose outside the code is folder READMEs only (1.6)"
fi

# A folder README describes the folder it sits in. A folder holding nothing but
# prose is a documentation tree wearing a README's name, which is the thing 1.6
# exists to prevent.
found=$(git ls-files '*/README.md' | { grep -v '^docs/' || true; } | while read -r readme; do
	siblings=$(git ls-files "${readme%/README.md}" |
		grep -vE '\.(md|markdown)$' | head -1)
	if [ -z "$siblings" ]; then
		printf '%s\n' "$readme"
	fi
done)
if [ -n "$found" ]; then
	report "a folder containing only documentation (AGENTS.md 1.6)" "$found
This folder holds no code, so its README describes something other than itself —
which makes it a document, not a folder README. Move what is still true into the
code or the top-level README, and the rest into issues."
else
	pass "every folder README sits beside code (1.6)"
fi

# --- AGENTS.md 1.7 — documentation describes what exists ---------------------
#
# AGENTS.md and the pull request template state this rule and so must be able
# to name the thing they forbid. CLAUDE.md is a symlink to AGENTS.md, and GNU
# grep follows symlinks named on the command line where BSD grep does not — so
# leaving it out here passes on macOS and fails on Linux.
#
# An issue number on its own is not the signal. A doc comment naming the issue
# that owns a case the code deliberately does not handle is describing today's
# boundary, and src/ui.rs does exactly that. What turns a reference into prose
# about the future is the promise beside it: a verb saying the issue changes
# this code, or a clause holding until it lands. Those are what `anchored`
# matches, so a bare `[#193]` passes and `#216 replaces this` does not.
phrases='coming soon|not yet implemented|will be implemented|planned for a|in a future release|once implemented|for now, this is a placeholder'
anchored='#[0-9]+ *(replaces|supersedes|removes|rewrites|will|lands\b)|(replaced|superseded|removed|rewritten|handled|fixed) (by|in) #[0-9]+|\b(until|once|when) #[0-9]+'
targets=$(git ls-files '*.md' '*.rs' |
	grep -vE '^(AGENTS\.md|CLAUDE\.md|\.github/pull_request_template\.md)$' |
	grep -vE '^docs/' || true)
if [ -n "$targets" ]; then
	found=$(printf '%s\n' "$targets" | tr '\n' '\0' |
		xargs -0 grep -rniE "$phrases|$anchored" 2>/dev/null || true)
	if [ -n "$found" ]; then
		report "documentation of something that does not exist (AGENTS.md 1.7)" "$found
Describe what the code does today. Work that has not happened belongs in a
GitHub issue, where it is queryable and can block other work — not in prose
that no test and no compiler will ever contradict."
	else
		pass "no documentation of unbuilt features (1.7)"
	fi
fi

# --- scripts/track.sh — a refusal fails one assertion, not the run -----------
#
# `die` calls `exit`, and `||` does not catch an exit, so a tracker command run
# bare on a path expected to succeed ends the whole selftest instead of
# recording one FAIL. Every assertion after it is lost, and the output blames
# whichever line the run happened to reach. A subshell contains the exit;
# catching the status keeps `set -e` from ending the run in its place.
#
# Both halves are needed, and each is invisible without the other: a bare call
# dies past `||`, and a contained one whose status nobody reads is a non-zero
# exit under `set -euo pipefail`. A pipeline element is already a subshell, and
# `|| die` is the preflight, which ends a run that has created nothing yet.
found=$(awk '
	/^cmd_selftest\(\) \{/ { inside = 1 }
	inside && /^\}/        { inside = 0 }
	!inside                { next }
	/^[ \t]*#/             { next }
	{
		if (buf == "") start = FNR
		line = $0
		sub(/[ \t]*\\$/, " ", line)
		buf = buf line
		if ($0 ~ /\\$/) next
		logical = buf
		buf = ""
		caught = (logical ~ /\|\|/)
		rest = logical
		bad = 0
		while (match(rest, /cmd_[a-z_]+/)) {
			name = substr(rest, RSTART, RLENGTH)
			pre  = substr(rest, 1, RSTART - 1)
			post = substr(rest, RSTART + RLENGTH)
			rest = post
			if (name == "cmd_selftest") continue
			tail = post
			gsub(/\|\|/, "XX", tail)
			contained = (pre ~ /\(/) || (tail ~ /\|/)
			if (!contained || !caught) bad = 1
		}
		if (bad) printf "%d: %s\n", start, logical
	}
' scripts/track.sh)
if [ -n "$found" ]; then
	report "a tracker call in cmd_selftest that a refusal would end the run on" "$found
Run it in a subshell — ( cmd_x … ) for a step, \$( cmd_x … ) for a capture — and
catch its status with || rc=\$?, then hand that to st_assert. A refusal is one
FAIL with the rest of the run still executing, not an exit that loses it."
else
	pass "a refusal fails one selftest assertion rather than ending the run"
fi

# --- scripts/track.sh — a selftest run has an issue budget --------------------
#
# Every throwaway issue a run files is a paced create, a paced delete at
# cleanup, and a read on the way in to check the parent it names is open — so
# the count is a real share of what a run costs in wall clock and in the
# account's hourly GraphQL budget. It creeps up one section at a time: a
# section that needs something to point at files its own rather than borrowing
# a fixture the run already has, and nothing says otherwise.
#
# The number is the rule. Changing it here changes what a run is allowed to
# cost, so change it only with a section that genuinely needs a shape none of
# the others hold.
SELFTEST_ISSUE_BUDGET=20
found=$(awk -v budget="$SELFTEST_ISSUE_BUDGET" '
	/^cmd_selftest\(\) \{/ { inside = 1 }
	inside && /^\}/         { inside = 0 }
	!inside                  { next }
	/^[ \t]*#/               { next }
	/^[ \t]*st_add(_foreign)? / { filed++ }
	END {
		if (filed > budget)
			printf "%d issues filed, and the budget is %d\n", filed, budget
	}
' scripts/track.sh)
if [ -n "$found" ]; then
	report "cmd_selftest files more throwaway issues than its budget" "$found
Borrow a fixture the run has already built rather than filing another. A
section that claims or closes one has to hand it back the way it found it, or
take its own — a borrowed issue left mutated fails a section further down for a
reason nothing there can explain."
else
	pass "a selftest run files no more issues than its budget"
fi

exit $status
