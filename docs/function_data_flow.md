# Function data-flow summaries

Stasis compiler index results include a schema-versioned summary for every function. The artifact
uses the same structured statements as lowering, so editor and build tooling do not need a separate
source-shape detector.

Each summary contains deterministic source identity plus `direct` and `aggregate` effects:

- `reads` and `writes` use explicit state paths. Indexed elements use `[*]`, so field subsets such
  as `state.enemies[*].hp` remain distinct from neighboring fields.
- `parameter_reads` and `parameter_writes` describe view/reference parameters by source name. When
  one function calls another, the aggregate summary substitutes those effects onto the caller's
  concrete state path for both receiver and function-form calls.
- `calls` lists Stasis callees. `host_calls` lists extern or host-resolved calls.
- `bounded_iterations` records `for` conditions and `foreach` collection bounds. A proven static
  maximum is included when the compiler can derive one; otherwise it is `null` rather than guessed.
- `aggregate` includes effects from the transitive Stasis call graph and is cycle-safe.

Function identity uses a project-relative file path in CLI output, body source offsets, and a
lossless 16-digit hexadecimal signature hash. Structured statements are cached by function body and
shared with lowering, so unchanged functions are not reparsed to produce summaries.

Language tooling can consume `Compiler::function_data_flow_summaries`. Runtime-backed tools can use
`JitProcess::function_data_flow_summaries`. `stasis inspect --json` exposes the same values under
`function_data_flow`, while human output provides a compact direct-effect view.
