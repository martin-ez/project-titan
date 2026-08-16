#!/usr/bin/env bash
#
# AGENTS.md 4.7 — a paragraph in a pull request body is one line. Reads the
# body on stdin, or from a file, and fails on the lines that wrap.
#
#   gh pr view --json body -q .body | scripts/check-pr-body.sh
#   scripts/check-pr-body.sh -F body.md
#   scripts/check-pr-body.sh --selftest

set -euo pipefail

die() { printf '\033[31m%s\033[0m\n' "$1" >&2; exit 2; }

# A line that continues the one above it is a wrap. Anything that opens a block
# of its own starts a line rather than continuing one, and a heading, a rule, a
# table row and raw HTML end theirs on the same line, so nothing after them
# continues anything either. A block quote is missing from that second list on
# purpose: markdown continues one lazily, which is exactly the wrap being
# looked for. A line ending in two spaces is markdown for "break here", and is
# taken at its word.
wrapped_lines() {
	awk '
		function opens_block(s) {
			return s ~ /^[[:space:]]*(#|>|\||<)/ ||
			       s ~ /^[[:space:]]*([-*+]|[0-9]+[.)])[[:space:]]/ ||
			       s ~ /^[[:space:]]*(-[[:space:]]*-|\*[[:space:]]*\*|_[[:space:]]*_)[-*_[:space:]]*$/
		}
		function ends_block(s) {
			return s ~ /^[[:space:]]*(#|\||<)/ ||
			       s ~ /^[[:space:]]*(-[[:space:]]*-|\*[[:space:]]*\*|_[[:space:]]*_)[-*_[:space:]]*$/
		}
		{
			sub(/\r$/, "")
			if (comment) {
				if ($0 ~ /-->/) comment = 0
				carries = 0; next
			}
			if ($0 ~ /^[[:space:]]*(```|~~~)/) { fenced = !fenced; carries = 0; next }
			if (fenced) { carries = 0; next }
			if ($0 ~ /<!--/ && $0 !~ /-->/) { comment = 1; carries = 0; next }
			if ($0 ~ /^[[:space:]]*$/) { carries = 0; next }
			if (carries && !opens_block($0)) printf "%d: %s\n", FNR, $0
			carries = !ends_block($0) && $0 !~ /  $/
		}
	'
}

check() {
	local body="$1" found
	found="$(printf '%s\n' "$body" | wrapped_lines)"
	[ -n "$found" ] || return 0
	printf '\n\033[31mFAIL\033[0m  %s\n' \
		"a hard-wrapped paragraph in the pull request body (AGENTS.md 4.7)"
	printf '%s\n' "$found" | sed 's/^/      /'
	printf '%s\n' "
GitHub keeps every newline in a body, so these breaks reach main's history in
the squashed commit message, and the renderer wraps what is left of the lines
again. Join each paragraph onto one line and leave the width to the reader.
End a line with two spaces where the break itself is the point." |
		sed 's/^/      /'
	return 1
}

st_case() {
	local want="$1" name="$2" body rc=0
	body="$(cat)"
	check "$body" >/dev/null || rc=$?
	if [ "$rc" = "$want" ]; then
		printf '\033[32mok\033[0m    %s\n' "$name"
	else
		printf '\033[31mFAIL\033[0m  %s (exit %s, wanted %s)\n' "$name" "$rc" "$want"
		st_status=1
	fi
}

selftest() {
	st_status=0

	st_case 0 "a paragraph on one line" <<-'EOF'
		A single line that runs on for as long as the sentence needs it to, and stops.

		### Changes

		Another line, just as long, sitting under a heading of its own after a blank.

		Tracks #328
	EOF

	st_case 1 "a wrapped paragraph" <<-'EOF'
		A single line that runs on for as long as the sentence
		needs it to, and stops.
	EOF

	st_case 0 "bullets on one line each" <<-'EOF'
		### Changes

		- The first bullet, which is long enough that an agent would have wrapped it.
		- The second bullet.
		- The third.
	EOF

	st_case 1 "a wrapped bullet" <<-'EOF'
		### Changes

		- The first bullet, which is long enough that an agent
		  wrapped it.
	EOF

	st_case 0 "a heading above its paragraph" <<-'EOF'
		### Changes
		The paragraph that follows the heading without a blank line between them.
	EOF

	st_case 0 "fenced code keeps its breaks" <<-'EOF'
		The command is:

		```sh
		scripts/track.sh ready
		scripts/track.sh start 7
		```
	EOF

	st_case 0 "an html comment keeps its breaks" <<-'EOF'
		<!-- One or two paragraphs of context: the problem this solves, and why now.
		     Link the issue on its own line: Tracks #N — several numbers on that line
		     are all settled. -->

		### Changes
	EOF

	st_case 0 "a table keeps its rows" <<-'EOF'
		| host | grant |
		| --- | --- |
		| macos | hosted |
	EOF

	st_case 0 "an explicit two-space break" < <(printf 'The first line, broken because the break is the point.  \nThe second line.\n')

	st_case 0 "an explicit break the event payload carried as CRLF" < <(printf 'The first line, broken because the break is the point.  \r\nThe second line.\r\n')

	st_case 1 "a wrapped paragraph the event payload carried as CRLF" < <(printf 'A single line that runs on for as long as the sentence\r\nneeds it to, and stops.\r\n')

	st_case 1 "a lazily continued block quote" <<-'EOF'
		> A quoted line that an agent wrapped rather than
		leaving on one line.
	EOF

	[ "$st_status" = 0 ] && printf '\n\033[32mok\033[0m    every case behaved\n'
	return "$st_status"
}

body=""
mode=check
while [ $# -gt 0 ]; do
	case "$1" in
	-F | --file)
		[ $# -ge 2 ] || die "-F needs a file"
		body="$(cat "$2")"
		shift 2
		;;
	--selftest)
		mode=selftest
		shift
		;;
	-h | --help)
		printf '%s\n' "usage: check-pr-body.sh [-F FILE] | --selftest"
		exit 0
		;;
	*) die "usage: check-pr-body.sh [-F FILE] | --selftest" ;;
	esac
done

if [ "$mode" = selftest ]; then
	selftest
	exit $?
fi

[ -n "$body" ] || body="$(cat)"
check "$body"
