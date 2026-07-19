# Exploration Garden

This is the same imported gameplay schedule used by Stasis Workshop and the Android AOT bundle.
From this directory, a desktop installation can run it with deterministic touch input:

```text
stasis play src/host.stasis --watch-dir . --input-script qa/first_keepsake.json --ticks 30
```

For repository development, the equivalent bounded acceptance command is:

```text
cargo run --manifest-path ../../../../../../../Cargo.toml -p stasis -- play src/host.stasis --watch-dir . --input-script qa/first_keepsake.json --ticks 30
```

`stasis check` and `stasis build --mode release` use the AOT adapter selected by `stasis.json`;
`stasis test` discovers the imported gameplay tests. Neither changes the Workshop adapter at
`src/main.stasis`.
