# Workshop Android libraries

`rustls-platform-verifier.aar` is the Android support component bundled by
`rustls-platform-verifier-android` 0.1.1. It is required by the phone-native
Codex Rust library so TLS verification uses Android's system trust manager.

`mobile/android/build_codex_native.ps1` discovers and recopies the AAR from the
Cargo package on every native build. Upstream source and licensing:
<https://github.com/rustls/rustls-platform-verifier>.
