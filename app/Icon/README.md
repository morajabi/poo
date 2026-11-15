# App icon

`AppIcon.icon` is an Icon Composer document (layered Liquid Glass, with light, dark,
tinted and clear appearances). `scripts/build-icon.sh` compiles it with `actool` into
`Assets.car` (macOS 26+) and `AppIcon.icns` (macOS 14–15). `scripts/build-app.sh` calls it.

- **AppIcon.icon** (Gauge): a free-space gauge with a spark. This is the default.
- **concepts/Platter.icon**: a disk platter, a sweep arc and a spark.
- **concepts/Sweep.icon**: three receding sweep lines and a spark.

To try a concept: `ICON=app/Icon/concepts/Platter.icon scripts/build-icon.sh`. To switch
for good, copy it over `AppIcon.icon`. Open any of them in Icon Composer
(Xcode ▸ Open Developer Tool) to tweak. `generate.py <dir>` regenerates all three from code.
