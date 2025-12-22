# Making Hosts a First-Class Concept in Stasis

## Executive Summary

This document outlines a comprehensive plan for elevating "hosts" to a first-class concept in the Stasis language. A **host** is the wrapping code that executes Stasis programs—whether that's a Windows application via a C wrapper, an Android app, a WebAssembly runtime in a browser, or an embedded game engine.

Currently, Stasis has implicit host coupling: external functions are hardcoded in the compiler, and the runtime (`stasis_graphics.c`) is a monolithic implementation targeting SDL2/OpenGL. This plan explores options to formalize, abstract, and extend the host concept.

---

## Table of Contents

1. [Current State Analysis](#current-state-analysis)
2. [Problem Statement](#problem-statement)
3. [Design Goals](#design-goals)
4. [Architecture Options](#architecture-options)
   - [Option A: Host Capability Manifests](#option-a-host-capability-manifests)
   - [Option B: Trait-Based Host Interfaces](#option-b-trait-based-host-interfaces)
   - [Option C: Dynamic Host Function Registration](#option-c-dynamic-host-function-registration)
   - [Option D: WebAssembly-Style Import/Export Model](#option-d-webassembly-style-importexport-model)
   - [Option E: Hybrid Layered Approach](#option-e-hybrid-layered-approach)
5. [Comparison Matrix](#comparison-matrix)
6. [Platform-Specific Considerations](#platform-specific-considerations)
7. [Implementation Phases](#implementation-phases)
8. [Recommendations](#recommendations)
9. [Appendix: Reference Implementations](#appendix-reference-implementations)

---

## Current State Analysis

### Compilation Pipeline

```
┌─────────────────┐     ┌─────────────────┐     ┌──────────────────┐
│  .stasis source │ ──► │   C# Compiler   │ ──► │  LLVM IR / CLIF  │
└─────────────────┘     └─────────────────┘     └────────┬─────────┘
                                                         │
                        ┌────────────────────────────────┼────────────────────────────────┐
                        ▼                                ▼                                ▼
              ┌─────────────────┐              ┌─────────────────┐              ┌─────────────────┐
              │  Windows DLL    │              │   Linux .so     │              │  macOS .dylib   │
              └────────┬────────┘              └────────┬────────┘              └────────┬────────┘
                       │                                │                                │
                       ▼                                ▼                                ▼
              ┌─────────────────┐              ┌─────────────────┐              ┌─────────────────┐
              │  stasis_runner  │              │  stasis_runner  │              │  stasis_runner  │
              │  + stasis_gfx   │              │  + stasis_gfx   │              │  + stasis_gfx   │
              └─────────────────┘              └─────────────────┘              └─────────────────┘
```

### Current External Function Binding

External functions are currently hardcoded in two places:

1. **Cranelift Backend** (`CraneliftCodeGenerator.cs:95-350`):
   ```csharp
   private void DeclareExternal(string name, string[] paramTypes, string? returnType)
   {
       // Each external function manually declared with signature
   }

   // Example declarations:
   DeclareExternal("stasis_init_window", ["i32", "i32", "i8*"], "i32");
   DeclareExternal("stasis_draw_line", ["f32", "f32", "f32", "f32", "f32", "f32", "f32", "f32"], null);
   ```

2. **LLVM Backend** (`ModuleLowerer.cs:468-609`):
   ```csharp
   private LLVMValueRef GetOrDeclareStasisInitWindow() { ... }
   private LLVMValueRef GetOrDeclareStasisDrawLine() { ... }
   ```

### Current Runtime Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│                    stasis_graphics.c                              │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │                     SDL2 + OpenGL                           │  │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐      │  │
│  │  │   Window     │  │   Sprites    │  │    Input     │      │  │
│  │  │  Management  │  │   & Atlas    │  │   Handling   │      │  │
│  │  └──────────────┘  └──────────────┘  └──────────────┘      │  │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐      │  │
│  │  │    Fonts     │  │   PostFX     │  │  File I/O    │      │  │
│  │  │  (stb_tt)    │  │   Shaders    │  │              │      │  │
│  │  └──────────────┘  └──────────────┘  └──────────────┘      │  │
│  └────────────────────────────────────────────────────────────┘  │
│                              │                                    │
│                              ▼                                    │
│  ┌──────────────────────────────────────────────────────────────┐│
│  │  Fallback: SDL Renderer (when OpenGL unavailable)            ││
│  └──────────────────────────────────────────────────────────────┘│
└──────────────────────────────────────────────────────────────────┘
```

### Key Limitations

| Limitation | Impact |
|------------|--------|
| Hardcoded externals in compiler | Adding new host functions requires compiler changes |
| Monolithic graphics runtime | Cannot swap rendering backends without forking |
| No capability discovery | Programs cannot adapt to host limitations |
| No formal host contract | Each platform re-implements ad-hoc |
| C-only FFI | No direct Java/Kotlin/Swift interop for mobile |

---

## Problem Statement

Stasis currently treats the host environment as an implicit, static dependency. This creates friction when:

1. **Targeting new platforms** (Android, iOS, consoles, WASM)
2. **Embedding in existing applications** (game engines, tools)
3. **Extending capabilities** (custom I/O, networking, audio)
4. **Testing in isolation** (mocking host functions)
5. **Optimizing for specific hosts** (Vulkan vs. Metal vs. OpenGL)

A first-class host concept would make the boundary between Stasis programs and their execution environment explicit, configurable, and extensible.

---

## Design Goals

### Must Have
- **Explicit Host Contract**: Clear interface between Stasis programs and hosts
- **Multiple Backend Support**: Same Stasis code runs on different hosts
- **Backward Compatibility**: Existing programs continue to work
- **Type Safety**: Host function signatures verified at compile time

### Should Have
- **Capability Discovery**: Programs can query available host features
- **Custom Host Functions**: Users can define new host functions
- **Resource Management**: Hosts can provide resource handles safely
- **Hot Reload Support**: Maintain current hot-reload workflow

### Nice to Have
- **Cross-Compilation**: Compile for target host from any platform
- **Host Versioning**: Programs can specify required host version
- **Performance Parity**: No overhead compared to current approach
- **Debugging Bridge**: Hosts can expose debugging hooks

---

## Architecture Options

### Option A: Host Capability Manifests

Introduce JSON/TOML manifest files that describe host capabilities, which the compiler reads to validate external function usage.

#### How It Works

**Host Manifest (host.toml)**:
```toml
[host]
name = "stasis-sdl"
version = "1.0.0"
platform = ["windows-x64", "linux-x64", "macos-arm64"]

[capabilities]
graphics = true
audio = false
networking = false
filesystem = true

[[functions]]
name = "init_window"
signature = "(i32, i32, *utf8) -> bool"
category = "graphics"

[[functions]]
name = "draw_sprite"
signature = "(i32, f32, f32, f32, f32, f32, f32, f32, f32, f32) -> void"
category = "graphics"
requires = ["graphics"]

[[functions]]
name = "is_key_down"
signature = "(i32) -> bool"
category = "input"
```

**Stasis Source**:
```stasis
@require_host("stasis-sdl", version = ">=1.0.0")

function main(): i32 {
    init_window(800, 600, "Game");
    // ...
}
```

#### Implementation Changes

| Component | Change Required |
|-----------|-----------------|
| Compiler | Parse manifest, validate calls against declared functions |
| CLI | New `--host-manifest` flag |
| Codegen | Generate calls based on manifest signatures |
| Runtime | No change (manifests describe existing runtime) |

#### Pros
- Minimal runtime changes
- Explicit documentation of host contract
- Easy to version and distribute
- Decouples compiler from specific host knowledge
- Enables static analysis of host dependencies

#### Cons
- Requires manifest maintenance alongside C code
- No runtime capability discovery
- Manifest/runtime can become out of sync
- Doesn't solve embedding in foreign environments (Android/iOS)

---

### Option B: Trait-Based Host Interfaces

Define host capabilities as **traits** (interfaces) in Stasis, which hosts must implement. Similar to Rust's trait system.

#### How It Works

**Standard Library Host Traits (`stdlib/host.stasis`)**:
```stasis
trait HostGraphics {
    extern function init_window(width: i32, height: i32, title: utf8[]): bool;
    extern function begin_frame(): void;
    extern function end_frame(): void;
    extern function clear(r: f32, g: f32, b: f32, a: f32): void;
    extern function draw_sprite(id: i32, cx: f32, cy: f32,
                                 sx: f32, sy: f32, angle: f32,
                                 r: f32, g: f32, b: f32, a: f32): void;
    extern function load_sprite(path: utf8[]): i32;
}

trait HostInput {
    extern function is_key_down(scancode: i32): bool;
    extern function should_quit(): bool;
}

trait HostTime {
    extern function get_time_ms(): i32;
    extern function sleep_ms(ms: i32): void;
}
```

**Stasis Source**:
```stasis
use host::HostGraphics;
use host::HostInput;

function game_loop() {
    while !should_quit() {
        begin_frame();
        // ...
        end_frame();
    }
}
```

**Host Implementation Registration (C side)**:
```c
// Host provides vtable or direct function pointers
StasisHostGraphics graphics_impl = {
    .init_window = sdl_init_window,
    .begin_frame = sdl_begin_frame,
    .end_frame = sdl_end_frame,
    .clear = sdl_clear,
    .draw_sprite = sdl_draw_sprite,
    .load_sprite = sdl_load_sprite,
};

stasis_register_host_graphics(&graphics_impl);
```

#### Implementation Changes

| Component | Change Required |
|-----------|-----------------|
| Language | Add `trait` keyword, `extern function` in traits |
| Compiler | Trait resolution, generate indirect calls or link-time binding |
| Codegen | Generate vtable slots or symbol references |
| Runtime | Host registration API, vtable management |

#### Pros
- Type-safe, compiler-verified host contract
- Natural grouping of related functionality
- Enables partial host implementation (headless testing)
- Familiar pattern from Rust/Go interfaces
- Self-documenting in the language

#### Cons
- Significant language addition (traits are complex)
- Potential performance overhead (indirect calls)
- Complex implementation in compiler
- May conflict with Stasis's "simple and explicit" philosophy

---

### Option C: Dynamic Host Function Registration

Allow hosts to register functions at runtime, with the compiler generating dynamic dispatch stubs.

#### How It Works

**Host-Side Registration (C API)**:
```c
#include "stasis_host.h"

// Define function implementations
int my_init_window(int w, int h, const char* title) {
    return SDL_CreateWindow(...) ? 1 : 0;
}

void my_draw_sprite(int id, float cx, float cy, ...) {
    // Custom rendering
}

int main() {
    StasisHost* host = stasis_host_create();

    // Register functions by name and signature
    stasis_host_register(host, "init_window", "(i32,i32,*u8)->i32", my_init_window);
    stasis_host_register(host, "draw_sprite", "(i32,f32,f32,f32,f32,f32,f32,f32,f32,f32)->void", my_draw_sprite);

    // Load and run Stasis program
    StasisProgram* prog = stasis_load(host, "game.dll");
    int result = stasis_call(prog, "main");

    stasis_program_free(prog);
    stasis_host_free(host);
    return result;
}
```

**Stasis Source (with declarations)**:
```stasis
// Declare expected host functions
extern function init_window(w: i32, h: i32, title: utf8[]): bool;
extern function draw_sprite(id: i32, cx: f32, cy: f32, ...): void;

function main(): i32 {
    if !init_window(800, 600, "Game") {
        return 1;
    }
    // ...
}
```

#### Implementation Changes

| Component | Change Required |
|-----------|-----------------|
| Language | Add `extern function` declaration syntax |
| Compiler | Generate indirect call stubs through function table |
| Codegen | Emit function table lookups instead of direct calls |
| Runtime | New `stasis_host.h` API, function table management |
| Loader | Extended `stasis_runner` with registration support |

#### Pros
- Maximum flexibility for embedders
- No compiler changes needed for new functions
- Clean embedding API for game engines
- Enables complete decoupling of Stasis from SDL

#### Cons
- Runtime overhead (function pointer dispatch)
- Late binding means runtime errors instead of compile-time
- Complex host-side setup
- Need robust error handling for missing functions

---

### Option D: WebAssembly-Style Import/Export Model

Adopt WASM's explicit import/export model where Stasis modules declare their imports and exports with explicit module namespaces.

#### How It Works

**Stasis Source**:
```stasis
// Explicit imports from host modules
import "env" {
    function init_window(w: i32, h: i32, title: *u8): i32;
    function draw_sprite(id: i32, cx: f32, cy: f32, ...): void;
}

import "audio" {
    function play_sound(id: i32): void;
    function set_volume(vol: f32): void;
}

// Exports visible to host
export function game_update(dt: f32): void {
    // ...
}

export function game_render(): void {
    // ...
}
```

**Host-Side Module Provision**:
```c
StasisImports imports = {
    .env = {
        .init_window = sdl_init_window,
        .draw_sprite = sdl_draw_sprite,
    },
    .audio = {
        .play_sound = fmod_play_sound,
        .set_volume = fmod_set_volume,
    },
};

StasisModule* mod = stasis_instantiate("game.wasm", &imports);
stasis_call(mod, "game_update", 0.016f);
```

#### Implementation Changes

| Component | Change Required |
|-----------|-----------------|
| Language | Add `import "module" { ... }` syntax |
| Compiler | Parse import declarations, namespace management |
| Codegen | Generate module-namespaced symbol references |
| Runtime | Module instantiation with import resolution |
| CLI | New `--emit-wasm` for actual WASM output (optional) |

#### Pros
- Industry-standard model (WASM compatibility path)
- Clear module boundaries
- Enables actual WASM compilation target
- Explicit dependencies, easy to analyze
- Natural fit for sandboxed execution

#### Cons
- More verbose than current approach
- Requires updating all existing samples
- Module namespace management adds complexity
- May be overkill for single-host scenarios

---

### Option E: Hybrid Layered Approach

Combine multiple approaches into a layered architecture where each layer adds capability:

```
┌─────────────────────────────────────────────────────────────────┐
│  Layer 3: Stasis Standard Library                               │
│  High-level wrappers (gfx_*, audio_*, net_*)                    │
│  Written in Stasis, calls Layer 2                               │
├─────────────────────────────────────────────────────────────────┤
│  Layer 2: Host Traits (Capability Interfaces)                   │
│  HostGraphics, HostAudio, HostInput, HostFilesystem             │
│  Compiler-verified, optional traits                             │
├─────────────────────────────────────────────────────────────────┤
│  Layer 1: Core Host ABI (Minimal Contract)                      │
│  Memory allocation, entry point, panic handler                  │
│  Required by all hosts                                          │
├─────────────────────────────────────────────────────────────────┤
│  Layer 0: Platform (Provided by Host)                           │
│  Windows DLL, Android JNI, WASM imports, etc.                   │
└─────────────────────────────────────────────────────────────────┘
```

#### How It Works

**Layer 0: Platform-Specific Loading**
- Windows: DLL via LoadLibrary
- Android: JNI via System.loadLibrary
- WASM: Import/export tables
- Embedded: Linked directly

**Layer 1: Core Host ABI**
```c
// Every host MUST provide these
typedef struct StasisCoreHost {
    // Memory (for future dynamic features)
    void* (*alloc)(size_t size);
    void (*free)(void* ptr);

    // Lifecycle
    void (*on_panic)(const char* message);
    int (*on_ready)(void);

    // Debug (optional)
    void (*log)(int level, const char* message);
} StasisCoreHost;

// Compiler generates calls to these core functions
```

**Layer 2: Capability Traits**
```stasis
// stdlib/host/graphics.stasis
trait HostGraphics {
    extern function gfx_init(w: i32, h: i32, title: utf8[]): bool;
    extern function gfx_present(): void;
    // ...
}

// Programs declare which capabilities they need
@require HostGraphics
@require HostInput
function main(): i32 {
    // ...
}
```

**Layer 3: Standard Library**
```stasis
// stdlib/graphics.stasis
import host::HostGraphics;

// High-level API wrapping trait functions
function create_window(title: utf8[], width: i32, height: i32): bool {
    return gfx_init(width, height, title);
}

function sprite_draw(sprite: Sprite, x: f32, y: f32) {
    gfx_draw_sprite(sprite.id, x, y, sprite.scale, sprite.scale,
                    sprite.rotation, 1.0, 1.0, 1.0, 1.0);
}
```

#### Implementation Changes

| Component | Change Required |
|-----------|-----------------|
| Language | `trait`, `extern function`, `@require` attribute |
| Compiler | Trait system, capability checking, ABI code generation |
| Codegen | Layered call generation, optional vtables |
| Runtime | Core host struct, capability registration |
| Stdlib | Restructure into capability modules |

#### Pros
- Incremental adoption (each layer independent)
- Maximum flexibility (pick your abstraction level)
- Clear upgrade path from current state
- Supports all target platforms
- Testing-friendly (mock at any layer)

#### Cons
- Highest implementation complexity
- More concepts to learn
- Risk of over-engineering
- Longer time to full implementation

---

## Comparison Matrix

| Criterion | A: Manifests | B: Traits | C: Dynamic | D: WASM-Style | E: Hybrid |
|-----------|:------------:|:---------:|:----------:|:-------------:|:---------:|
| **Implementation Effort** | Low | High | Medium | Medium | Very High |
| **Runtime Overhead** | None | Low | Medium | Low | Low |
| **Compile-Time Safety** | Medium | High | Low | High | High |
| **Embedding Flexibility** | Low | Medium | High | Medium | High |
| **Platform Portability** | Medium | High | High | Very High | Very High |
| **Backward Compatibility** | High | Medium | High | Low | High |
| **WASM Path** | None | Possible | None | Native | Possible |
| **Learning Curve** | Low | Medium | Medium | Medium | High |
| **Extensibility** | Medium | High | Very High | High | Very High |
| **Testing/Mocking** | Low | High | High | Medium | Very High |

### Scoring (1-5, higher is better)

| Criterion | Weight | A | B | C | D | E |
|-----------|--------|---|---|---|---|---|
| Must work for Android | 20% | 2 | 4 | 5 | 5 | 5 |
| Must work for Windows C wrapper | 20% | 5 | 4 | 4 | 4 | 5 |
| Minimal breaking changes | 15% | 5 | 3 | 4 | 2 | 4 |
| Future WASM support | 15% | 1 | 3 | 2 | 5 | 4 |
| Developer ergonomics | 15% | 4 | 4 | 3 | 3 | 4 |
| Implementation feasibility | 15% | 5 | 2 | 4 | 3 | 2 |
| **Weighted Score** | 100% | **3.4** | **3.4** | **3.8** | **3.7** | **4.1** |

---

## Platform-Specific Considerations

### Windows (C Wrapper)

**Current State**: Well-supported via DLL + `stasis_runner`

**Recommended Approach**:
- Continue DLL model
- Add formal host header (`stasis_host.h`) for embedders
- Generate C-compatible headers from Stasis exports

**Implementation Notes**:
```c
// stasis_host.h - generated or handwritten
typedef struct StasisExports {
    int (*main)(void);
    void (*game_update)(float dt);
    void (*game_render)(void);
} StasisExports;

// Host loads DLL and populates struct
StasisExports exports;
exports.main = (int(*)(void))GetProcAddress(dll, "main");
```

### Android (Java/Kotlin + JNI)

**Current State**: Not supported

**Recommended Approach**:
1. Compile Stasis to shared library (`.so`) via NDK clang
2. JNI bridge layer for host function callbacks
3. Kotlin/Java host implements capabilities

**Architecture**:
```
┌─────────────────────────────────────────────┐
│  Kotlin/Java Host                           │
│  ┌─────────────────────────────────────────┐│
│  │  StasisHost interface                    ││
│  │  - initWindow(w, h, title): Boolean     ││
│  │  - drawSprite(id, cx, cy, ...): Unit    ││
│  └─────────────────────────────────────────┘│
│                      ▲                       │
│                      │ JNI                   │
│                      ▼                       │
│  ┌─────────────────────────────────────────┐│
│  │  libstasis_bridge.so (C)                ││
│  │  - Converts JNI calls to C callbacks    ││
│  │  - Manages function pointer table       ││
│  └─────────────────────────────────────────┘│
│                      ▲                       │
│                      │ Function calls        │
│                      ▼                       │
│  ┌─────────────────────────────────────────┐│
│  │  libgame.so (Compiled Stasis)           ││
│  │  - Calls into function table            ││
│  └─────────────────────────────────────────┘│
└─────────────────────────────────────────────┘
```

**JNI Bridge Example**:
```c
// stasis_android_bridge.c
#include <jni.h>

static JNIEnv* g_env;
static jobject g_host;
static jmethodID g_initWindow;
static jmethodID g_drawSprite;

JNIEXPORT void JNICALL Java_com_example_StasisBridge_init(
    JNIEnv* env, jobject thiz, jobject host) {
    g_env = env;
    g_host = (*env)->NewGlobalRef(env, host);

    jclass hostClass = (*env)->GetObjectClass(env, host);
    g_initWindow = (*env)->GetMethodID(env, hostClass,
        "initWindow", "(IILjava/lang/String;)Z");
    g_drawSprite = (*env)->GetMethodID(env, hostClass,
        "drawSprite", "(IFFFFFFFFFF)V");
}

// Stasis calls this
int stasis_init_window(int w, int h, const char* title) {
    jstring jtitle = (*g_env)->NewStringUTF(g_env, title);
    jboolean result = (*g_env)->CallBooleanMethod(
        g_env, g_host, g_initWindow, w, h, jtitle);
    (*g_env)->DeleteLocalRef(g_env, jtitle);
    return result ? 1 : 0;
}
```

### WebAssembly (Browser/Node.js)

**Current State**: Not supported

**Recommended Approach**:
1. Add WASM as compilation target (via LLVM wasm32 or Cranelift)
2. Use WASM import/export for host functions
3. JavaScript host provides implementations

**Architecture**:
```javascript
// JavaScript host
const imports = {
    env: {
        stasis_init_window: (w, h, titlePtr) => {
            const title = readString(memory, titlePtr);
            canvas.width = w;
            canvas.height = h;
            document.title = title;
            return 1;
        },
        stasis_draw_sprite: (id, cx, cy, sx, sy, angle, r, g, b, a) => {
            ctx.save();
            ctx.translate(cx, cy);
            ctx.rotate(angle);
            ctx.scale(sx, sy);
            ctx.globalAlpha = a;
            ctx.drawImage(sprites[id], -0.5, -0.5, 1, 1);
            ctx.restore();
        },
    },
};

const { instance } = await WebAssembly.instantiate(wasmBytes, imports);
instance.exports.main();
```

### iOS (Swift/Objective-C)

**Current State**: Not supported

**Recommended Approach**:
1. Compile to static library (`.a`) via clang
2. Swift/Obj-C wrapper with protocol for host
3. Similar pattern to Android but with Swift interop

**Swift Host Protocol**:
```swift
@objc protocol StasisHost {
    func initWindow(width: Int32, height: Int32, title: String) -> Bool
    func drawSprite(id: Int32, cx: Float, cy: Float,
                    sx: Float, sy: Float, angle: Float,
                    r: Float, g: Float, b: Float, a: Float)
    func isKeyDown(scancode: Int32) -> Bool
}

class GameViewController: UIViewController, StasisHost {
    override func viewDidLoad() {
        super.viewDidLoad()
        stasis_set_host(Unmanaged.passUnretained(self).toOpaque())
        stasis_main()
    }

    func initWindow(width: Int32, height: Int32, title: String) -> Bool {
        // Set up Metal view
        return true
    }
}
```

### Game Engine Embedding (Unity, Godot, Unreal)

**Current State**: Not supported

**Recommended Approach**:
1. Compile Stasis as native plugin
2. Engine provides host functions
3. Stasis handles game logic, engine handles rendering/physics

**Unity C# Example**:
```csharp
public class StasisBridge : MonoBehaviour
{
    [DllImport("game")]
    private static extern int stasis_main();

    [DllImport("game")]
    private static extern void stasis_game_update(float dt);

    // Host function callbacks - registered via delegate
    private delegate int InitWindowDelegate(int w, int h, IntPtr title);

    [AOT.MonoPInvokeCallback(typeof(InitWindowDelegate))]
    static int InitWindow(int w, int h, IntPtr title) {
        Screen.SetResolution(w, h, false);
        return 1;
    }

    void Start() {
        stasis_register_init_window(InitWindow);
        stasis_main();
    }

    void Update() {
        stasis_game_update(Time.deltaTime);
    }
}
```

---

## Implementation Phases

### Phase 1: Foundation (Weeks 1-4)

**Goal**: Establish core host abstraction without breaking existing code.

#### 1.1 Define Core Host ABI
```c
// runtime/stasis_host.h
#ifndef STASIS_HOST_H
#define STASIS_HOST_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

// Version for compatibility checking
#define STASIS_HOST_ABI_VERSION 1

// Core host structure - required by all hosts
typedef struct StasisHost {
    uint32_t abi_version;

    // Lifecycle callbacks
    void (*on_panic)(const char* file, int line, const char* message);
    void (*on_log)(int level, const char* message);

    // Entry point invocation
    int (*run_main)(void);
    int (*run_tests)(void);
} StasisHost;

// Host creation/destruction
StasisHost* stasis_host_create(void);
void stasis_host_destroy(StasisHost* host);

// Function registration
typedef enum {
    STASIS_TYPE_VOID = 0,
    STASIS_TYPE_I32 = 1,
    STASIS_TYPE_F32 = 2,
    STASIS_TYPE_PTR = 3,
    // ...
} StasisType;

int stasis_host_register_function(
    StasisHost* host,
    const char* name,
    void* func_ptr,
    StasisType return_type,
    int param_count,
    StasisType* param_types
);

// Program loading
typedef struct StasisProgram StasisProgram;
StasisProgram* stasis_load_program(StasisHost* host, const char* path);
void stasis_unload_program(StasisProgram* prog);

// Execution
int stasis_call_main(StasisProgram* prog);
int stasis_call_tests(StasisProgram* prog);

#ifdef __cplusplus
}
#endif

#endif // STASIS_HOST_H
```

#### 1.2 Add `extern` Keyword to Language
```stasis
// New syntax for declaring external functions
extern function init_window(w: i32, h: i32, title: utf8[]): bool;
extern function draw_sprite(id: i32, cx: f32, cy: f32, ...): void;

// Can still be called normally
function main(): i32 {
    init_window(800, 600, "Game");
    return 0;
}
```

#### 1.3 Compiler Changes
- Add `extern` keyword to lexer/parser
- Create `ExternFunctionDeclaration` AST node
- Validate extern calls against declarations
- Generate appropriate symbol references in codegen

#### 1.4 Maintain Backward Compatibility
- Keep current built-in function recognition
- Treat undecorated calls as implicit externs (with warning)
- Provide migration guide

### Phase 2: Platform Abstraction (Weeks 5-8)

**Goal**: Enable multiple host implementations.

#### 2.1 Refactor Graphics Runtime
```c
// Split stasis_graphics.c into:

// stasis_graphics_interface.h - Abstract interface
typedef struct StasisGraphicsHost {
    int (*init_window)(int w, int h, const char* title);
    void (*begin_frame)(void);
    void (*end_frame)(void);
    void (*clear)(float r, float g, float b, float a);
    int (*load_sprite)(const char* path);
    void (*draw_sprite)(int id, float cx, float cy, ...);
    // ...
} StasisGraphicsHost;

// stasis_graphics_sdl.c - SDL/OpenGL implementation
StasisGraphicsHost* stasis_graphics_create_sdl(void);

// stasis_graphics_null.c - Headless implementation
StasisGraphicsHost* stasis_graphics_create_null(void);
```

#### 2.2 Create Platform-Specific Loaders
```
runtime/
  ├── stasis_host.h              # Common interface
  ├── stasis_host_common.c       # Shared implementation
  ├── platforms/
  │   ├── windows/
  │   │   └── stasis_loader_win.c
  │   ├── linux/
  │   │   └── stasis_loader_linux.c
  │   ├── android/
  │   │   ├── stasis_loader_android.c
  │   │   └── StasisBridge.kt
  │   └── wasm/
  │       └── stasis_loader_wasm.js
  └── graphics/
      ├── stasis_graphics_sdl.c
      ├── stasis_graphics_null.c
      └── stasis_graphics_metal.m   # Future
```

#### 2.3 Build System Updates
- CMake presets for each platform
- Cross-compilation toolchains
- CI/CD for multi-platform testing

### Phase 3: Capability System (Weeks 9-12)

**Goal**: Compile-time capability checking.

#### 3.1 Add Host Manifests
```toml
# hosts/sdl-desktop.toml
[host]
name = "stasis-sdl-desktop"
version = "1.0.0"

[capabilities]
graphics = true
audio = false
filesystem = true
input.keyboard = true
input.mouse = false

[[functions]]
name = "init_window"
signature = "(i32, i32, *utf8) -> bool"
capability = "graphics"
```

#### 3.2 Compiler Integration
```csharp
// New compiler option
public class CompilerOptions {
    public string? HostManifest { get; set; }  // --host path/to/host.toml
}

// Validate extern calls against manifest
public class ExternValidator {
    public void Validate(ExternFunctionCall call, HostManifest manifest) {
        if (!manifest.HasFunction(call.Name)) {
            Error($"Host '{manifest.Name}' does not provide '{call.Name}'");
        }
        if (!manifest.HasCapability(call.RequiredCapability)) {
            Error($"Host '{manifest.Name}' lacks capability '{call.RequiredCapability}'");
        }
    }
}
```

#### 3.3 Stasis Source Annotations
```stasis
@require_capability("graphics")
@require_capability("input.keyboard")
function main(): i32 {
    // ...
}
```

### Phase 4: Advanced Features (Weeks 13-20)

**Goal**: Full platform support and advanced use cases.

#### 4.1 WASM Compilation Target
- Add LLVM `wasm32-unknown-unknown` target
- Implement WASM import/export generation
- JavaScript loader library

#### 4.2 Android Full Support
- NDK build integration
- Kotlin/Java bindings
- Sample Android app

#### 4.3 Game Engine Integration
- Unity plugin template
- Godot GDNative module
- Documentation and examples

#### 4.4 Host Versioning
```stasis
@require_host("stasis-sdl", version = ">=1.2.0")
```

---

## Recommendations

### For Immediate Implementation (Recommended Path)

Based on the analysis, I recommend a **phased hybrid approach** combining the best elements:

1. **Start with Option A (Manifests)** for immediate value with minimal changes
2. **Add extern keyword (from Option C)** for explicit declarations
3. **Evolve toward Option E (Hybrid)** as the full vision

### Specific Recommendations

| Priority | Recommendation | Rationale |
|----------|----------------|-----------|
| **High** | Add `extern function` syntax | Explicit is better than implicit; enables validation |
| **High** | Create `stasis_host.h` header | Standardizes embedding API for all platforms |
| **High** | Split graphics into interface + impl | Enables headless testing and alternative renderers |
| **Medium** | Host capability manifests | Documents contracts, enables static analysis |
| **Medium** | Android JNI bridge | Opens mobile market |
| **Medium** | WASM compilation target | Web deployment, sandboxed execution |
| **Low** | Full trait system | Complex, defer until other features stable |
| **Low** | Hot-swap hosts at runtime | Nice-to-have, significant complexity |

### Suggested First Steps

1. **Week 1**: Define and implement `stasis_host.h` C interface
2. **Week 2**: Add `extern function` syntax to language
3. **Week 3**: Refactor `stasis_graphics.c` into interface + SDL implementation
4. **Week 4**: Create null/headless graphics host for testing
5. **Week 5-6**: Create host manifest format and compiler validation
6. **Week 7-8**: Android proof-of-concept with JNI bridge

---

## Appendix: Reference Implementations

### A. WASM Import/Export (Industry Standard)

WebAssembly's approach is the most battle-tested:

```wat
;; Imports declared in module
(import "env" "memory" (memory 1))
(import "env" "print" (func $print (param i32)))

;; Exports from module
(export "main" (func $main))
(export "update" (func $update))
```

### B. Lua C API (Embeddable Language)

Lua's approach prioritizes embedding simplicity:

```c
// Host registers functions
lua_register(L, "my_print", l_my_print);

// Or via table
lua_newtable(L);
lua_pushcfunction(L, l_draw_sprite);
lua_setfield(L, -2, "draw_sprite");
lua_setglobal(L, "gfx");
```

### C. Zig's `@extern` and LinkMode

Zig provides explicit control over linking:

```zig
extern "c" fn printf(format: [*:0]const u8, ...) c_int;

export fn main() void {
    // Exported symbol
}
```

### D. Rust FFI with `extern "C"`

Rust's explicit ABI declarations:

```rust
extern "C" {
    fn init_window(w: i32, h: i32, title: *const u8) -> bool;
}

#[no_mangle]
pub extern "C" fn game_update(dt: f32) {
    // Exported to C
}
```

---

## Conclusion

Making hosts a first-class concept in Stasis is essential for:

- **Cross-platform deployment** (mobile, web, desktop)
- **Engine integration** (Unity, Godot, custom engines)
- **Testing and development** (mock hosts, headless mode)
- **Long-term maintainability** (clear contracts, versioning)

The recommended phased approach minimizes disruption while building toward a flexible, powerful host system. Starting with `extern function` syntax and the `stasis_host.h` API provides immediate value, with manifests and capability traits as natural extensions.

The investment pays dividends in every new platform, every embedding scenario, and every test suite that benefits from the clear host abstraction.
