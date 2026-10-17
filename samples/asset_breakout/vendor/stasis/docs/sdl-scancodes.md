# SDL scancodes

`src/stdlib/sdl_scancodes.stasis` exposes SDL's named physical-key slots as the
typed `SCANCODE` constant group. Import it and use a qualified member when
indexing `HostFrame.keys`:

```stasis
import "/vendor/stasis/stdlib/host_frame.stasis";
import "/vendor/stasis/stdlib/sdl_scancodes.stasis";

global frame: HostFrame;

function should_move_left(): bool {
    // Refresh once per game tick, then read all keys from that snapshot.
    frame.refresh();
    return frame.keys[sdl_scancodes.SCANCODE.A] != 0 || frame.keys[sdl_scancodes.SCANCODE.LEFT] != 0;
}
```

Scancodes identify physical keyboard positions. They are not text characters
or locale-independent key labels; use text input for typed text. A host may
provide only the keys it can observe. `HostFrame.keys` has 512 slots, and each
declared member is a valid index into that array.

The generated group contains the 246 ordinary named scancodes from SDL
3.4.10, preserving SDL header order and integer values. `UNKNOWN = 0`,
`RESERVED = 400`, and `COUNT = 512` are omitted: they are enum markers, not
keyboard slots. `LANG6` through `LANG9` are retained because SDL gives them
named slot values, while documenting those slots as reserved. SDL's numeric
names `_1` through `_9` and `_0` are exposed as `DIGIT_1` through `DIGIT_9`
and `DIGIT_0`, which are legal Stasis identifiers. The table has no aliases
or duplicate values.

The older `Scancode` enum remains available for source compatibility. It is a
small subset; new code should use `SCANCODE` for the complete table. The web
host maps browser keyboard events into the SDL physical-key slots it supports,
so browser coverage is necessarily limited to that mapping.

The generator is pinned to the SDL 3.4.10 release archive
(`https://github.com/libsdl-org/SDL/releases/download/release-3.4.10/SDL3-3.4.10.tar.gz`,
SHA-256 `12b34280415ec8418c864408b93d008a20a6530687ee613d60bfbd20411f2785`)
and reviewed `SDL_scancode.h` SHA-256
`d5e3bb25b523b61ed354189919f2bcb058f0f2fa08e0529ce8e21480937c8ca5`. After
the repository's SDL dependency has been fetched, check that the committed
group matches the pinned header with:

```powershell
python tools/generate_sdl_scancodes.py `
  --header runtime/build/_deps/stasis_sdl3-src/include/SDL3/SDL_scancode.h `
  --check
```

To regenerate the group from that header, omit `--check`. The generator
validates the release pin, header digest, marker declarations, unique names
and values, and the `HostFrame.keys` bounds before writing the generated block.
