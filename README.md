# poo

**Find and safely free the disk space your dev work eats.** A CLI, a macOS menu bar
app, and an MCP server plus skill so your coding agent can do it for you.

Most cleaners stop at global caches, so you clear things you rebuild two days later.
On a developer Mac, the real growth is elsewhere:

- every worktree (yours, Claude's, Codex's, Conductor's) gets its own Rust `target`,
  SwiftPM `.build` per package, Xcode DerivedData, `node_modules` and `dist`;
  that is 10–25 GB per feature, duplicated per checkout;
- agents pass `-derivedDataPath` and `CARGO_TARGET_DIR` into folders nobody cleans;
- merged worktrees, old simulators and runtimes, superseded toolchains and agent CLI
  versions pile up.

poo knows these workflows. It walks your dev folders and agent worktree homes, maps
DerivedData back to its checkout, checks each worktree for uncommitted work and merge
state, and gives every item a **tier** that says how much regret cleaning it causes.
It also covers the usual cleaner territory (app, browser and system caches, logs, temp,
installers, Docker, Homebrew) with safety rules learned from tools like Mole.

```
$ poo
poo  27.9 GB free of 494 GB (94% used)   scanned just now in 17.2s

  Build output               95.9 GB  ██████████░░░░░░░░░░░░  39 items
  Xcode & simulators          217 GB  ██████████████████████  39 items, 8.53 GB stale
  Worktrees                  2.63 GB  ░░░░░░░░░░░░░░░░░░░░░░  13 items
  ...
Plans
  quick         152 MB    31 items  Logs, temp files, crash reports and broken simulators.
  stale        9.01 GB    44 items  Adds build output, worktrees and caches nobody touched for weeks...
  deep          134 GB   100 items  Adds recent builds and caches. Expect rebuilds and redownloads.
  emergency    49.7 GB    45 items  Frees the least regrettable items first until 15% is free.

$ poo worktrees
~/dev/app  194 GB across 12 checkouts
    40.7 GB clean   warm                 ~/dev/app-blocks   feature-x
    28.2 GB dirty   protected            ~/dev/app-typo     (detached)
     143 MB clean   cold      codex      ~/.codex/worktrees/review/app   codex/review
```

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/morajabi/poo/main/install.sh | sh
# with the menu bar app:
curl -fsSL https://raw.githubusercontent.com/morajabi/poo/main/install.sh | POO_APP=1 sh
```

Then `poo setup` to pick your dev folders. Homebrew formula and cask are coming
(`packaging/homebrew`). poo updates itself (`poo update`); the app updates itself and
its bundled CLI.

## Tiers, plans and modes

| tier | meaning | cleaned by plans |
|---|---|---|
| junk | nothing to rebuild: logs, temp, crash reports, broken simulators | quick, not in use, deep, emergency |
| cold | unused for days, orphaned (project gone), or its worktree's commits already landed | not in use, deep, emergency |
| idle | nothing uses it now: builds of checkouts other than your 2 newest per repo, untouched for hours | not in use, deep, emergency |
| warm | in active use: recent, or an agent, editor or shell is open in that checkout | deep, emergency |
| hot | a build or the owning app is running, or touched in the last hour | never |
| review | needs a human look: unknown ignored folders, worktrees with uncommitted work | never |
| protected | data, Docker volumes, projects you protected | never |

Mark projects `active`, `stale` or `protected` (`poo projects set`) and plans follow. Tune
`scan.keep_recent_checkouts` and `scan.idle_hours` to how many checkouts you build at once.

Everything that is not risk-free carries a note. `--include-in-use` also cleans things in
use: used in the last hour, checkouts with an agent open, worktrees with uncommitted work
(stashed first, never including build output). `--dangerously-include-protected` also takes
running builds, data, kept projects and detached commits (saved to a `poo/` branch first).
Plans and schedules only ever clean safe items.

## Backups you can see

Before removing a worktree poo copies its small ignored files (`.env`, `Local.xcconfig`)
into a dated folder, stashes uncommitted work, and saves detached commits to a branch.
`poo backups` lists them, `restore` puts files back, `prune` removes them. A 2 GB budget and
a 90-day limit keep the folder from growing; both live in `[backup]` in `config.toml`.

## Agent hooks (opt-in)

`poo hooks install all` makes Claude Code and Codex clean a worktree's build output when an
agent finishes there and its work already landed in main. Nothing is installed by default.

## Routine cleanups

```sh
poo schedule add nightly    # 03:30 every night: clean what nothing is using
poo schedule add guard      # low-disk guard: below 20 GB free, clean what's not in use until 50 GB
poo schedule ls
```

A launchd agent wakes poo every 15 minutes; due schedules rescan first, respect bucket and
tag filters, notify you, and log to `poo schedule log`.

## CLI

```
poo                         overview
poo ls [bucket] [--tier cold] [--project PATH] [--min 1G] [-q text]
poo show <id|path>          why it is there, what it costs to rebuild, what is inside
poo plan stale              preview a plan
poo clean <id>... | --plan stale [--dry-run] [--yes]
poo free 30G                least-regret items until 30 GB are free
poo projects | poo worktrees
poo buckets | poo rules | poo mods | poo roots | poo config
poo catalog                 everything poo knows
poo rules explain <path>    which rule matches and why
poo history
```

Output is JSON when piped (`--text` forces tables). Errors are
`{"error":{"kind","message","hint"}}` with stable exit codes: 2 usage, 3 not found,
5 needs approval, 6 partial failure, 7 update failed, 8 invalid mods.

## For coding agents

```sh
poo integration install all    # skill + MCP for Claude Code, Codex, Cursor, OpenCode, Gemini, Windsurf
npx skills add morajabi/poo --skill poo -g   # skill only, via the open skills CLI
```

The MCP server (`poo mcp`) exposes `overview`, `list_items`, `inspect`, `plan`,
`clean` (dry run by default), `projects`, `catalog`, `explain_path`, `configure`,
`setup_brief` and `history`. Agents may clean up to the tier you allow
(`poo config set agents.auto_clean_max_tier cold`); anything above needs your approval.

Ask your agent: *"Set up poo for me: run `poo setup --agent` and configure it for my
workflows."* The brief gives it your disk, projects, duplication across worktrees,
everything poo knows, and the questions worth asking you.

## Mods

Everything poo knows (hundreds of rules across Xcode, Swift, Rust, Node, Python, JVM,
Go, Ruby, PHP, Elixir, Haskell, .NET, Flutter, game engines, Docker, AI agents and
editors, browsers and macOS) is a built-in mod written in the same TOML format you can
use. Buckets, plans, stacks, worktree homes and guards are mod data too. Add rules,
override any field, disable rules, add buckets, write providers or guards in any
language: see [docs/mods.md](docs/mods.md) or `poo mods docs`.

## Safety

poo never deletes outside your home and temp folders (or a rule's declared system
path), never symlinks or anything under `.ssh`, `.gnupg`, Keychains or iCloud, never a
project artifact that contains `.git`, and removes worktrees only through
`git worktree remove`, which refuses uncommitted changes and keeps the branch. Right
before deleting, it re-checks the item against its rule and skips projects where a
build tool or coding agent is running. Deletes rename into a graveyard first, so a
folder disappears atomically. Every clean is logged (`poo history`).

## Build from source

```sh
cargo build --release                 # CLI
./scripts/build-app.sh                # dist/Poo.app with the CLI bundled
```

MIT licensed.
