---
name: poo
description: Inspect and safely free disk space on a developer Mac with the poo CLI. Use when the user is low on disk space, asks what takes space, wants to clean builds, caches, DerivedData, simulators, Docker or agent worktrees, or asks to configure poo.
---

# poo

Before using poo, run `poo --skill` and follow it. The installed binary is the
authority for commands and flags.

If `poo` is not installed, ask the user before installing it:
`curl -fsSL https://raw.githubusercontent.com/morajabi/poo/main/install.sh | sh`

Never delete files yourself; clean through `poo clean` or `poo free`, which
re-check every item and refuse uncommitted worktrees and running builds.
