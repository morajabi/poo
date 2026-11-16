# App icon

`AppIcon.icon` is an Icon Composer document (layered Liquid Glass, with light, dark,
tinted and clear appearances). `scripts/build-icon.sh` compiles it with `actool` into
`Assets.car` (macOS 26+) and `AppIcon.icns` (macOS 14–15). `scripts/build-app.sh` calls it.

- **AppIcon.icon** (Flush): a soft glass swirl pulling colorful build blocks in. The default.
- **concepts/Puff.icon**: a fluffy dust-bunny mascot with a sparkle.
- **concepts/Broom.icon**: a broom flicking build blocks away.
- **concepts/Beetle.icon**: a dung beetle rolling a ball of build junk (rough).
- **concepts/Gauge.icon**, **Platter.icon**, **Sweep.icon**: the first, quieter round.

To try a concept: `ICON=app/Icon/concepts/Puff.icon scripts/build-icon.sh`. To switch
for good, copy it over `AppIcon.icon`, then re-export `site/images/icon.png`:
`ictool app/Icon/AppIcon.icon --export-image --output-file site/images/icon.png --platform macOS --rendition Default --width 256 --height 256 --scale 1`
(`ictool` is inside Icon Composer.app/Contents/Executables). Open any of them in Icon
Composer (Xcode ▸ Open Developer Tool) to tweak. `generate.py <dir>` regenerates the
Flush, Puff, Broom and Beetle concepts from code.
