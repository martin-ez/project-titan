#!/usr/bin/env bash
# scripts/gate.sh — every check CI can fail a pull request on, run before it does.
#
#   scripts/gate.sh              the whole gate
#   scripts/gate.sh --selftest   check the rules the gate runs on
#
# RUSTFLAGS and RUSTDOCFLAGS carry -D warnings for the whole run, because
# .github/workflows/ci.yml sets them at workflow level and so every job there
# inherits them. A warning that only denies in CI is the drift this exists to
# close, and it costs a rebuild the first time the gate runs over ad-hoc work.
#
# Four of pr.yml's assertions are outside this gate, because a working tree
# cannot answer them: the title's length, its `type(scope): summary` shape,
# the body's wrapping, and the scan for tool attribution in the body. All four
# belong to a request that does not exist while this runs.
# `scripts/check-pr-body.sh -F body.md` answers the wrapping one against a
# drafted body and nothing else; the other three are yours to hold as you
# write them (4.2, 4.4, 4.5). The commit half of the attribution scan is here,
# because commits do exist.
#
# --selftest holds CI to that list rather than trusting it to hold still: it
# reads the workflows and fails on a command CI runs that this gate neither
# runs nor names in `ci_exceptions`. The body's wrapping is the one of the four
# that reaches a workflow as a command, so it is the one entry in that table;
# the other three are inline shell that no extraction can see.
#
# Every check runs even after an earlier one fails, as `!cancelled()` makes
# them in CI, so one run says everything rather than the first thing.
#
# The heartbeat naming each command goes to stderr and every verdict to
# stdout, so `scripts/gate.sh 2>/dev/null` is the report on its own. A missing
# precondition is the exception: it stops the gate before there is a report,
# and says why on stderr.
#
# Exit codes:
#   0  every check passed
#   1  a check failed, or the gate could not run one
#
# Written for bash 3.2 (macOS /bin/bash).

set -euo pipefail

cd "$(dirname "$0")/.."

export RUSTFLAGS="-D warnings"
export RUSTDOCFLAGS="-D warnings"

TAB="$(printf '\t')"

die()  { printf '\n\033[31mFAIL\033[0m  %s\n' "$*" >&2; exit 1; }
note() { printf '%s\n' "$*" >&2; }
pass() { printf '\033[32mok\033[0m    %s\n' "$*"; }

# A failure record is a name and the command that reproduces it, so the summary
# can hand back a single line to run rather than a check to go and find. They
# differ wherever the fix is not the check: `--check` reports the formatting,
# `cargo fmt --all` repairs it.
failures=""

record() {
	local entry="$1$TAB$2"
	if [ -z "$failures" ]; then
		failures="$entry"
	else
		failures="$failures
$entry"
	fi
}

summarise() {
	local failures="$1" count=0 word=checks name cmd

	if [ -n "$failures" ]; then
		count="$(printf '%s\n' "$failures" | wc -l | tr -d ' ')"
		if [ "$count" = 1 ]; then word=check; fi
		printf '\n\033[31mFAIL\033[0m  %s %s failed:\n\n' "$count" "$word"
		while IFS="$TAB" read -r name cmd; do
			[ -n "$name" ] || continue
			printf '      %s\n        %s\n' "$name" "$cmd"
		done <<EOF
$failures
EOF
		printf '\n'
		return 1
	fi

	pass "every check CI can fail a pull request on passed"
	return 0
}

# A command reduced to its shape: every word after the first that names a path,
# an expansion or a count becomes a placeholder. CI writes paths and job counts
# its own way, so comparing shapes lets two invocations agree in their flags and
# differ in their operands, while a flag only one side carries still reads as a
# difference.
command_shapes() {
	awk '
		{
			sub(/^[[:space:]]+/, "")
			sub(/[[:space:]][0-9]*>.*$/, "")
		}
		$1 == "cargo" || $1 ~ /^scripts\// {
			$1 = $1
			for (i = 2; i <= NF; i++)
				if (index($i, "/") || index($i, "$") || $i ~ /^[0-9]+$/)
					$i = "_"
			print
		}
	'
}

# Every command a workflow runs. A line's content — after a leading `- ` and
# `run: `, and one pipeline segment at a time — is a command when it begins
# `cargo ` or `scripts/`, which reads a step inside a `run: |` block as well as
# a one-line `run:`. `uses:`, `sudo apt-get` and `git diff` steps fall out on
# their own, because they begin with something else.
ci_commands() {
	local file
	for file in "$@"; do
		tr '|' '\n' <"$file" | sed -e 's/^[[:space:]]*//' -e 's/^- //' \
			-e 's/^run: //' | command_shapes
	done
}

# The commands the gate runs, read out of the calls that run them. A check the
# gate spells as a shell function has no command line and announces the line
# that reproduces it instead, so a `run_check` call yields both its command and
# that line. Reading the calls, and a bare invocation at the top level or one
# level inside it, is what stops a command named in a comment or in a fixture
# below from accounting for one the gate stopped running.
gate_commands() {
	local file calls
	for file in "$@"; do
		calls="$(grep '^run_check ' "$file" || true)"
		{
			printf '%s\n' "$calls" | sed 's/^run_check "[^"]*" "[^"]*" //'
			printf '%s\n' "$calls" |
				sed -n 's/^run_check "[^"]*" "\([^"]*\)".*/\1/p'
			grep -E "^${TAB}?(cargo |scripts/)" "$file" || true
		} | tr '|' '\n' | command_shapes
	done
}

# Commands CI runs that the gate cannot, each with the reason it is out. A
# working tree holds no pull request, so a check that reads one is a property of
# the request rather than of the tree. The reason sits beside the command as
# data rather than in prose about it, so a reviewer sees what was excluded and
# why in the diff that excludes it.
ci_exceptions() {
	printf '%s\n' \
		"scripts/check-pr-body.sh${TAB}reads a pull request body, which does not exist while the gate runs"
}

# The commands CI runs that the gate neither runs nor excepts. Both sides are
# read out of the files that run them, so a check dropped from the gate stops
# accounting for the step in CI it mirrored.
unaccounted() {
	local ci="$1" gate="$2" exceptions="$3" cmd found=""
	while IFS= read -r cmd; do
		[ -n "$cmd" ] || continue
		if printf '%s\n' "$gate" | grep -qxF "$cmd"; then
			continue
		fi
		if printf '%s\n' "$exceptions" | cut -d"$TAB" -f1 | grep -qxF "$cmd"; then
			continue
		fi
		found="$found$cmd
"
	done <<EOF
$ci
EOF
	printf '%s' "$found"
}

# Named before anything runs rather than six minutes into the gate, and with
# the line that installs the missing piece. Both components ship with a
# toolchain installed through rustup and are missing on one installed another
# way, where every check below would die on an unknown subcommand.
preconditions() {
	cargo fmt --version >/dev/null 2>&1 ||
		die "rustfmt is not installed, so the formatting check cannot run.

        rustup component add rustfmt"

	cargo clippy --version >/dev/null 2>&1 ||
		die "clippy is not installed, so the lint checks cannot run.

        rustup component add clippy"
}

# A check written as a shell function has no command line of its own, so the
# heartbeat and the summary both show the line that reproduces it instead. A
# function name announced as though it were a command is one a reader cannot run.
run_check() {
	local name="$1" fix="$2" out rc=0 shown
	shift 2
	shown="$*"
	if [ "$(type -t "$1")" = function ] && [ -n "$fix" ]; then
		shown="$fix"
	fi
	note "→ $shown"
	out="$("$@" 2>&1)" || rc=$?
	if [ "$rc" = 0 ]; then
		pass "$name"
		return 0
	fi
	printf '\n\033[31mFAIL\033[0m  %s\n\n' "$name"
	printf '%s\n\n' "$out"
	[ -n "$fix" ] || fix="$*"
	record "$name" "$fix"
	return 0
}

# The commit half of CI's hygiene job. The body half reads a pull request that
# does not exist while the gate runs; this reads the commits, which do.
no_coauthors() {
	local base=main merge_base found
	if git rev-parse --verify --quiet origin/main >/dev/null; then
		base=origin/main
	fi
	merge_base="$(git merge-base "$base" HEAD)" || {
		printf '%s and HEAD share no history, so no range of commits could be read.\n' "$base"
		return 1
	}
	found="$(git log --format='%B' "$merge_base..HEAD" |
		grep -inE '^co-authored-by:' || true)"
	[ -z "$found" ] || {
		printf '%s\n\n%s\n' "$found" \
			"AGENTS.md 4.5: the tool that produced a change is not a fact about
the change. Reword the commit, or drop the trailer with a rebase."
		return 1
	}
}

st_is() {
	local want="$1" got="$2" name="$3"
	if [ "$got" = "$want" ]; then
		printf '\033[32mok\033[0m    %s\n' "$name"
	else
		printf '\033[31mFAIL\033[0m  %s (got "%s", wanted "%s")\n' \
			"$name" "$got" "$want"
		st_status=1
	fi
}

st_has() {
	local text="$1" want="$2" name="$3"
	case "$text" in
	*"$want"*) printf '\033[32mok\033[0m    %s\n' "$name" ;;
	*)
		printf '\033[31mFAIL\033[0m  %s (never says "%s")\n' "$name" "$want"
		st_status=1
		;;
	esac
}

st_hasnt() {
	local text="$1" unwanted="$2" name="$3"
	case "$text" in
	*"$unwanted"*)
		printf '\033[31mFAIL\033[0m  %s (says "%s")\n' "$name" "$unwanted"
		st_status=1
		;;
	*) printf '\033[32mok\033[0m    %s\n' "$name" ;;
	esac
}

st_check_passes() { return 0; }

st_check_fails() {
	printf 'the tool said what was wrong\n'
	return 1
}

# The gate has no game to be tested through an App, so it carries its cases
# with it the way the body check does, and they run wherever it can change.
selftest() {
	st_status=0

	local out rc two

	rc=0
	out="$(summarise "")" || rc=$?
	st_is 0 "$rc" "a clean gate exits 0"
	st_has "$out" "every check CI can fail" "a clean gate says the whole gate passed"

	two="clippy${TAB}cargo clippy --all-targets --all-features
documentation${TAB}cargo doc --no-deps --all-features"
	rc=0
	out="$(summarise "$two")" || rc=$?
	st_is 1 "$rc" "a failure exits 1"
	st_has "$out" "2 checks failed" "the summary counts them"
	st_has "$out" "clippy" "the summary names the first failure"
	st_has "$out" "documentation" "the summary names the second"
	st_has "$out" "cargo clippy --all-targets --all-features" \
		"each failure carries its command"
	st_has "$out" "cargo doc --no-deps --all-features" \
		"the command is the one that reproduces it"
	st_hasnt "$out" "passed" "a gate with a failure never says passed"

	rc=0
	out="$(summarise "formatting${TAB}cargo fmt --all")" || rc=$?
	st_has "$out" "1 check failed" "one failure is not called two"
	st_has "$out" "cargo fmt --all" "the fix is offered where it is not the check"

	out="$(run_check "a demo" "" st_check_passes 2>&1)"
	st_has "$out" "a demo" "a passing check is named"
	st_hasnt "$out" "FAIL" "a passing check is not reported as one"

	out="$(run_check "a demo" "the line that reproduces it" st_check_passes 2>&1)"
	st_has "$out" "the line that reproduces it" \
		"a check written as a function announces a line that can be run"
	st_hasnt "$out" "st_check_passes" "the function name is not offered as a command"

	out="$( { failures=""
		run_check "a demo" "" st_check_fails
		summarise "$failures"
	} 2>&1 )" || true
	st_has "$out" "the tool said what was wrong" "a failing check shows what it said"
	st_has "$out" "1 check failed" "a failing check reaches the summary"
	st_has "$out" "        st_check_fails" \
		"a failure with no fix renders the command itself in the summary"

	out="$( { failures=""
		run_check "the first demo" "the first line" st_check_fails
		run_check "the second demo" "the second line" st_check_fails
		summarise "$failures"
	} 2>&1 )" || true
	st_has "$out" "2 checks failed" "two failing checks are recorded as two"
	st_has "$out" "        the first line" "the summary renders the first"
	st_has "$out" "        the second line" "the summary renders the second"

	local workflow
	workflow="$(mktemp "${TMPDIR:-/tmp}/titan-gate.XXXXXX")"
	cat >"$workflow" <<'EOF'
jobs:
  demo:
    steps:
      - uses: actions/checkout@v4
      - run: sudo apt-get update && sudo apt-get install -y libasound2-dev
      - run: cargo test --all-features
      - run: |
          git diff origin/main...HEAD > /tmp/pr.diff
          cargo bench --all-features
EOF

	out="$(ci_commands "$workflow")"
	st_has "$out" "cargo test --all-features" "a one-line run: step is a command"
	st_has "$out" "cargo bench --all-features" \
		"a cargo line after the first of a run: block is one too"
	st_hasnt "$out" "actions/checkout" "a uses: step is not a command"
	st_hasnt "$out" "apt-get" "an apt-get step is not a command"
	st_hasnt "$out" "git diff" "a git diff step is not a command"

	out="$(unaccounted "$out" "$(gate_commands scripts/gate.sh)" "$(ci_exceptions)")"
	st_has "$out" "cargo bench --all-features" \
		"a command the gate neither runs nor excepts is unaccounted"
	st_hasnt "$out" "cargo test" "a command the gate runs is accounted for"
	rm -f "$workflow"

	st_has "$(unaccounted "scripts/check-pr-body.sh" "" "")" \
		"scripts/check-pr-body.sh" \
		"dropping an entry from the exception table unaccounts its command"
	st_is "" "$(unaccounted "scripts/check-pr-body.sh" "" "$(ci_exceptions)")" \
		"the entry that carries a reason is what accounts for it"

	st_is "" "$(unaccounted \
		"$(ci_commands .github/workflows/ci.yml .github/workflows/pr.yml)" \
		"$(gate_commands scripts/gate.sh)" \
		"$(ci_exceptions)")" \
		"every command CI runs is one the gate runs or excepts"

	if [ "$st_status" = 0 ]; then
		printf '\033[32mok\033[0m    every rule the gate runs on holds\n'
	fi
	return "$st_status"
}

while [ $# -gt 0 ]; do
	case "$1" in
	-h | --help)
		sed -n '2,5p' "$0" | cut -c 3-
		exit 0
		;;
	--selftest)
		selftest || exit 1
		exit 0
		;;
	*) die "usage: gate.sh | --selftest" ;;
	esac
done

preconditions

run_check "the gate's own rules" "scripts/gate.sh --selftest" selftest
run_check "house style" "" scripts/check-style.sh
run_check "pull request body rules" "" scripts/check-pr-body.sh --selftest
run_check "no co-authored commits" "git log --format='%B' origin/main..HEAD" no_coauthors
run_check "formatting" "cargo fmt --all" cargo fmt --all -- --check
run_check "clippy" "" cargo clippy --all-targets --all-features
run_check "documentation" "" cargo doc --no-deps --all-features
run_check "tests" "" cargo test --all-features

summarise "$failures"
