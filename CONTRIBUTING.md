# Contributing to poo

poo only knows the folders someone taught it about, and no single Mac uses every stack.
The most useful contribution is a rule: a few lines of TOML that tell poo what a folder is,
what removing it costs, and how it comes back.

## Share a rule from your Mac

```bash
poo suggest                   # big folders no rule explains yet
poo suggest --write mine      # draft a rule for each into the mod "mine" (disabled)
poo rules explain <path>      # check what a rule matches, and why
poo mods lint                 # the same safety lint CI runs
poo mods submit mine          # preview, then open a pull request
```

`poo mods submit` never sends anything without asking. It:

1. lints the rules (the checks below) and stops on errors;
2. removes personal details: your home path becomes `~`, and your user, machine and project
   names and email addresses are replaced with placeholders;
3. shows you the exact files and pull request text and saves them under
   `~/.local/state/poo/submissions/<name>/`;
4. with your yes, opens the pull request through `gh` (a fork, a branch and a PR in your
   account), or without `gh` prints prefilled GitHub links you can open yourself.

The evidence it shares is the number of matching folders and their total size rounded to two
digits, the tool versions behind the rule's stacks, and matcher fixtures that list only the
file names your rule mentions. `--no-evidence` leaves all of that out. Coding agents run
`poo mods submit <mod> --json` to get the preview and pass `--yes` only after you approved it.

No `gh`, no Mac at hand? Open a [Suggest a rule](../../issues/new?template=suggest-a-rule.yml)
issue instead.

## Writing a safe rule

Rules live in `rules/core/*.toml`, one file per topic (`web.toml` and `dev-web.toml` for
JavaScript, `python.toml`, `apple.toml`, ...). The format is in [docs/mods.md](docs/mods.md).

- **Paths stay under `~` or `$TMPDIR`**, and never directly in your home folder: point at
  the folders inside (`~/.tool/cache`, or `~/.tool/*`). System caches need `system = true`
  and a reason in the PR.
- **Generic folder names need proof.** `build`, `dist`, `out`, `lib`, `site`, `vendor`,
  `deps`, `ios`, `android` and friends need a marker next to them (`parent_any`, like
  `Cargo.toml`) and proof inside them (`children_any`) or `ignored = true`, so a committed
  folder never matches.
- **`names = ["*"]`** needs `children_all` (a fingerprint like `CMakeCache.txt`) and
  `children_none` with the files only sources have, so a source folder never matches.
- **Pick the honest cost.** `none` for logs and temp, `rebuild` for build output,
  `redownload` for package caches, `data` for anything a person made or cannot fetch again
  (shown, never planned).
- **Toolchains, SDKs and models get `tier = "review"`.** They are large and often pinned.
- **Apps that keep a cache open** go in `running_apps`. Locations macOS protects (browser
  profiles, Safari, Mail, Messages, other apps' containers) get `tags = ["needs-permission"]`.
- **Every rule has** `name`, `bucket`, `group`, `kind`, `cost`, `stacks`, a one-line
  `description` and, unless the cost is `none`, a `rebuild` hint.

## Fixtures

Matcher fixtures in `tests/rule-fixtures/*.toml` describe folders a rule must and must not
match. A new rule with a generic folder name needs at least one of each:

```toml
[[fixture]]
rule = "docs.mkdocs-site"
name = "site"
parent = ["mkdocs.yml", "docs"]          # entries next to the folder
children = ["index.html", "search"]      # entries inside it
matches = true

[[fixture]]
rule = "docs.mkdocs-site"
name = "site"
parent = ["package.json", "src"]
children = ["index.html"]
matches = false
note = "a web project's site/ source folder"
```

## Checks

```bash
cargo run -- mods validate --text
cargo run -- mods lint --core --base origin/main --text
cargo test
```

CI runs the same on every pull request that touches `rules/` (`.github/workflows/mods.yml`).
To try rule edits without rebuilding, point an installed poo at your checkout:
`POO_CORE_DIR=$PWD/rules/core poo rules explain <path>`.

## Code

- `src/` is the Rust engine and CLI, `app/` the SwiftUI app (it only talks to the CLI).
- Test against real disks read-only (`POO_HOME=$(mktemp -d) cargo run --release -- scan`);
  deletion tests build a sandbox in `$TMPDIR` and pass `--root`.
- Commit messages are lowercase and scoped: `rules: ...`, `cli: ...`, `app: ...`.
- The safety floor (`src/clean.rs::floor_check`) is never loosened from a mod or config.
