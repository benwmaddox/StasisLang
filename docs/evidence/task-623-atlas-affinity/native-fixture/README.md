# Native atlas fixture evidence

This is a real SDL/native atlas contract fixture using SheepHerder source assets in a 200×120 test surface. It verifies the native query schema and the byte-preserving submit path; it is not a representative 2000×900 packaged-game capture.

The fixture loads sheep, dog, and dog-run assets, reports three pages (2048×2048 frozen page, 64×64 and 128×128 plan-eligible pages), three residents, and two directed pair rows of weight 63 each. The original before/after fixture PNGs are byte-identical (SHA-256 `381bee089b406cff7d839f953c0d3fc034f60526e21340350e3a643e02952201`).

The archived [raw query](atlas-affinity-native-query-v1.json) and [compiler v4 manifest](engine-bundle-manifest-v4.json) are the inputs used by the [production Rust planner report](../native-fixture-sheepherder/SheepHerder-report.json). That report yields 2→1 eligible pages, directed cut weight 63→0, candidate texture bytes 65,536, final device bytes 16,859,136→16,842,752, and peak device bytes 16,924,672. The AOT manifest is incomplete because of an unbounded or dynamic loop, so the fixture uses the bounded native runtime histogram. GPU timing and native frame page-run counts were not measured.

Reproduce the native C fixture with:

```powershell
ctest --test-dir build/task623-atlas-native -C Release -R '^stasis_sprite_atlas_staging_contract$' -V
```
