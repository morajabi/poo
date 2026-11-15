---
name: poo
description: Inspect and safely free disk space on a developer Mac with the `poo` CLI or its MCP tools. Use when the user is low on disk space, asks what takes space, wants to clean build output, caches, DerivedData, simulators, Docker or agent worktrees, or asks you to configure or extend poo.
---

# poo: disk space for developers

poo finds what eats disk on a dev machine and frees it safely. It knows dev
workflows: Rust `target`, SwiftPM `.build`, Xcode DerivedData per checkout,
`node_modules`, virtualenvs, simulators and runtimes, Docker, package caches,
and the linked git worktrees Claude, Codex, Conductor and others create. Every
item gets a tier that says how much regret cleaning it causes.

## Rules for you

- Never delete files yourself (`rm`, `git clean`, `docker system prune`...).
  Clean through poo: it re-checks each item right before deleting, refuses
  worktrees with uncommitted changes, skips projects with running builds and
  never leaves its safety floor.
- Junk and cold items may be cleaned when the user asked you to free space.
  Idle and warm items need the user's OK unless `poo config get agents.auto_clean_max_tier`
  allows them. Hot, review and protected items: show them and let the user decide.
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

## Tiers

| tier | meaning | plans |
|---|---|---|
| junk | nothing to rebuild (logs, temp, crash reports, broken simulators) | all |
| cold | rebuildable and unused for days, orphaned, or its worktree's work already landed | stale, deep, emergency |
| idle | nothing uses it now: other checkouts' builds while you work in your 2 newest, untouched for hours | stale, deep, emergency |
| warm | in active use; costs a rebuild or redownload | deep, emergency |
| hot | in use right now (build or agent running, touched in the last hour) | never |
| review | needs a human look (unknown ignored folder, transcripts, archives) | never |
| protected | never cleaned (uncommitted work, data, volumes) | never |

## Commands

Output is JSON when piped; add `--text` for tables. Errors are
`{"error":{"kind","message","hint"}}` on stderr.

```
poo                         overview: free space, buckets, plan sizes
poo scan                    rescan (results are cached for an hour)
poo ls [bucket] [--tier cold] [--project PATH] [--min 1G] [--query text]
poo show <id|path>          reasons, rebuild cost, what is inside
poo plan quick|stale|deep|emergency [--free 30G]
poo clean <id>... | --plan stale [--dry-run] [--yes]
poo free 30G [--dry-run]    least-regret items until 30 GB are free
poo projects                projects with status and policy
poo projects set PATH active|stale|protected|auto
poo worktrees               repos, their worktrees, dirty/merged state, sizes
poo catalog                 every bucket, rule, provider and recommendation
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
`setup_brief`, `schedules`, `history`). `clean` defaults to a dry run.

## Routine cleanups

Schedules make free space a non-issue: `poo schedule add nightly` cleans everything
nothing is using every night; `poo schedule add guard` frees space automatically when
the disk runs low. Scheduled runs rescan first and only clean items without risks.
Ask the user before adding or enabling schedules; tune them with `--bucket`,
`--exclude-bucket`, `--tag`, `--exclude-tag`, `--max`, `--when-free-below`, `--target-free`.

## Typical flows

- Out of space: `poo free 20G --dry-run`, show the list, then `poo free 20G --yes`.
- What takes space: `poo`, then `poo ls <bucket>` and `poo show <id>`.
- After a feature lands: `poo worktrees`; propose removing merged worktrees. Their
  build output goes with them and the branch stays.
- Duplicated builds: the same repo across many worktrees often costs 10-25 GB
  each (DerivedData, .build, target, node_modules). `poo ls --query <repo>` shows it.

## Configure poo for the user

When asked to set up or tune poo, run `poo setup --agent` and follow the brief.
Combine it with what you already know about the user's projects and habits
from memory and context. Ask only what you cannot infer.

## Extend poo with mods

All of poo's knowledge is mod data. To teach it about a folder it does not
recognise (`poo ls other`), disable a rule, change a threshold or add buckets:

```
poo mods new <id>           creates ~/.config/poo/mods/<id>/ with examples
poo mods validate           precise errors for every mod
poo rules explain <path>    check your rule matches
poo mods docs               the full mod format
```

Prefer config overrides (`poo rules set <id> warm_days=7`, `poo rules disable <id>`,
`poo buckets set <id> --name ...`) for small changes and a mod for new knowledge.
