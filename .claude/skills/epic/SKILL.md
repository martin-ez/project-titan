---
name: epic
description: Run one epic's ready issues in parallel — read the whole set, batch the ones that can genuinely run at once, dispatch a subagent each, review their plans side by side, and send every finished pull request to an independent reviewer. Use when asked to work an epic rather than a single issue.
---

# Run an epic

`next` is one issue to one pull request, and that is the right unit for an agent.
This is the unit above it: a set of issues that share a subsystem, an invariant
and often a file. AGENTS.md is already in context and applies throughout.

**You supervise. You do not build.** The supervisor edits no file, closes no
issue and merges nothing. Everything that writes code is a subagent holding one
issue and opening one pull request, which is what keeps *one task per session*
true of everything that writes, and keeps `3.3` intact because the supervisor
never pushes.

## 1. Orient

```sh
git switch main && git pull --ff-only
scripts/track.sh doctor
scripts/track.sh plan
```

`plan` groups open work by epic and gives every child a stance — `ready`,
`claimed`, `review` or `waiting`. Take the epic the user named. With none named,
offer the epics `plan` prints with nothing in `waits`.

An epic with no `ready` child is not work for this skill. Say what it waits on
and stop.

## 2. Read the whole set, not one issue

```sh
scripts/track.sh show 271        # the epic
scripts/track.sh show 341        # every ready child
```

Then read the code each one touches, and group them: which plugin, which
component, which invariant.

**This is the judgement the tracker cannot make.** A dependency edge records
logical order and nothing else. Two issues can both be `ready` and both rewrite
the same system, and no edge will ever say so. Ready children that reach for the
same plugin belong in different batches whatever the graph says — and in a Bevy
codebase the collision is often quieter than a shared file: two agents adding
systems to the same schedule, or two claiming the same component name.

## 3. Choose the batch

At most three, and fewer whenever fewer are disjoint. The cap is configurable;
take the number the user gives.

Three is the default because the gate is sized for it. Bevy links a large binary
per build, so three gates at once is already most of a laptop — and a machine
under that load reports slow tests rather than wrong ones, which is the failure
that wastes an agent's session. Where the machine is small, batch two.

Report the batch, and name what you held back and why. An exclusion nobody sees
reads as an omission.

**Dispatch only what `plan` calls `ready`.** Out of scope on purpose, so it is
not re-derived every run: stacking on unmerged work. Readiness comes from closed
issues, so a draft frees nothing, and dispatching on top of one is the supervisor
overriding the tracker. The repository is squash-only, so every merge in a stack
rebases its descendants, and that cost lands on the person reviewing. Stop
instead, hand back what is waiting, and run this again after a merge — when the
tracker says what is ready rather than the supervisor asserting it.

## 4. Dispatch, phase one: plans only

One subagent per issue, `model: opus` named explicitly on every dispatch here and
in step 7. Inheriting the supervisor's model makes a batch depend on which model
the session happened to start on.

Each agent's contract:

- `scripts/track.sh start <n>`, then **its own worktree** on that branch —
  `git worktree add .claude/worktrees/<n>-<slug> feat/<n>-…`. `start` branches the
  shared checkout, and three agents in one checkout move each other's HEAD.
- Read the issue with `show`, read the code it touches.
- **Return the plan as the result**: the tests it will write, the shape of the
  code under them, the invariants it holds and how. Write nothing — no test, no
  source, no commit.
- Stop there.

A subagent cannot stop for a person, so plan approval cannot live inside it. It
leaves as a return value instead.

## 5. Review the plans side by side

The phase that pays for the whole skill, and it has two stages.

**You review first.** Compare the plans against one another rather than one at a
time. Per plan: test-first, tests beside the code and driven through a headless
`App`, the invariant named, scope inside its own issue. Across the set: the same
system registered twice, the same helper invented twice, one invariant held two
different ways, a plan assuming another's unmerged work.

**Then the person approves, once, for the whole batch.** You do not have the last
word. `next` puts every plan to a person before a test exists, and dispatching
three issues at once must not quietly delete that gate — least of all when you
chose the batch, and would otherwise be marking your own grouping. Put up the
plans, the conflicts you found, and what you folded in. Comparing plans against
one another is worth more than approving them one at a time, which is the point
of running the epic rather than the issue.

Only then resume each agent with its corrections. A plan that will not fit one
session is an issue that needs splitting: `release <n>`, then `add --parent`.

## 6. Build

Each agent runs its corrected plan — test first, red then green, then the full
gate from AGENTS.md, then a draft pull request with `Tracks #<n>` on a line of
its own, then `scripts/track.sh submit <n>`.

They run at once and they share one machine. An agent reporting a test that
timed out rather than failed is reporting load, not a bug: it says so and runs
that check again alone rather than writing code against it.

## 7. Send each pull request to someone who did not write it

A fresh subagent per finished pull request, with no authorship stake, given the
issue, AGENTS.md and the diff. It judges against the `Done when`, the
documentation and testing rules, and the four design invariants; it returns
findings and changes nothing. The agent that wrote the branch applies them.

## 8. Stop, and hand back what waits on a person

One list: every pull request, the issue it tracks, its state, and what each is
waiting on. Release anything left unbuilt — `scripts/track.sh release <n>`.

**Close nothing and merge nothing.** Merging runs `done` for every number a body
tracks, and that is the person's call, not the supervisor's.
