<!-- For rules, stacks, worktree homes and launchers. `poo mods submit` fills this in for you. -->

## What this adds

- **Rule name** `tool.thing`: one line on what the folder is and why removing it is safe.

## Evidence

| rule | seen on my Mac | how it comes back |
|---|---|---|
| `tool.thing` | e.g. 3 folders, 4.2 GB | `tool build` |

Tool versions (`tool --version`):

-

## Safety checklist

- [ ] `poo mods validate` and `poo mods lint --core` pass
- [ ] paths stay under `~` or `$TMPDIR` (system caches set `system = true` and explain why)
- [ ] generic folder names (`build`, `dist`, `out`, `deps`...) have a marker next to them and proof inside, or `ignored = true`
- [ ] new generic rules have fixtures in `tests/rule-fixtures/` that match and that must not match
- [ ] the folder really comes back by itself or with the `rebuild` command
- [ ] apps that keep it open are listed in `running_apps`; protected locations are tagged `needs-permission`
- [ ] toolchains, SDKs and models use `tier = "review"`; anything a person made uses `cost = "data"`
