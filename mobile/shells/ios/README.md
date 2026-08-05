# iOS arm64 package

On macOS, install Xcode and place device-capable `SDL3.xcframework` and
`SDL3_image.xcframework` in one directory. Build the checked-in thin Xcode
project with your signing team:

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
