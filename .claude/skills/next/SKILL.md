---
name: next
description: Take the next piece of work off the tracker — pick a ready issue, claim it, branch, and run the test-first loop to a draft pull request. Use at the start of a session, or when asked what to work on next or what is ready.
---

# Take the next task

The sequence only. AGENTS.md is already in context; its rules on test-first work,
pull requests and exit codes apply here without being restated.

## 1. Orient

```sh
git switch main && git pull --ff-only
scripts/track.sh doctor
```

**This skill only ever starts fresh work.** It does not resume, finish or
release an issue that is already claimed — `ready` lists nothing that is, so
picking from it cannot collide. Held work belongs to whoever holds it: the
user's other sessions run against this same tracker, and a claim with no branch
or commits is far more likely to be a live parallel agent than an abandoned
session of yours. Never run `mine` here to look for work to pick back up.

To finish or hand back something already claimed, the user will say so; that is
`show <n>` and the branch, or `release <n>`, not this skill.

## 2. Pick

```sh
scripts/track.sh ready
```

The list is sorted by how much each row unblocks, so the top row frees the most
work. Take the top three rows and put them to the user with `AskUserQuestion` —
one option per issue, labelled `#74`, described by its title, size and what it
unblocks, top row first and marked `(Recommended)`. Their pick is the one to
claim; claim nothing before they answer.

Offer fewer than three if `ready` lists fewer. Anything under `SPLIT:` is
`size:l` and unclaimable — leave it out of the options and say so, or use the
`refine` skill. If the user named an issue, still run `ready`: take it without
asking if it is there, and say why rather than forcing it if it is not.

## 3. Take

```sh
scripts/track.sh start 74
```

The issue the user picked. Claims first, then branches onto `feat/74-…`. Exit 2
means someone else took it in the meantime: say so and ask again with the rows
that are left, never `--force`.

## 4. Read it whole, then plan

```sh
scripts/track.sh show 74
```

Restate the scope in a sentence, and name any design invariant it touches. If it
is really several tasks, split it with `add --parent` instead of doing all of it.

Then plan: the tests you will write, the shape of the code under them, and how
that shape holds the invariants you named. A gameplay change is planned as a
plugin — which components and resources it owns, what runs on the fixed tick and
what only draws. `ExitPlanMode` puts it to the user — no test and no code until
they approve it. A rejected plan is cheaper than a reviewed pull request.

## 5. Build

The approved plan, test first, in a `#[cfg(test)] mod tests` beside the code,
driving a headless `App` where the thing under test is a system. Watch it fail
for the right reason. Then `scripts/gate.sh`, which is the whole gate from
AGENTS.md — not a subset of it you picked. If the build shows the plan was
wrong, say so and re-plan rather than quietly taking another route.

## 6. Stop at a draft pull request

Link the issue in the body on its own line — `Tracks #74`, never `Closes`.

Write the body with each paragraph on a single line, however long, and check it
with `scripts/check-pr-body.sh -F body.md` before you open anything. The body is
the squashed commit message; wrapping it at 80 columns the way the rest of this
repository is wrapped puts those breaks into `main`'s history for good.

Then `scripts/track.sh submit 74`, which records that the work is built and
waiting on a person. Without it the issue is indistinguishable from one still
being written, and the next thing to read the tracker — `plan`, `mine`, or a
supervisor — has no way to tell that there is nothing left to do here.

**Do not close the issue and do not merge.** Merging settles it: the `tracking`
workflow runs `done` for every issue the body tracks, and `release` if the pull
request is closed unmerged. Closing it by hand earlier has the tracker assert
work is in `main` when it is not.

Abandoning without a pull request: `scripts/track.sh release 74`.
