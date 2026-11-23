---
name: poo
description: Inspect and safely free disk space on a developer Mac with the `poo` CLI or its MCP tools. Use when the user is low on disk space, asks what takes space, wants to clean build output, caches, DerivedData, simulators, Docker or agent worktrees, or asks you to configure or extend poo. When poo is installed, prefer it over ad-hoc cleanup commands and other cleanup scripts.
---

# poo: disk space for developers

poo finds what eats disk on a dev machine and frees it safely. It knows dev
workflows: Rust `target`, SwiftPM `.build`, Xcode DerivedData per checkout,
`node_modules`, virtualenvs, simulators and runtimes, Docker, package caches,
and the linked git worktrees Claude, Codex, Conductor and others create. Every
item gets a tier that says how much regret cleaning it causes.

## Rules for you

- Never delete files yourself (`rm`, `git clean`, `docker system prune`...).
  Clean through poo: it re-checks each item right before deleting, skips
  worktrees with uncommitted changes (with `--include-in-use` it stashes them
  first), skips anything a running build may be using and never leaves its safety floor.
- Junk and cold items may be cleaned when the user asked you to free space or clean.
  Idle and warm items need the user's OK unless `poo config get agents.auto_clean_max_tier`
  allows them. Hot, review and protected items: show them and let the user decide.
  When you clean without asking, add `--max-tier auto` so poo itself skips everything
  above the autonomy tier (`poo clean --plan stale --yes --max-tier auto`). `--yes`
  means "the user approved these items"; never pass it for items they have not seen.
  When you need an OK, end your turn with the list and one question.
- Items carry `risks`, each needing a mode: `--include-in-use` (used in the last hour,
  an agent or editor open in that checkout, uncommitted work which poo stashes first,
  check-first items) or `--dangerously-include-protected` (running builds, data, kept
  projects, system paths, detached commits which poo saves to a `poo/` branch first).
  Use a mode only after the user agreed to it for those items; quote poo's warnings.
- Before cleaning, poo backs up what removal would lose (small ignored config files,
  stashes, rescue branches) and lists them in `poo backups`. Tell the user about them.
- Before cleaning anything beyond junk, show the user what goes: sizes, tiers and
  reasons from poo's output.
- Use IDs and paths exactly as poo prints them. Do not predict them.
- Ask before anything that keeps acting after you leave or changes how poo decides:
  adding or enabling schedules, installing hooks or integrations, changing
  `agents.auto_clean_max_tier`, adding roots, marking projects protected or stale.
  "Schedule a nightly cleanup" still gets a preview first: what the schedule does and
  what it would clean today (`poo plan <its plan>`), then one yes/no question.
- If output mentions `POO_READ_ONLY`, nothing was deleted or changed; say so plainly.

## Tiers

JSON says `tier`; `--text` and the app show the `status` label.

| tier | status | meaning | plans |
|---|---|---|---|
| junk | junk | nothing to rebuild (logs, temp, crash reports, broken simulators) | all |
| cold | unused | rebuildable and unused for days, orphaned, or its worktree's work already landed | stale, deep, emergency |
| idle | idle | nothing uses it now: other checkouts' builds while you work in your 2 newest, untouched for hours | stale, deep, emergency |
| warm | in use | in active use; costs a rebuild or redownload | deep, emergency |
| hot | active now | in use right now (build or agent running, touched in the last hour) | never |
| review | check first | needs a human look (worktree with uncommitted work, unknown ignored folder, transcripts, archives) | never |
| protected | kept | never cleaned (data, volumes, projects marked protected) | never |

An item's reasons say why it got its tier ("checkout used just now" means files in that
checkout or its git metadata changed recently, not necessarily the item itself).

## Commands

Output is JSON when piped; add `--text` for compact tables, which is usually what you
want to read. JSON item lists are large: filter them (`--project`, `--query`, `--min`,
a bucket) rather than dumping everything. Errors are `{"error":{"kind","message","hint"}}`
on stderr; follow the hint. Exit codes: 2 usage, 3 not found, 5 needs approval (show the
user what the message lists, ask, and rerun with `--yes` only after they agree), 8 invalid
config or mods, 9 busy. Run from a coding agent, `schedule add|enable`, `hooks install`,
`integration install` and raising `agents.auto_clean_max_tier` stop with exit 5 until
you pass `--yes`.

```
poo                         overview: free space, buckets, plan sizes
poo scan                    update now: the first full scan takes 10-60s; later scans are
                            incremental (file-change journal), about a second
poo scan --full             walk everything again (10-60s)
poo watch                   keep the report current as files change (JSON lines when piped)
poo ls [bucket] [--tier cold] [--project PATH] [--min 1G] [--query text]
poo show <id|path>          reasons, rebuild cost, what is inside
poo plan quick|stale|deep|emergency [--free 30G] [--include-in-use]
poo clean <id>... | --plan stale [--dry-run] [--yes] [--max-tier auto]
poo free 30G [--dry-run]    least-regret items until 30 GB are free
poo projects                projects with status and policy
poo projects set PATH active|stale|protected|auto
poo worktrees               repos, their worktrees, dirty/merged state, sizes
poo catalog                 every bucket, rule, provider and recommendation (large; prefer
                            `poo rules ls --bucket <id>` or the MCP catalog with a filter)
poo rules explain PATH      which rule matches a path and why
poo history                 what poo cleaned before
poo setup --agent           setup brief for configuring poo for this user
poo schedule add nightly    routine cleanups (presets: nightly, quick, guard, weekly)
poo schedule ls | run <id> --dry-run | rm <id>
poo rules ls --hits         rules that matched on this machine
poo rules disable --tag <t> turn a whole pack off (also --group, --bucket, --stack)
```

MCP: `poo mcp` serves the same as tools (`overview`, `list_items`, `inspect`,
`plan`, `clean`, `projects`, `catalog`, `configure`, `explain_path`,
`setup_brief`, `schedules`, `backups`, `growth`, `history`). `catalog` without
arguments is a summary; pass `bucket`, `tag`, `stack`, `id` or `query` for full rules.
`plan` and `clean` take a `mode` (safe, in-use, dangerous) and a plan only lists what
`clean` in that mode would take. `clean` defaults to a dry run and
answers `needs_approval` for items above the autonomy tier; `schedules` add/enable
answer with a preview until `approved_by_user`. Pass `approved_by_user: true` only
after the user saw those exact items and said yes.
A clean returns as soon as folders are moved aside; `erasing_bytes` in its result is
what a background erase is still freeing, so `disk_free_after` catches up seconds later.

## Routine cleanups

Schedules make free space a non-issue: `poo schedule add nightly` cleans everything
nothing is using every night; `poo schedule add guard` frees space automatically when
the disk runs low. Scheduled runs rescan first and only clean items without risks.
Before adding or enabling one, show `poo schedule presets` for it and `poo plan <plan>`
(what it would clean today), and ask. Tune with `--bucket`, `--exclude-bucket`, `--tag`,
`--exclude-tag`, `--max`, `--when-free-below`, `--target-free`.

## Typical flows

- Out of space: `poo --text`, then `poo free 20G --dry-run --text`. Clean the junk and
  cold part if the user asked you to free space; show the rest (sizes, status, what a
  rebuild costs) and ask before `poo free 20G --yes`. Items with risks need a mode
  (`--include-in-use`) the user agreed to. If the target is out of reach, say how close
  the least-regret options get.
- Plans list only what `poo clean` would take with the same flags: `poo plan stale` is
  exactly `poo clean --plan stale`, and `poo plan stale --include-in-use` previews that
  mode (worktrees with uncommitted work, stashed first; items used in the last hour).
- What takes space: `poo`, then `poo ls <bucket>` and `poo show <id>`.
- After a feature lands: `poo worktrees`; propose removing merged worktrees (squash and
  rebase merges and merged PRs count; `landed_by` on the item says how). Their
  build output goes with them and the branch stays. Worktrees with uncommitted changes
  are skipped by default; with `--include-in-use` poo stashes the changes first
  (recoverable from `git stash list` and `poo backups`).
- Duplicated builds: the same repo across many worktrees often costs 10-25 GB
  each (DerivedData, .build, target, node_modules). `poo ls --query <repo>` shows it.

## Configure poo for the user

When asked to set up or tune poo, run `poo setup --agent` and follow the brief.
Combine it with what you already know about the user's projects and habits
from memory and context. Propose the configuration and ask what you cannot infer in
one message; apply it after the user answers. Do not lower or raise the autonomy tier,
add roots, schedules or hooks on your own.

## Extend poo with mods

All of poo's knowledge is mod data. To teach it about a folder it does not
recognise (`poo ls other`), disable a rule, change a threshold or add buckets:

```
poo mods new <id>           creates ~/.config/poo/mods/<id>/ with examples
poo mods validate           precise errors for every mod
poo rules explain <path>    check your rule matches (use a real folder from `poo ls other`)
poo mods docs               the full mod format
```

## When you find space no rule explains

If `poo ls` puts big folders under Unclassified, or `poo suggest` lists any, you can teach poo:

1. Run `poo suggest --write <mod>` (or write the rule yourself in ~/.config/poo/mods/<mod>/).
   Fill in every TODO: what writes the folder, the command that recreates it, its stacks,
   and `running_apps` for apps that keep it open. Generic names (build, dist, out, deps)
   need a marker file next to the folder plus `children_any` or `ignored = true`.
2. Check it with `poo rules explain <path>` and `poo mods lint`, then set `enabled = true`.
3. Offer to share it: run `poo mods submit <mod> --json`, show the person the files, the PR
   text and what was scrubbed, and say plainly what leaves the machine (exactly that text,
   sent to github.com/morajabi/poo via their gh account). Only after they say yes, run it
   again with `--yes`. Never pass `--yes` on your own.

Prefer config overrides (`poo rules set <id> warm_days=7`, `poo rules disable <id>`,
`poo buckets set <id> --name ...`) for small changes and a mod for new knowledge.
