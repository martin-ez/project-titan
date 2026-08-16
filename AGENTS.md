# AGENTS.md

How this project is built. It applies to agents and humans alike.

Project Titan is a planet-terraforming builder: you place extractors and
factories, wire them into production chains, and everything moving between them
is a rover driving on a road you built. That is what it is for, and it is what
makes it a different game from the ones it takes after — a chain that balances
on paper still jams at the junction. What it does today is the ground it stands
on: `cargo run` opens a pan-orbit camera over a hex tile grid, routes the
keyboard and mouse through one input plugin, and switches the player between
selecting, editing roads and editing buildings.

So a feature extends a substrate rather than starting beside one. `MapPlugin`
owns the grid and what a tile is, `PlayerInput` is how a key or a click becomes
an action and `PlayerAction` is what says which tool is holding it, `Initialize`
and `NeedsInitialization` are how a spawned entity gets its mesh and its place
in the world, and `Destroy` and `DestroyOnStateChange` are how it leaves. What
is ready to pick up is in the issue tracker, not here.

## Commands

```sh
cargo run --features dev  # play what is there, linking Bevy dynamically
cargo test                # while you work
scripts/gate.sh           # before you push
```

`dev` is the fast iteration loop and nothing else: it ships Bevy as a shared
library, so a build relinks in a fraction of the time. Nothing sits behind it
but that, which is why the gate and CI never enable it — a second lint pass
over a different linking strategy checks no code, and costs a full dylib link
to find that out. A release build must not need `libbevy_dylib` beside it.

The gate is every check CI can fail a pull request on, in the configuration CI
uses: formatting, clippy over all targets, `cargo doc` and the tests under
`-D warnings`, the house-style check, and the script selftests. It keeps going
after a failure and reports them all, so one run says everything. A check that
is missing from it is a bug in the gate, not a command to run beside it.

Three of CI's assertions are not in it, because a working tree cannot answer
them: the title's length, its `type(scope): summary` shape, and the scan for
tool attribution in the body all belong to a request that does not exist while
the gate runs. Hold them yourself as you write them (4.2, 4.4, 4.5).
`scripts/check-pr-body.sh -F body.md` covers a fourth, checking a drafted body
for hard wrapping — and only that.

## Design invariants

Load-bearing. A change that breaks one is wrong even if it compiles, passes
tests, and looks tidier.

1. **Traffic is the game, not a delivery detail.** Everything a building
   receives arrives on a rover that drove a road to get there. No instant
   transfer between connected buildings, no global resource pool a factory draws
   from, no belt-shaped abstraction that moves goods without occupying road.
   Every one of those makes the game easier to build and removes the problem it
   is about.

2. **The simulation runs on fixed ticks; presentation runs on frames.**
   Gameplay state changes in `FixedUpdate` and reads the tick, never
   `delta_secs()`. Smoothing, interpolation and camera easing run in `Update`
   and change nothing a rover can observe. This is what makes a jam reproducible
   — the same inputs give the same traffic, on any machine and at any frame rate
   — and it is the only reason a simulation test can assert anything at all.

3. **Hex coordinates are the truth; a world position is derived.** A tile, a
   building and a road segment are located by integer grid coordinates, and
   `Vec3` comes out of a function that converts one. Storing the float and
   rounding it back means neighbours stop being exact, and adjacency is what the
   whole map is made of.

4. **A subsystem is a Bevy plugin, and it owns itself.** Its components,
   resources, states and system registration live together and nothing outside
   registers a system into it. Presentation derives from simulation state and
   never the reverse: a mesh reads the world, a rover never reads its transform
   back to decide where it is.

Corollary: **it has to hold at fleet scale.** Thousands of rovers on the map is
the target, so a per-tick system does not allocate per entity, and does not scan
every entity to find the few it wants. Check before adding a dependency that it
builds on every platform the game targets.

---

## 1. Documentation

Documentation is a first-class citizen here, not a step at the end.

**1.1 A doc comment has a budget: eight lines of prose on an item, twelve on a
module.** Say the thing and stop. Prose that restates a signature in English is
worse than nothing: it has to be maintained, and it will go stale.

Blank lines and fenced examples are free — an example is a test (1.5). What is
left is what a reader pays to reach the signature they came for, and eight lines
buys what the item does and what it promises. Wanting more is the signal that
the prose is carrying something the code should: a name, a type, a function
whose signature says it instead. A module doc gets more because 1.6 leaves the
shape of a folder nowhere else to live. `scripts/check-style.sh` counts it.

**1.2 Prefer in-code documentation over documents.** A doc comment is reviewed in
the same diff as the code it describes and cannot drift unnoticed; a standalone
file can. If it explains how something works, it is a doc comment.

**1.3 Anything another module can reach has a doc comment, and nothing else
does.** A plugin, a component, a resource, a state, a public function — each is
something another part of the game builds against, so each says what it is for.
A helper private to its module is not part of any contract, and a doc comment on
one is clutter that outlives the code.

**1.4 Inline comments are not allowed.** Name the value, or extract the step into
a function whose name is the sentence you were going to write.

> One exception in substance, not in form: a constant that came from somewhere
> outside the code — a figure from a paper, a value settled by play testing.
> That provenance is not derivable from the code, so put it in the doc comment
> of the item that uses it — never in a `//`.

**1.5 Tests are documentation.** A reader should learn what a system does from
its tests. Name them as sentences — `rover_waits_when_the_segment_is_full`, not
`test_traffic_2` — and prefer several small tests stating one fact each.

**1.6 The only documentation outside the code is a per-folder `README.md`,**
describing that folder. No `docs/architecture.md`, no decision-record directory,
no folder holding nothing but prose. Documentation trees rot from the leaves
inward, where nothing links.

Exempt, being the project's contract rather than documentation of it: `README.md`,
`AGENTS.md`, `CONTRIBUTING.md`, and templates under `.github/`. `CLAUDE.md` is a
symlink to this file.

Also exempt, being configuration rather than prose about the code: `.claude/`.
A skill there is an executable procedure an agent follows, closer to
`scripts/track.sh` than to a document — it goes stale the way a script does,
by failing, rather than the way a document does, by being believed.

Exempt for a third reason: `docs/`, which holds game design — the production
tree, recipes, costs, progression. That is the material the game is made of
rather than prose about how the code works, and there is no item to hang it on.
Design that has become code documents itself in the code; design that has not
is still design, and 1.7 does not reach it.

**1.7 Never document a feature that does not exist.** No "coming soon", no
roadmap, no doc comment on a stub describing what it will become. Unbuilt work
belongs in a GitHub issue, where it is queryable, can block other work, and gets
closed when it lands. Aspirational prose does none of that: no test contradicts
it, and it becomes a lie the moment the plan changes.

A `README.md` may state what the game is *for* and the constraints it is built
under; it may not describe something a player cannot do today. There are no
exemptions — not for a founding brief, a design document, or a plan. Import what
is still true into the code, the `README.md` or `docs/`, and put the rest in
issues.

## 2. Development

**2. All code is developed test-first.** Write the test, watch it fail for the
right reason, then write the code that makes it pass.

**2.1 Tests live beside the code they test,** in a `#[cfg(test)] mod tests` at
the bottom of the file. This is a binary crate: a test under `tests/` compiles
as an external consumer and has nothing to import, so there is no version of
this rule that puts them elsewhere.

The compiler therefore cannot stop a test reaching a private item, which makes
it yours to hold: **test what a system does to the world, not the helper it
called on the way.** A test that names an internal function is a test that will
fail the next time that function is refactored, having told nobody anything
about the game.

**2.2 Make them go red, then green.** A test that has never failed has never
demonstrated that it can. Nothing in a diff reveals the order, so this one rests
on you — and a test written afterwards to mirror an implementation is the one
that passes when the implementation is wrong.

**2.3 A system is tested through a headless `App`.** Build one with
`MinimalPlugins`, add the plugin under test, drive it with `app.update()` or by
advancing the fixed timestep, then assert against the `World`. Nothing needs a
window: a system that cannot be tested without one is doing rendering and
gameplay at once, which invariant 4 already forbids.

**A balance claim needs a measurement.** "The refinery keeps up with two
extractors" is not reviewable; a test that runs the chain for a fixed number of
ticks and asserts what came out is.

## 3. Trunk-based development

**3.1 Branches are short-lived** — hours or a day. If a change cannot be finished
that fast it is several changes: split it and merge the first.

**3.2 Merge small and often.** A large pull request gets approved rather than
read.

**3.3 `main` is always green.** Never commit to `main`, never force-push, never
merge your own pull request. A change that breaks `main` is reverted first and
diagnosed second. A rejected push means find another route, not try harder.

**3.4 Incomplete work sits behind a Cargo feature,** named for the capability
rather than the ticket. This is what makes 3.1 and 3.2 possible: unfinished work
merges safely because the plugin is never added to the `App`. `TODO` and `FIXME`
markers are rejected — incomplete work goes behind a flag and into an issue,
where it is visible and can block other work.

**3.5 Sweep up after a merge with `scripts/sweep.sh`.** It removes the local
branches and `.claude/worktrees/` entries whose work is in `main`, and refuses
anything it cannot show has landed. Run it bare to see the plan, `--yes` to
apply it. A squash merge leaves the branch with commits that are not ancestors
of `main`, so `git branch --merged` is not the check to reach for — and an
upstream marked `gone` says the remote branch was deleted, not that the work
was merged.

## 4. Pull requests

**4.1 Be concise and descriptive.** Give the reader what they need to trust the
change, and nothing else.

**4.2 The title is the summary.** It must be clear from the title alone what the
change does; it is what lands in `main`'s history after a squash merge. Use
`type(scope): summary`.

**4.3 The body says what changed and why, not how.** Do not restate the diff,
walk through the implementation, or explain a function a reader can open. Do not
add a verification or test section — CI reports what passed, and prose repeating
it is a claim rather than evidence. The shape is context, then `### Changes`; a
further section is allowed, but should be rare.

**4.4 Titles are at most 50 characters.** If the change will not fit, it is
usually two changes.

**4.5 Never add a co-author.** No `Co-authored-by:` trailer on any commit, no tool
attribution in any body. Agents especially: the tool that produced a change is
not a fact about the change.

**4.6 Link the issue with `Tracks #N`, on a line of its own.** Merging then runs
`done` for every number on that line, and closing the pull request unmerged runs
`release`. Never a keyword GitHub acts on itself — `Closes`, `Fixes` — which
closes the issue behind `track.sh`'s back, leaves `wip` set, and binds to only
the first of several numbers.

**4.7 A paragraph in the body is one line.** Do not wrap it at 72 or 80 columns
the way every file in this repository is wrapped: a file is read in a diff, and
a body is laid out by a renderer. GitHub keeps each newline as a break, so a
wrapped body reaches `main`'s history in the squashed commit message as ragged
short lines that nothing will re-flow, and gets wrapped again on top of that.
One line per paragraph and one per bullet; two trailing spaces where the break
itself is the point. `scripts/check-pr-body.sh` says so before CI does.

## Task tracking

**GitHub Issues is the single source of truth.** Not a markdown TODO list, not a
second tracker — dual tracking is the main way a project like this loses track of
itself.

**Reach it through `scripts/track.sh`, never `gh` directly.**

```sh
scripts/track.sh ready            # what can be started right now
scripts/track.sh show 7           # one issue in full, before you write any code
scripts/track.sh start 7          # claims it, then branches onto it; exit 2 = taken
scripts/track.sh mine             # claims under this checkout's id, yours or not
scripts/track.sh find pathfind    # match titles, open and closed, before filing
scripts/track.sh submit 7         # built; now waiting on a human merge
scripts/track.sh done 7 -m "..."  # closes it, prints what that unblocked
scripts/track.sh plan             # the epic chain in order, the current ones opened
scripts/track.sh --help           # refs, blocked, claim, release, reopen, add,
                                  # dep, note, graph, labels-init, doctor, selftest
```

**Say when the draft goes up.** `submit <n>` is what separates work waiting on a
person from work an agent is still writing, and those want opposite responses.
The claim stays either way — finished work must not be offered back to `ready`
for a second agent to build again — so nothing downstream moves, and `show`,
`plan`, `mine` and `doctor` stop reading a queued merge as a session that
stalled. Merging or closing the pull request clears it.

Take the top row of `ready`; it sorts by how much each item unblocks, so the top
row is the one that frees the most work. **Claim it before you write any code** —
`start <n>` claims and branches in one step, and an issue nobody has claimed is
an issue another agent will take. Any non-zero exit other than 2 is fatal —
surface stderr and stop. Release anything you will not finish.

> This is a correctness rule, not a preference. The legacy search index — raw
> GraphQL `search(type: ISSUE)`, or REST without `advanced_search=true` —
> silently ignores `is:blocked` and returns blocked issues as ready, with a 200
> and no error. The script derives readiness from each issue's `blockedBy`
> payload instead, which makes that failure unreachable rather than merely
> documented, and read-your-writes consistent where the index lags by seconds.

It also spaces writes against the 80/minute and 500/hour account limits, and
holds a lock across both the read and the write in `claim`, so two agents cannot
take the same issue.

Issue types and fields are organisation-only, so metadata is labels: `area:`,
`kind:` and `size:`, all three required by `add`. The areas are `sim` for
traffic, rovers and production, `world` for the grid, roads and buildings, `ui`
for the HUD, tools, input and camera, `render` for meshes and shaders, and
`infra` for build and tooling. `ready` offers only what `claim` will accept — a
`size:l` issue is listed under `SPLIT:` instead, and is split with
`add --parent <n>`.

**Filing new work.** `find` first: a duplicate check that cannot see closed
issues is the one that lets a closed issue be filed again. Then `add`, with all
three labels **and its dependency edges** — `ready` sorts by how much each issue
unblocks, so one filed with no `--blocked-by` or `--blocking` sorts last and
stays invisible. Filing is also what to do instead of a `TODO` marker (3.4): file
it, wire it, and go back to the task in hand.

`--parent` is required, because readiness is inherited through it: work filed
without one is gated by nothing and is startable ahead of the whole chain it
belongs to. `add` refuses that and prints the chain, marking which epics are
startable, so the work goes where it belongs rather than where it can start
soonest. The parent must also be open — a closed one gates nothing either —
and `add`, `dep --child` and `dep --parent` all refuse it. An epic takes
`--blocked-by` instead, naming the epic it comes off: the chain is the order,
so one that comes off nothing is a second root, and nothing gates it or the
work filed under it. Only the first epic in an empty tracker comes off
nothing.

The `next`, `epic`, `refine` and `file` skills in `.claude/skills/` carry the
full procedure for taking one issue, running a whole epic, grooming and filing
work.

## Working agreement

- **One task per session, one pull request per task.** Scope creep is the most
  expensive thing an agent can do here.
- **Start work with `start <n>`**, which claims the issue and branches from
  `main` onto it. Open pull requests as drafts.
- **New dependencies need a reason**: what it does, why not hand-rolled, its
  licence, and that it builds on every platform the game targets. A Bevy plugin
  crate also has to track the Bevy version this project is on.
- **No panicking path in a system that runs per frame.** Setup is fine; a system
  is not. `single()` and `unwrap()` take the whole game down mid-play, so reach
  for the fallible form and handle the empty case.
- **Binary assets go through Git LFS.** `.gitattributes` routes `assets/**`
  there; a mesh or a texture committed around it bloats every clone forever.
- **Match the surrounding code.** Its naming and idiom beat personal preference.
- **Report honestly.** If tests fail, say so and paste the output. If part of the
  task was skipped, say which and why.
- **State assumptions rather than blocking.** Pick the reading a careful
  colleague would, write it down in the pull request, and keep going.

## Commits

Conventional Commits, imperative mood, subject at most 50 characters, no
trailers:

```
feat(sim): queue rovers behind a full segment
fix(world): keep tile neighbours exact at the rim
```

The body is optional. Use it for a decision a future reader would otherwise have
to reconstruct, not to describe the diff.
