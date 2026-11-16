# App icon

`AppIcon.icon` is an Icon Composer document (layered Liquid Glass, with light, dark,
tinted and clear appearances). `scripts/build-icon.sh` compiles it with `actool` into
`Assets.car` (macOS 26+) and `AppIcon.icns` (macOS 14–15). `scripts/build-app.sh` calls it.

**AppIcon.icon** is Poo: a chubby two-tier vanilla swirl with the Puff face and a sparkle, on chocolate.

Alternates in `concepts/`:
- Vanilla family (same palette and face): `Vanilla*` shapes (two tier, dome, mochi, dumpling,
  dollop, lean curl, three tier) and backgrounds (`*Deep`, `*Warm`, `*Soft`).
- Puff family, same face: `Swirl*` (soft-serve in chocolate, toffee, cocoa, vanilla and
  caramel on butter, sky, mint, chocolate and peach), `Puff*` (cloud puff in browns), `Drop*`,
  and `Puff.icon` (the original white cloud on blue).
- Earlier rounds: `Flush`, `Broom`, `Beetle`, `Gauge`, `Platter`, `Sweep`.

To try one: `ICON=app/Icon/concepts/SwirlToffeeSky.icon scripts/build-icon.sh`. To switch
for good, copy it over `AppIcon.icon`, then re-export `site/images/icon.png`:
`ictool app/Icon/AppIcon.icon --export-image --output-file site/images/icon.png --platform macOS --rendition Default --width 256 --height 256 --scale 1`
(`ictool` is inside Icon Composer.app/Contents/Executables). Open any of them in Icon
Composer (Xcode ▸ Open Developer Tool) to tweak. `generate_vanilla.py <dir>` regenerates the
Vanilla family, `generate_puff.py <dir>` the Puff family from code and `generate.py <dir>` the Flush round.
