# Desktop native adapters

Manifest-v3 projects may opt into one project-owned C adapter while Stasis keeps
the application entry point and frame loop. The adapter is intended for bounded
platform services that must share the packaged process, such as a Windows Store
API. It is not a general plugin loader.

```json
{
  "manifest_version": 3,
  "desktop": {
    "native_adapter": {
      "abi_version": 1,
      "source": "native/store_adapter.c",
      "system_links": {
        "windows": ["windows_app", "runtime_object"]
      }
    }
  }
}
```

`source` must be one project-relative `.c` file no larger than 1 MiB. Packaging
captures and hashes it with the manifest and reachable Stasis sources, then
freezes those exact bytes into the AOT input directory and compiles that copy as
a separate translation unit. The file must be self-contained apart from system
headers, `stasis_desktop_adapter.h`, and generated `stasis_host_exports.h`;
project-local headers are not an adapter input. Unknown fields, unknown libraries,
duplicate libraries, absolute/traversing paths, unsupported ABI versions, and
more than eight total system-link entries are rejected.

The allowlists are intentionally small:

- Windows: `advapi32`, `ole32`, `runtime_object`, `shell32`, `user32`, and
  `windows_app`.
- Linux: `dl`, `m`, and `pthread`.
- macOS: `app_kit`, `foundation`, and `store_kit` frameworks.

The adapter includes `stasis_desktop_adapter.h` and implements its four fixed
hooks. It may also include the generated `stasis_host_exports.h` to call guest
functions declared with `@host_export`. `initialize` runs after guest `main` and
before the first tick. `pump` runs exactly once before every runtime step.
`on_foreground` requests a service-state requery after the window regains input
focus or is restored from a minimized/hidden state; the next pump occurs before
the next guest tick. `shutdown` runs exactly once
after successful initialization and before the runtime destroys the window.
Returning `STASIS_DESKTOP_ADAPTER_PUMP_REQUEST_EXIT` from `pump` asks Stasis to
leave the loop successfully; Stasis still calls adapter `shutdown` and then tears
down the runtime window. Other nonzero hook results are failures.

These hooks run only in a source-built desktop monolith produced by desktop
`build --mode release` or `package --target desktop`. Stasis validates the
manifest declaration and adapter source when it checks the project, but JIT
preview (`check`, `play`, `run`, `live`, and `build --mode dev`) does not compile
or execute the adapter. Web, Android, and iOS builds likewise ignore this
desktop-only input and preserve their existing runtime behavior. Projects that
need the service in preview or on another target must provide a separate,
target-specific implementation.

Every hook runs on the UI/runtime thread. Async platform callbacks must enqueue
bounded native results and return. Only `pump` or another runtime-thread hook may
publish those results through generated guest exports. This keeps request/result
ordering at the existing between-tick boundary.

The context begins with `struct_size` and `abi_version`. Its native window is a
borrowed runtime-owned handle: `HWND` on Windows, an AppKit window on macOS, or a
Wayland surface/X11 window identity on Linux. The adapter must not destroy,
subclass beyond its own balanced lifetime, retain past shutdown, or use the
handle from an async callback. Check both `platform` and `native_window_kind`
before casting `native_window`.

Package consumers should pin the first official Stasis release whose provenance
contains this contract. A development build or source PR does not establish the
supported consumer release boundary.
