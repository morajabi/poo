# Mods

Everything poo knows comes from mods: which folders are build output, which caches are
safe, how worktrees and simulators are found, and how each kind of item is cleaned.
poo's own knowledge ships as the built-in `core` mod, written in the same format you
use. You (or your coding agent) can add rules, override any field of a built-in rule,
disable rules, add scan roots and write custom providers and guards in any language.

The one thing a mod cannot change is the **safety floor** (see the end of this page).

## Where mods live

| Location | Purpose |
|---|---|
| built in (`rules/core/*.toml` in the repo) | the `core` mod, embedded in the binary |
| `~/.config/poo/mods/<id>/` | user and agent mods, loaded in id order |
| `~/.config/poo/config.toml` | roots, project policies, disabled rules, per-rule overrides |

`poo mods new <id>` scaffolds a mod. `poo mods validate` checks every mod and prints
precise errors. `poo rules explain <path>` shows which rule matches a path and why.

## Layout

```
~/.config/poo/mods/team/
  mod.toml          # manifest
  *.toml            # any number of rule files
  bin/              # optional executables used by exec providers and guards
```

`mod.toml`:

```toml
id = "team"
name = "Team rules"
version = "0.1.0"
description = "Extra build folders our monorepo produces"
priority = 10            # higher priority wins when two mods define the same rule id
```

## Buckets

Buckets are the top-level sections you see in the app, the CLI and MCP. They are mod
data too, so you can rename, reorder, recolor, hide, add or merge them.

```toml
[[bucket]]
id = "build"
name = "Build output"
order = 10
color = "#E8833A"          # chart color
symbol = "hammer"          # SF Symbol name used by the app
description = "Compiled output inside your projects and worktrees"
```

Move a rule to another bucket with `bucket = "..."` in an override, or add a new bucket
and point rules at it. `poo buckets` lists, adds, edits and removes buckets from the
command line (`poo buckets set build --name "Builds" --order 1`), which writes to
`config.toml` so built-in defaults stay untouched. Hidden buckets still count toward the
total under "Hidden".

## Rules

Every rule file holds `[[rule]]` tables. A rule with an id that already exists
(for example `rust.target`) **overrides only the fields you set**.

```toml
[[rule]]
id = "rust.target"
name = "Rust target directories"
bucket = "build"           # top-level bucket id (see Buckets)
group = "Rust"             # subsection shown inside the bucket
kind = "artifact"          # artifact | path | provider
names = ["target"]         # directory names (globs allowed) to match while walking roots
parent_any = ["Cargo.toml"]          # parent folder must contain one of these entries
children_any = ["debug", "release", "CACHEDIR.TAG", ".rustc_info.json"]
cost = "rebuild"           # none | rebuild | redownload | data
rebuild = "cargo build"    # shown to users and agents
warm_days = 14             # touched within N days counts as warm (in use)
description = "Compiled Rust output. Rebuilds on the next cargo build."
action = "delete"          # delete | trash | command
```

### Kinds

- `artifact` matches directories found while walking your dev roots and agent worktree
  homes. Matching uses the directory name plus entries of the parent and of the
  directory itself, which poo has already read, so matchers are free.
  Fields: `names`, `parent_any`, `parent_all`, `parent_none`, `children_any`,
  `children_all`, `children_none`, `path_contains`, `path_excludes`.
- `path` matches fixed locations. `paths` take `~`, `$TMPDIR` and globs; each match is
  one item. `each = true` lists the **children** of each match as separate items instead
  (every folder in `~/Library/Caches`). `files = true` includes files among those
  children, `extensions` restricts them, `exclude` skips names (globs), `min_age_days`
  keeps only entries untouched for a while, `min_bytes` drops small ones,
  `contents = true` cleans what is inside but keeps the folder, `system = true` allows a
  location outside your home folder.
  An exact path in one rule beats the same path produced by another rule's `each`, and a
  path claimed by a more specific rule is subtracted from its parent's size.
- `provider` asks a provider for items. Built-in providers: `git-worktrees`,
  `git-ignored`, `xcode-derived-data`, `simulator-devices`, `simulator-runtimes`,
  `docker`, `homebrew`, `versions`, `exec`. Providers take parameters under `params`.
  `versions` lists old versions in `params.path`, keeping the one any `params.active`
  symlink points into and the newest `params.keep` others.

Matching extras for any kind:

- `ignored = true` (artifact rules) only matches when git ignores the folder, so
  generic names like `dist` or `build` never hit committed folders.
- `running_apps = ["Google Chrome"]` makes items hot while one of those processes runs.
- `tier = "review"` forces a tier (for example archives you should look at first).
- `recommend` is advice shown to people and agents.

### Cost and tiers

`cost` says what you pay to get the space back as it was:

| cost | meaning |
|---|---|
| `none` | junk: logs, temp, crash reports, unavailable simulators |
| `rebuild` | regenerated by a build (target, .build, DerivedData) |
| `redownload` | fetched again by a package manager (node_modules, cargo registry) |
| `data` | real data (Docker volumes, model weights you pulled, transcripts); never in plans |

poo combines cost with activity into a tier for each item:

| tier | meaning | in plans |
|---|---|---|
| `junk` | cost `none` | Quick, Not in use, Deep, Emergency |
| `cold` | untouched for longer than `warm_days`, orphaned, or its worktree's commits already landed | Not in use, Deep, Emergency |
| `idle` | nothing uses it now: builds of checkouts other than your `scan.keep_recent_checkouts` newest per repo, untouched for `scan.idle_hours`; or anything untouched for `idle_days` with nobody working there | Not in use, Deep, Emergency |
| `warm` | in active use (recent, or an agent, editor or shell is open in that checkout) | Deep, Emergency |
| `hot` | in use now (a build tool runs there, an app that owns it is open, touched in the last hour) | never |
| `review` | needs a human look (unknown ignored folder, worktree with uncommitted work) | never |
| `protected` | never cleaned without `--danger` (data, protected projects) | never |

### Risks and modes

Every item carries `risks`. Each risk names the mode that may clean it anyway:

| mode | allows |
|---|---|
| standard | items without risks (plans and schedules only ever use this) |
| `--force` | `recent`, `agent-active`, `review`, `whole-checkout`, `dirty` (poo runs `git stash push --include-untracked` on the real changes first, never on build output) |
| `--danger` | `build-running`, `app-running`, `data`, `protected`, `unreachable-commits` (saved to a `poo/<name>` branch first), paths outside your home folder except system trees |

Before removing any worktree poo backs up small ignored files (`.env`, `Local.xcconfig`)
to `~/.local/state/poo/backups/`.

### Tags and stacks

`tags = ["rust", "build-output"]` group rules into packs that can be switched on or off
together: `poo rules disable --tag games`, `poo rules enable --stack android`, also
`--group` and `--bucket`. `stacks = ["rust"]` tells poo which stack a rule belongs to so the
app and CLI show what is relevant to the machine (`poo rules ls --hits` lists rules that
matched something). Rules with `enabled = false` are opt-in packs.

Project policies (`poo projects set <path> active|stale|protected`) move every item of a
project up or down.

### Actions

- `delete` removes the folder (fast: it is renamed into poo's graveyard first and
  erased in the background).
- `trash` moves it to the macOS Trash.
- `command` runs `command = ["xcrun", "simctl", "delete", "{id}"]` where `{path}`,
  `{id}` and `{name}` are replaced with item fields.

## Plans

Recommended plans are data too. Core ships `quick`, `stale`, `deep` and `emergency`.

```toml
[[plan]]
id = "weekly"
name = "Weekly tidy"
order = 15
description = "Junk plus cold build output, never caches"
tiers = ["junk", "cold"]
max_cost = "rebuild"
exclude_buckets = ["caches"]
```

`greedy = true` with `target_free_percent` (or `poo free 30G`) picks the least
regrettable items (safest tier, cheapest cost, longest unused) until the target is met.
Override a built-in plan from config with `[plan_overrides.stale]`.

## Stacks

`[[stack]] id = "rust" markers = ["Cargo.toml"]` tells poo what a project is built with.
Stacks show up in `poo projects` and help agents reason about rebuild costs.

## Schedules

Schedules live in `config.toml` and are managed with `poo schedule` (presets: nightly,
quick, guard, weekly) or the MCP `schedules` tool. A launchd agent
(`com.morajabi.poo.scheduler`) wakes poo every 15 minutes; due schedules rescan, evaluate
their plan with their filters, and clean in standard mode.

```toml
[[schedule]]
id = "nightly"
plan = "stale"
every = "1d"
at = "03:30"
exclude_buckets = ["ai"]
exclude_tags = ["browsers"]
max_bytes = "200G"

[[schedule]]
id = "guard"
plan = "emergency"
every = "30m"
when_free_below = "10%"
target_free = "20%"
```

## Exec providers

An exec provider is any executable that prints JSON. Agents use this to teach poo about
tools it does not know.

```toml
[[rule]]
id = "team.big-fixtures"
name = "Downloaded test fixtures"
bucket = "build"
group = "Fixtures"
kind = "provider"
provider = "exec"
params.command = ["bin/list-fixtures.sh"]
cost = "redownload"
```

The command prints:

```json
{"items": [{"path": "/Users/me/dev/app/fixtures/cache", "name": "fixtures cache",
            "bytes": 123456, "last_used": 1731600000, "cost": "redownload"}]}
```

`bytes` is optional (poo measures when missing).

## Guards

Guards veto cleaning. They run right before poo deletes anything.

```toml
[[guard]]
id = "team.running-file"
description = "Skip projects with an active build listed in .running"
kind = "file-nonempty"     # file-nonempty | process | exec
file = "{project}/.running"
```

`process` guards list process names (`names = ["xcodebuild", "cargo"]`) and block items
whose project contains the working directory of such a process. `exec` guards get the
item as JSON on stdin and exit non-zero to block, printing a reason.

## Roots and worktree homes

```toml
[[root]]
path = "~/work"            # extra dev root to walk

[[worktree_home]]
path = "~/.codex/worktrees"
agent = "codex"
```

## Config file

```toml
# ~/.config/poo/config.toml
roots = ["~/dev", "~/Developer"]
disabled_rules = ["system.downloads-installers"]
disabled_mods = []

[rule_overrides."rust.target"]
warm_days = 7
bucket = "rust"

[[bucket]]                  # add or override buckets from config too
id = "rust"
name = "Rust"
order = 5

[bucket_overrides.system]
hidden = true

[projects."/Users/me/dev/old-app"]
policy = "stale"           # active | stale | protected | auto

[agents]
auto_clean_max_tier = "cold"   # MCP may clean junk and cold without asking
```

## Safety floor

These checks run for every item and no mod or config can turn them off:

- Only paths inside your home folder, your temp folder, or rule-declared system cache
  folders, never the home folder itself, a dev root, or a parent of one.
- Never a symlink, never a path through `.ssh`, `.gnupg`, `Keychains`, `Mobile Documents`
  or `.Trash` of another volume, never anything named like `.env`.
- Project artifacts must not contain a `.git` entry; whole worktrees are only removed
  through `git worktree remove`, which refuses uncommitted changes.
- The item is re-checked against its rule immediately before cleaning.
- Items whose project hosts a running build process are skipped.
