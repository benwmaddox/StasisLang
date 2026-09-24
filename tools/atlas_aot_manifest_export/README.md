# AOT engine-bundle manifest exporter

This standalone Rust utility compiles a project with `stasis_compiler::AotProcess` using release reachability, then writes the ordinary engine-bundle manifest and object files. It does not link or run the game, and it does not modify the source project.

Example for SheepHerder, from the StasisLang checkout:

```powershell
cargo run --offline --manifest-path tools/atlas_aot_manifest_export/Cargo.toml -- D:\code\SheepHerder src\main.stasis D:\code\StasisLang\build\task623-sheepherder-aot
```

Arguments are project root, entry file (default `src/main.stasis`), and output directory (default `<project-root>/build/atlas-affinity-aot-manifest`). The output directory contains `engine_bundle_manifest.json` and the AOT object files. The preview exporter accepts that JSON path with `--aot-manifest`.

A v4 schema alone does not mean pair evidence is accepted. The preview validates the transition summary; incomplete, unknown, omitted, or unmapped rows are reported and cause use of the same native snapshot's bounded runtime histogram when available. SheepHerder's current manifest is v4 but its summary is incomplete because analysis reaches an unbounded or dynamic loop.
