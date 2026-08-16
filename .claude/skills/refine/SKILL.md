---
name: refine
description: Sharpen the issues at the front of the dependency graph before anyone claims them — split oversized ones, add missing dependencies, turn thin descriptions into testable criteria. Use when grooming the backlog or splitting a size:l epic.
---

# Refine the frontier

Work the front of the graph only: ready now, or one hop behind. A deep node's
shape depends on decisions nobody has made yet, so refining it is guessing.

Conversational — propose, ask, then apply. Nothing here is applied silently.

## 1. Look

```sh
scripts/track.sh ready
scripts/track.sh graph --json
```

Three signals worth acting on:

- **`CYCLE:`** — issues blocking each other, unreachable from any root. Fix these
  first; they strand real work.
- **`SPLIT:`** — `size:l`, so unclaimable. Usually why the queue looks emptier
  than it should.
- **a leaf with no parent and empty `unblocks`** — more often a missing edge than
  genuinely independent work.

## 2. Assess

`show <n>`, then report what fails rather than a general impression.

- **One session?** If the body describes two verbs on two nouns, it is two issues.
- **Testable done condition?** "A rover entering a full segment waits" is;
  "traffic works" is not.
- **Invariant named?** Anything touching goods movement, the fixed tick, hex
  coordinates or which plugin owns what should say which invariant constrains
  it.
- **Balance claim without a measurement?** Turn it into a chain run for a fixed
  number of ticks with a number asserted at the end, rather than a sentence.
- **Edges complete?** Ask what this work reaches for on day one. If that is an
  open issue and not a blocker, the edge is missing.

## 3. Offer a closer look

The assessment is a proposal, and the user knows things the tracker does not —
why a design went the way it did, what they mean to try next. Put it to them with
`AskUserQuestion` before applying any of it: one option per issue you would
change, at most three and the ones where their input would change the shape, plus
one for applying the plan as it stands. `Other` is where they name an issue you
did not list.

On a pick, `show` that issue and work it through with them. What they add is
scope, so it leaves as sub-issues, edges or a `note` — never as a promise to
remember it. Then ask again: context on one issue tends to surface context on the
next. Stop when they choose to apply.

## 4. Apply, additively

```sh
scripts/track.sh add -t '<part>' --parent 65 --area sim --kind feat --size s
scripts/track.sh dep 88 --needs 85
scripts/track.sh note 65 -m 'Split into #102-#104; one lane only for now.'
```

There is no body-edit command, and that is the right constraint: a change of
scope becomes a comment plus new sub-issues, so the issue's history stays
readable instead of being rewritten under a reviewer who already read it.

## 5. Confirm

`scripts/track.sh ready` — children should appear, `SPLIT:` should be gone, any
cycle resolved. If nothing moved, the refinement did not land; say so rather than
reporting success.

Stop when the frontier is clean. A distant epic left as a stub is the correct
state for it, not an omission.
