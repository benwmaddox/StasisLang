# iOS arm64 package

On macOS, install Xcode and place device-capable `SDL3.xcframework` and
`SDL3_image.xcframework` in one directory. The validated inputs are the
official SDL3 3.4.10 and SDL3_image 3.4.4 release DMGs; their versions match
the native runtime pins. Build the checked-in thin Xcode project with your
signing team:

```text
xcodebuild -project StasisMobile.xcodeproj -scheme StasisMobile \
  -configuration Debug -sdk iphoneos -arch arm64 \
  STASIS_SDL_FRAMEWORKS=/absolute/path/to/frameworks \
  DEVELOPMENT_TEAM=YOUR_TEAM_ID build
```

The target links the AOT objects from `../aot`, compiles the shared SDL-only
runtime from `../runtime`, and copies `StasisMobile/stasis_game` into the app
resources. It contains no JIT, hot swap, dynamic game loader, or writable
Stasis source.

For a network-enabled package, `stasis package-mobile --target ios-arm64`
must run on macOS with Xcode's `iphoneos` SDK and the
`aarch64-apple-ios` Rust target. The package command builds the signed/static
`stasis_network` library, stages it under `ios/network/`, and enables it only
through `StasisMobile.xcconfig`; it also stages `network_guest.bundle` under
`StasisMobile/stasis_game`. The generated `Info.plist` requests local-network
permission for direct TCP/unicast play. This v1 transport does not use Bonjour,
multicast, or discovery entitlements. After startup the native shell presents
the host join URL in a bounded Copy/Dismiss alert; the URL is not passed through
Stasis state or logs.

Pull requests run `tools/ci/build_ios_package.sh` on macOS. That check verifies
the published DMG hashes, packages `samples/mobile_storage_link`, performs an
unsigned `iphoneos` arm64 Xcode build, and inspects the resulting app's
architecture, embedded SDL frameworks, game assets, provenance, and source
exclusion. Signing and installation remain a developer-owned Xcode handoff.

The optional slow PR seam also makes a CI-only copy of that generated package,
replaces its device-only AOT objects with a simulator qualification fixture,
and builds the same shared SDL/mobile runtime for an arm64 iOS simulator. The
fixture fixes the logical canvas at 1600x720 and records the live safe-area fit,
an injected resize/orientation-safe-area transition, and logical pointer
round-trip receipts. The job uploads both landscape-stage screenshots, exact
Xcode/runtime/device identifiers, logs, and the machine-checked receipt.
The receipt is authoritative for app-window orientation. `simctl io screenshot`
may encode those landscape stages in hardware-native portrait pixel order, so
the verifier accepts only the exact drawable dimensions or their exact
transpose, labels that encoding, and requires distinct stage colors.
The live receipt preserves the simulator's actual safe area, including a
truthful zero-inset result. The injected receipts separately exercise the
production mobile safe target, left/right cutout math, display generation, and
logical pointer mapping; they are seam evidence, not device-observed cutouts.
This does not add a public simulator package target, and it does not qualify
signing, thermal behavior, touch hardware, or cutout behavior on a physical
phone.
