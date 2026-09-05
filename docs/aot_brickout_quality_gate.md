# AOT Brickout Quality Gate

Brickout Revenge v1 is the representative production-AOT compiler gate. It is a default Rust test,
not an opt-in environment-variable check.

## Command

```powershell
python tools/cargo_cache.py run -- cargo test -p stasis --lib aot_brickout_revenge_v1_ -- --test-threads=1
```

The test compiles `samples/brickout_revenge/brickout_revenge_v1.stasis` with the production AOT
profile and verifies the engine bundle, lifecycle objects, string-literal table, and collection
metadata. On Windows, the same default test group also compiles the production runtime bridge,
links the complete engine bundle, loads it, initializes a deterministic headless host frame, and
executes `main` followed by two `tick` calls. The execution test is not hidden behind an environment
variable; a missing linker, unresolved symbol, load failure, or bad return value fails the gate.

Other platform packaging workflows additionally link and execute their release images against the
installed platform SDK/toolchain; they do not weaken or replace the default compiler gate.
