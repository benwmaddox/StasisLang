# Runtime compatibility

Stasis source-language behavior, the standard library, and compiler-facing APIs are the game developer compatibility surface.

Generated command buffers, host bindings, renderer internals, packaged provenance, and other post-compilation artifacts are evergreen and current-only while Stasis is in alpha. They must be rebuilt and shipped together from the same toolchain revision. The render command header keeps an explicit version field so a mixed build fails immediately with an actionable incompatible-version diagnostic; it is not a backward-compatibility switch.

Command trace values and generated evidence schemas or values are current-build diagnostics, not cross-release compatibility promises. Tests may assert structural semantic relationships within one build, such as nonzero traces, stable traces for an unchanged scene, or changed traces after a semantic input change; they must not freeze generated numeric evidence across releases.

Downstream hosts accept only the canonical current render version and exact canonical buffer capacities. When the render contract changes, update the compiler, generated artifacts, native/Web/Android hosts, fixtures, and package provenance in lockstep. Do not add compatibility branches for older downstream render schemas.
