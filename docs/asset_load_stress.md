# Asset-load stress fixture

The Windows slow CI lane generates an ephemeral asset project with
`tools/ci/generate_asset_load_fixture.py`. The fixture contains 10 distinct
font paths (copies of the canonical test font), 256 small PNG sprites, 256 SVG
sprites, 24 larger PNG sprites, and 600 deterministic text phrases. Its v2
manifest has 546 entries and is removed by the test guard; no generated files
belong in source control.

`apps/stasis/tests/desktop_asset_load_stress.rs` validates the manifest with
the default `stasis_assets::AssetLimits`, loads all 536 sprites and 10 fonts
through the native SDL runtime, measures and caches all 600 phrases, checks
positive unique handles and cache reuse, and writes bounded count/timing
evidence to `CARGO_TARGET_DIR/seam-tests/it-asset-load-stress.json` before
cleanup. Reproduce on Windows after building the runtime by
setting `STASIS_RUNTIME_DLL_PATH` and running:

```text
python -m unittest tools.ci.test_generate_asset_load_fixture
python tools/cargo_cache.py run -- cargo test -p stasis --test desktop_asset_load_stress -- --nocapture
```

The load test fixed the former eight-font capacity failure by making the
runtime font table explicitly bounded at 32 entries. Other explicit runtime
bounds remain: 4096 manifest entries, 1024 cached text runs, 256 KiB cached
text bytes, 65,536 cached text quads, and 4096 submitted sprites. The test does not
claim coverage for mobile/web packaging, physical GPU texture limits, or
platform-specific font rasterizer behavior.
