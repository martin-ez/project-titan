# ![Banner](assets/docs/repo_banner.png)

# Project Titan

Terraform a planet via traffic and resource management.

This video game is heavily inspired by Satisfactory and Cities Skylines. The player has to set up production chains, but
instead of conveyor belts, it relies on rovers running on roads, leading to traffic management issues.

## Building

```sh
cargo run     # play what is there
cargo test
```

## Contributing

[AGENTS.md](AGENTS.md) is how this project is built — the design invariants, the
documentation and testing rules, the branching model, and how work is tracked.
It applies to agents and humans alike. Run `scripts/gate.sh` before opening a
pull request, and take work from the issue tracker through `scripts/track.sh`.
