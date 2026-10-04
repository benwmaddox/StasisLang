# Generic bounded collections

This sample is a small, allocation-free consumer of Stasis compile-time
generic parameters. Run it from a checkout with:

```text
stasis --workspace samples/generics_collections vendor status
stasis --json vendor status --workspace samples/generics_collections
stasis --workspace samples/generics_collections fmt --check
stasis --workspace samples/generics_collections check
stasis --workspace samples/generics_collections test
stasis --workspace samples/generics_collections run --headless --ticks 1
stasis --workspace samples/generics_collections package --target desktop --development-build
stasis --workspace samples/generics_collections package --target web --development-build
stasis --workspace samples/generics_collections package-mobile --target android-arm64 --out build/android-arm64 --development-build
stasis --workspace samples/generics_collections package-mobile --target android-x86_64 --out build/android-x86_64 --development-build
stasis --workspace samples/generics_collections package-mobile --target ios-arm64 --out build/ios-arm64 --development-build
stasis --workspace samples/generics_collections package-mobile --target ios-simulator-arm64 --out build/ios-simulator-arm64 --development-build
```

The production compiler seam checks the canonical `src/main.stasis` entry
through JIT, linked AOT, raw Wasm, and the packaged Web runtime. All targets
execute the same generic collection workload and expose the same deterministic
post-run state digest through `generics_collections_state_digest()`, while
`tick()` retains its zero-success lifecycle contract. Packaged Web reads the
captured digest through the supported global accessor. The Web acceptance also verifies the
real browser's WebGL2 frame and the fixed-array bounds trap. Android uses this
same full entry and valid frame lifecycle on an x86_64 emulator. Its bounded
test seam reads the digest global as `507`, verifies the digest-gated teal frame,
checks the successful launch log for crash/error evidence, and runs `tick()` with
indices `-1` and `2` in separate emulator processes to prove both `Entity[2]`
bounds traps. The production arm64 package/link check remains a separate CI
build; no physical arm64 device run is required.
The iOS acceptance packages the same unchanged entry separately for device and
arm64 Simulator ABIs. Hosted Simulator execution requires lifecycle result `0`,
reads the packaged digest global as `507`, inspects the teal frame, and runs
low/high bounds probes as isolated fatal-trap processes. Device package
architecture, frameworks, symbols, assets, provenance, and hashes are recorded
separately; unsigned CI output is never reported as physical-device evidence.
The desktop acceptance runs the shared semantic oracle through production JIT
and a freshly linked native AOT executable on each desktop host, including
isolated bounds-trap children. It then packages and launches this canonical
sample through the production desktop runtime. The authored frame is teal only
when the captured sample digest is exactly `507`, giving the package launch an
independent visible digest oracle; a mismatch renders red. The shared entry
also runs the scalar math oracle and hashes 13 raw f32 bit patterns, including
negative-zero input and canonicalized outputs, into
`math_oracle_raw_digest_value`. Every target compares that value with the
independent expected digest `-1430176193`; packaged Web and mobile receipts
record the observed value, and the desktop teal frame is gated on both digests.
`vendor/stasis` is the recorded, hash-checked graphics/runtime snapshot used
by every packaged target.

## Install or upgrade the compiler

Use the versioned Stasis release archive for your operating system. Extract it
to its own directory and put its compiler directory on `PATH`: the Windows
archive provides `stasis.exe` at its root, while Linux and macOS provide
`bin/stasis`. Verify the selected executable with `stasis version`,
`stasis env`, and `stasis --json editor-info`. To upgrade, extract the newer
archive alongside the current one and update `PATH`; do not replace a compiler
inside the sample. See [the CLI installation contract](../../docs/toolchain_cli.md#install)
for the runtime files and platform build prerequisites.

After selecting the newer compiler, inspect this project's pin, explicitly
update it, and inspect the result again:

```text
stasis --json vendor status --workspace samples/generics_collections
stasis --workspace samples/generics_collections vendor update
stasis --json vendor status --workspace samples/generics_collections
```

The status digest must match the `vendor/stasis` tree. Review the manifest and
snapshot diff before committing an upgrade. If the new snapshot is not wanted,
restore `stasis.json` and `vendor/stasis` from the reviewed commit containing
the intended pin, then verify the status again.

## Vendor snapshot review and recovery

`vendor status` is read-only. Use its JSON report to check the selected
toolchain's release identity and compare the recorded vendor digest with the
checked-in tree before changing the pin:

```text
stasis --json vendor status --workspace samples/generics_collections
stasis --workspace samples/generics_collections vendor update
stasis --json vendor status --workspace samples/generics_collections
git diff -- samples/generics_collections/stasis.json samples/generics_collections/vendor/stasis
```

`vendor update` replaces the manifest and `vendor/stasis` as one transaction.
If an update fails, it restores the previous files. If a successful update
needs to be rolled back, restore both paths from the reviewed commit that held
the intended snapshot, then run `vendor status` again. When `HEAD` is that
known-good commit, the recovery command is:

```text
git restore --source=HEAD -- samples/generics_collections/stasis.json samples/generics_collections/vendor/stasis
stasis --json vendor status --workspace samples/generics_collections
```

The generated CI consumer starts without a local `vendor/stasis` snapshot and
must populate it from the accepted archive. The bundled consumer starts with
the checked-in snapshot, updates it to that same nightly, and checks the
resulting manifest and hash. Both consumers also inject a tampered release
archive and verify that a rejected update leaves their manifest and vendor
tree unchanged.

```text
python tools/cargo_cache.py run -- cargo test -p stasis_compiler --test generics_collections_jit_aot_wasm -- --nocapture
python tools/cargo_cache.py run -- cargo test -p stasis_compiler --test generics_collections_aot_seam -- --nocapture
python tools/cargo_cache.py run -- cargo test -p stasis --test generics_collections_desktop -- --ignored --exact full_generics_desktop_package_launches_with_provenance_and_digest_frame --nocapture
```

## Storage contract

`Buffer<T, N>`, `Ring<T, N>`, `SampleWindow<N>`, `EntityPool<N>`, and
`Rig2D<N>` own a fixed `T[N]` or struct-array field. `N` is a non-negative,
compile-time `i32`; zero-capacity arrays have no usable element slots and
follow the existing fixed-array extent policy. The full bounded layout is
reserved even when `count` is zero. None of these helpers allocates, resizes,
or exposes a runtime length.

Counts are explicit and are the only logical occupancy. `buffer_clear`,
`window_clear`, `ring_clear`, and `entity_pool_clear` reset reuse state without
erasing the backing layout. Appends and pushes reject full collections without
changing the count. `Ring` wraps `head` and its write slot modulo `N` and
rejects empty drops. `EntityPool` reuses the last released slot and only copies
scalar fields, which is the supported struct-view shape in this sample.

Scalar `i32` and `f32` elements support assignment and return through
receiver-bound helpers. `Entity` is accessed through indexed field paths and an
`Entity[]` view; the sample deliberately does not claim a general deep-copy
operation for arbitrary composite `T`. The concrete `first_value(i32[])`
demonstrates the existing fixed-array-to-view rule without introducing a raw
view-only generic.

`Nested<T, N>` demonstrates a legal generic container containing another
generic container. The two integer buffers use capacities 4 and 8 and are
mutated independently. `CompileValue<N>` binds the compile-time value used by
`apply_offset` and `apply_mode`, demonstrating that an `i32` generic value is
not inherently a capacity. `buffer_capacity` is exercised in both free and
receiver-style call paths; function calls never carry explicit generic
arguments.

`live_edit_helper.stasis` is intentionally tiny: `live_edit_tick` increments
`live_edit_state` before calling the receiver-bound specialization for
`LiveEditPolicy<7>`. During a live session, change only the generic helper
return expression and re-run the live-edit command; every affected concrete
specialization and caller is rebuilt, the next result changes, and the
incremented `live_edit_state` remains. The generic collection layouts are not
changed by that helper edit, so unrelated artifacts and state remain reusable.

The intentionally failing sources are under `negative/`, outside the normal
project `tests/` directory. They are compiled by the focused Rust harness and
cover runtime capacity arguments, unsupported composite element copies,
unresolved fixed-capacity inference, checked layout overflow, and ambiguous
generic receivers. They must not be included in the green sample test run.
