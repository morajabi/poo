# Working on poo

- `src/` is the Rust engine and CLI (`poo`), including the MCP server (`src/mcp.rs`).
  `app/` is the SwiftUI menu bar and window app; it only talks to the CLI.
- All knowledge lives in `rules/core/*.toml` (the built-in mod). Prefer adding or
  fixing rules there over special cases in code. New behavior that rules cannot
  express becomes a provider (`src/providers/`) that rules opt into.
- The safety floor is in `src/clean.rs::floor_check`. Never weaken it from a mod or
  config path.
- The JSON shapes in `src/model.rs` are a contract with the app (`app/Sources/Poo/Model.swift`)
  and agents. Additive changes only; bump `SCHEMA` for breaking ones.
- Test against real disks read-only: `POO_HOME=$(mktemp -d) cargo run --release -- scan`.
  For deletion tests, build a sandbox in `$TMPDIR` and pass `--root`.
- `cargo test`, `cargo run -- mods validate --text`, `swift build --package-path app`.
- Commit messages are lowercase and scoped: `rules: ...`, `cli: ...`, `app: ...`.
