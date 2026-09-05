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
