# Compiler-Owned Symbol Identity

Stasis has one declaration identity contract. The compiler creates a lossless `SymbolId` and a
compact `FnId`; hosts, JIT/AOT artifacts, snapshots, hot-swap patches, and Workshop tooling consume
those values rather than allocating IDs or deriving keys from vector positions.

## Canonical form

Function identity is `v1|function|<project-relative source path>|<qualified name>|<overload discriminator>`.
The `v1` prefix versions the persisted contract. Paths use `/`, remove `.` and bounded lexical `..`
components, and remain case-sensitive on every host. Compiler process APIs accept an explicit
absolute project root so an absolute host path and its project-relative spelling produce the same
identity. Absolute paths without a root, paths outside the root, leading `..`, UNC paths, empty paths,
and ambiguous rooted components are deterministic errors. Components escape reserved delimiters.
The overload discriminator is the ordered receiver/parameter type list. The compiler
derives the 32-bit `FnId` from the canonical text with FNV-1a and rejects any digest collision that
maps two different `SymbolId` values in one program.

`FnId` is identity, not storage. A compiler may use a dense `FunctionStorageIndex` internally, but
that index must never cross a snapshot, graph, artifact, manifest, host, runtime, or tooling boundary.

## Edit and compatibility rules

- Body, whitespace, comment, source-position, declaration-order, unrelated declaration, and return
  type edits retain `SymbolId`/`FnId`.
- A receiver or parameter type edit replaces identity because it changes overload selection.
- Rename, file/module move, add, and delete respectively replace, move to a new identity, add, or
  remove an identity.
- `signature_hash` is separate and includes the complete receiver/parameter/return contract. A
  retained identity with a changed signature hash is an incompatible hot-swap candidate unless the
  caller/host contract is rebuilt and validated. `body_hash` controls code reuse only.

Bare names exist only as host ABI aliases (`main`, `tick`, `render`, `on_code_swap`). Alias resolution
must select exactly one canonical identity; zero or multiple matches are deterministic diagnostics.
Artifacts and code pointers remain keyed by `FnId` even when an alias is published.

Workshop source items, AI selections, semantic edit plans, receipts, and Android artifact manifests
persist the canonical string. Semantic edit schema v2 requires `target.symbol_id`; schema v1 remains
a compatibility reader and its legacy name/kind/file/owner/signature tuple succeeds only when it
selects one declaration. Structured artifact manifests use JSON so source names cannot corrupt row
boundaries.

Task #182 will define the canonical module/import graph. Until then identity normalizes the compiler's
project-relative source path without inventing module semantics.
