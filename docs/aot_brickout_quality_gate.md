# AOT Brickout Quality Gate

Brickout Revenge v1 is the representative production-AOT compiler gate. It is a default Rust test,
not an opt-in environment-variable check.

## Command

```powershell
python tools/cargo_cache.py run -- cargo test -p stasis aot_brickout_revenge_v1_compiles_full_engine_bundle -- --test-threads=1
```

The test compiles `samples/brickout_revenge/brickout_revenge_v1.stasis` with the production AOT
profile and verifies the engine bundle, lifecycle objects, string-literal table, and collection
metadata. It does not require a system linker or signing credential, so it can run in the default
hermetic validation suite.

Platform packaging workflows separately link and execute release images. Those workflows verify
the installed platform SDK/toolchain; they do not weaken or replace the default compiler gate.
