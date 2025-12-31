# Stasis - A Deterministic Game Language and Runtime

This document is a public-facing overview of Stasis (v0).

## What Stasis Is

Stasis is a statically allocated, deterministic programming language and runtime designed for building 2D games and simulations with extremely fast iteration times.

Stasis prioritizes:

- Predictable performance
- Fast reload loops
- Explicit memory layout
- Data-driven gameplay
- Simple, inspectable systems

Stasis is not a general-purpose engine and does not attempt to compete with Unity or Unreal on features. It is a focused tool for developers who want control, clarity, and speed.

## Core Design Principles

### 1. Deterministic by Default

- No garbage collection
- No hidden allocations
- Fixed memory layout
- Identical behavior across runs and machines

This makes Stasis suitable for:

- Simulations
- Replays
- Lockstep or rollback networking
- Tight gameplay tuning

### 2. Static Memory Model

All memory is allocated at compile time or load time.

Key properties:

- Structs have fixed layout
- Arrays have fixed capacity
- Strings are fixed-size (current toolchain focuses on ASCII/byte-string workflows)
- No runtime heap allocation

Internally, Stasis lowers ergonomic AoS-style code into SoA-friendly layouts for cache efficiency, without exposing this complexity to the user.

### 3. Fast Iteration Loops

Stasis is built around rapid iteration:

- Warm runs should avoid relinking when nothing changed (artifact cache)
- Full optimized builds are explicit

The runtime supports tick-hosted workflows and hot-swap between ticks for gameplay tuning without restarting the process.

### 4. Data-Driven Gameplay

Game logic is designed to be:

- Table-driven
- Reloadable at runtime

Typical tuning workflows involve modifying data, not code.

### 5. Narrow Scope

Stasis intentionally focuses on:

- 2D rendering
- Deterministic update loops
- Explicit state machines
- Simple platform abstraction

Stasis does not include:

- Visual editors
- Scene graphs
- Physics engines
- Scripting layers
- Asset stores

These can be layered externally if desired.

## Typical Use Cases

Stasis is a good fit for:

- Indie 2D games
- Tower defense, strategy, simulation games
- Tools that embed gameplay-like logic
- Educational or training simulations
- Projects where iteration speed matters more than features

Stasis is not a good fit for:

- AAA production pipelines
- Heavy 3D rendering
- Content-authoring-heavy teams
- Projects requiring large third-party ecosystems

## Example Workflow (Windows, current repo)

From the repo root:

```bat
build.bat
test.bat
```

Run an interactive demo (graphics/input/audio/text):

```bat
.\stasis.bat run .\samples\interactive_showcase.stasis --backend llvm --graphics
```

For more runnable examples and commands, see `docs/demo-day.md`.

## Stability Contract (v1, draft)

The following are intended to be stable once v1 is released:

- Memory model and layout rules
- Struct, array, enum semantics
- Fixed-size string behavior
- Deterministic update loop contract
- Platform abstraction surface

Experimental features should be clearly labeled. Breaking changes should only occur in major versions with migration notes.

## Licensing (draft)

Proposed trust-based license outline:

### Personal / Indie

- Free until $25,000 USD in revenue
- After threshold: 1% of revenue
- Annual self-reporting

### Commercial

- 1% revenue share
- Optional flat-fee buyout
- Priority support and versioned releases

### Enterprise / Internal Use

- Flat annual license
- No revenue tracking
- Contractual support terms

There is no DRM. Compliance is enforced socially and contractually.

## Support (draft)

### Free Users

- Documentation
- Examples
- Public issue tracker (best effort)
- Community discussion channels

### Licensed Users

- Guaranteed response times (48-72h)
- Versioned releases
- Migration guides
- Private issue queue

Support covers Stasis behavior, not application debugging or custom feature development.

## Project Status

Stasis is under active development and is currently suitable for:

- Internal projects
- Experimental games
- Early adopters comfortable with evolving tooling

Public v1 will be announced once the stability contract is finalized.

## Philosophy

Stasis is built on the belief that:

- Most games do not need complex engines
- Fast iteration beats large feature sets
- Explicit systems are easier to reason about
- Determinism is a feature, not a constraint

Stasis favors boring, predictable systems over clever abstractions.
