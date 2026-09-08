# Type and compile-time value parameters

Status: proposed compiler design, not implemented language syntax. This document
does not amend the supported-language contract in spec.md. No implementation
tasks are created by this proposal.

## Objective and representative use

Support reusable fixed-storage collections and shared-library algorithms with
type parameters and compile-time integer parameters. Keep persistent storage,
allocation, bounds behavior, and tick semantics explicit.

```stasis
struct Buffer<T: type, N: i32> {
    count: i32;
    values: T[N];
}

struct Rig2D<N: i32> {
    count: i32;
    bones: RigBone2D[N];
}

global samples: Buffer<f32, 128>;
global courier: Rig2D<24>;
global boss: Rig2D<64>;

function clear<T: type, N: i32>(self: Buffer<T, N>): void {
    self.count = 0;
}

function capacity<T: type, N: i32>(self: Buffer<T, N>): i32 {
    return N;
}

// Type inference binds T=f32 and N=128 from the receiver.
function reset_samples(): void {
    samples.clear();
}
```

The representative compilation path is Buffer<f32,128> -> a concrete struct
layout with the existing fixed-array header/payload -> clear<f32,128> with a
concrete receiver view -> ordinary HIR -> JIT, AOT, or Wasm. A future pool of
entities should follow the same path with an entity type and another capacity.

The number of active elements remains a user-declared count. This feature does
not add array .length, allocation, resizing, or new element-copy semantics.

## Parameter kinds and syntax

Proposed declaration grammar:

```text
generic-parameters := '<' generic-parameter (',' generic-parameter)* '>'
generic-parameter  := identifier ':' ('type' | 'i32')
type-application   := qualified-name '<' generic-argument (',' generic-argument)* '>'
explicit-call     := callee '::<' generic-argument (',' generic-argument)* '>' '(' arguments ')'
```

`type` in this position denotes a type parameter. It is not a runtime type value
or a new data representation. `N: i32` denotes a compile-time value parameter;
every parameter in a generic parameter list is resolved during compilation.
No `const` keyword is required. Ordinary `(n: i32)` function parameters retain
their existing runtime meaning. Assignment to a generic value parameter is an
error even inside a specialized function.

V1 permits parameter lists on structs and functions. Generic enums, aliases,
defaults, variadic parameters, higher-kinded types, and partially applied types
are deferred. Type arguments must be concrete supported types; substituting
void or a view into a stored field is rejected wherever existing storage rules
reject it. Value arguments are i32 only in v1. Other integer kinds, bool, enums,
strings, and floating-point value arguments require later explicit proposals.

Parameters have declaration-local identity and scope. A function explicitly
declares its parameters even when its receiver is generic; there is no implicit
capture of the struct declaration's parameter names. Duplicate parameter names
and collisions with ordinary parameter names are rejected.

In type positions, use Buffer<f32,128> and allow nested applications. In
expression positions, explicit calls use clear::<f32,128>(samples) or
samples.clear::<f32,128>(). The `::` marker avoids reinterpreting existing `<`
and `>` comparison expressions through name lookup or speculative token scans.
Ordinary inferred calls, including samples.clear(), remain the common form.

The parser owns delimiter handling, including adjacent closing `>` tokens in
nested types. It must preserve existing shift/comparison parsing in expressions.
No formatter, indexer, or backend may independently guess angle-bracket syntax.

## Compile-time evaluation

Value arguments may contain integer literals, named constants, enclosing generic
value parameters, parentheses, unary plus/minus, and +, -, *, /, % expressions.
No function calls, mutable globals, host queries, or runtime values are allowed
in v1. Reuse and extend one typed constant evaluator for generic arguments and
array extents. Existing nongeneric constant behavior must not change silently.

Evaluate with checked signed i32 arithmetic; overflow, division/remainder by
zero, MIN / -1, and invalid literals report errors at the expression. Division
truncates toward zero. Evaluation is host-independent. Detect constant-reference
cycles and bound expression depth/work, with an actionable diagnostic.

```stasis
const COURIER_CAPACITY: i32 = 24;
global a: Rig2D<24>;
global b: Rig2D<COURIER_CAPACITY>;
global c: Rig2D<12 + 12>;
// a, b, c have the same concrete type.
```

An i32 parameter is not inherently a capacity: negative values may be useful
for fixed offsets or modes. Array extents must independently satisfy the
canonical fixed-array extent policy after substitution, and checked layout
arithmetic must reject unrepresentable sizes. Preserve existing zero-capacity
rules; verify and document them in the extent implementation slice rather than
inventing a different generic-only policy.

Symbolic expressions such as T[N + 1] may occur in templates. Evaluate them
after substitution. V1 does not prove identities over symbolic expressions or
solve arithmetic equations to infer N.

## Identity, layout, and storage

Concrete nominal type identity is the defining declaration identity plus an
ordered list of kind-tagged canonical arguments. Rig2D<24> differs from
Rig2D<64> even if an optimization happens to erase all capacity-dependent work.
Buffer<i32,24> differs from Buffer<f32,24>. Equal evaluated arguments intern to
one concrete type; spelling and constant-expression source text are not identity.

The current frontend TypeId is u16 and TypeKey includes Named(String), fixed
arrays, and views. Introduce a structured instantiated-nominal key rather than
encoding applications as unparsed names. TypeId remains a compilation-local
handle; persistent cache/layout keys use canonical semantic identities, never
allocation order or unstable numeric TypeIds. Type interning must report
exhaustion before any narrowing conversion can wrap.

Materialize concrete fields through the existing layout machinery, including
fixed-array headers, alignment, backing/provenance, and state inspection.
Generic structs get the same layout rules as equivalent handwritten concrete
structs. Reject recursive by-value containment with a field/type path diagnostic.
Separate instances stored in distinct globals or fields have distinct backing.

Generic syntax does not change Stasis reference/view semantics (spec sections
5 and 7). Passing a Buffer<T,N> is a view of its existing storage. A local alias
or parameter is not a deep copy, and no local owning temporary is implied.
Struct/array returns retain the requirement to reference global-backed storage.
Explicit copy operations remain subject to the existing supported field/layout
rules. Correct the earlier informal expectation that owning fields imply value
copying: ownership describes where backing is declared, not how bindings copy.

## Functions, inference, and resolution

Infer generic arguments by structurally matching ordinary argument types against
parameter types. A Buffer<f32,128> receiver matches Buffer<T,N>, producing
T=f32 and N=128. Repeated occurrences must agree. Explicit arguments must supply
the complete generic list in v1 and must agree with ordinary arguments.

Match exact fixed-array extents before applying existing fixed-array-to-view
compatibility. T[] can infer T from a fixed array, but does not infer or capture
that array's capacity. A runtime view cannot infer N for T[N]. Matching T[N+1]
does not solve for N: require explicit arguments or infer N from another exact
occurrence, then check the expression. Inference does not use return context,
literal widening, or conversions to select among conflicting bindings in v1.

Keep receiver-based overload resolution and existing arity restrictions. Resolve
names under current module rules, collect structurally viable candidates, infer
arguments, and apply existing ordinary argument compatibility. A unique exact
nongeneric receiver candidate takes precedence over a generic fallback. Reject
ambiguous generic candidates; no ordering by declaration order or speculative
body execution. Overlapping generic receiver patterns in one visible family are
rejected conservatively in v1. Equivalent parameter renaming cannot create a
second overload. Diagnose concrete/generic arity conflicts under the same
receiver family rules as today's declarations.

## Operations on T and initial collection scope

V1 uses instantiation-time checking for operations that depend on T. Parse and
check template-independent names, syntax, duplicate declarations, and scopes
at declaration time. After substitution, run the ordinary semantic checks on
the entire specialized body, including both branches of runtime if statements.
An unused template is not a claim that its body works for every type.

```stasis
function push<T: type, N: i32>(self: Buffer<T, N>, value: T): bool {
    if (self.count < 0 || self.count >= N) {
        return false;
    }
    self.values[self.count] = value;
    self.count += 1;
    return true;
}
```

This works for scalar instantiations with supported assignment semantics. A
struct containing unsupported copy fields must get a substitution diagnostic;
generics must not manufacture memcpy or alias assignment to make push compile.
The initial library can expose clear/count/capacity and access to caller-backed
elements for broader types, plus push/copy for supported types. Before calling
a collection general-purpose, document and test its supported element shapes.

Similarly, an add<T> body using `+` succeeds only for types that already support
that operation. No traits, implicit duck-typed field contracts, new conversions,
or user-defined operator overloads are introduced. Existing field/function
resolution after substitution remains authoritative. A later constraints design
can make these requirements explicit without changing parameter kinds.

Template names resolve in their defining module; bindings are not captured from
the instantiating caller's imports. Dependent receiver calls resolve against
substituted types using the defining module's visibility. Diagnose failures with
both the template expression and the concrete instantiation/call chain. Do not
silently discard a candidate whose selected body fails to type-check.

## Compiler integration

1. Frontend indexer/parser and shared type-expression representation: record
   parameter kinds, source spans, type applications, constant expressions, and
   explicit call arguments. Use structured substitution, not text rewriting.
2. frontend/types.rs: intern concrete applications and expose generic definition
   identity, argument kinds, and substituted fields to semantic checking.
3. Compiler semantic analysis/data-flow: instantiate required signatures and
   bodies, resolve dependent calls, and preserve effects, read/write tracking,
   bounds/provenance, and diagnostics. Generic helpers obey existing effects.
4. A compiler-owned specialization worklist follows concrete calls/type uses
   from normal roots. Memoize by declaration plus canonical arguments. Recursive
   calls with the same key reuse the pending specialization; changing arguments
   can grow the worklist and must be bounded. Templates themselves are not
   executable backend functions.
5. backend/compile_analysis.rs, program_snapshot.rs, reachability.rs and
   state_layout.rs consume concrete artifacts. emit.rs, JIT, AOT, and Wasm use
   the same specialized typed input with ordinary fixed layouts and signatures.
   Backends must not separately implement substitution or inference.
6. Language service, semantic edit commands, formatter, and symbol index expose
   template definitions and concrete arguments without creating synthetic source
   files. Go-to-definition points to the template. Renaming generic parameters
   changes bound uses only. Symbol edits and import resolution remain atomic.

Every syntactically used concrete application is validated for argument kinds
and legal layout, including unused global declarations. Emit bodies only for
reachable specializations. Report generic-dependent errors when a specialization
is checked, using the same checking path in diagnostic and executable modes.

Use deterministic configurable limits shared by all backends, with initial
defaults of 128 nested instantiations and 4096 distinct specializations per
compilation. These are compiler resource limits, not a collection-capacity limit.
Also enforce the existing TypeId/layout limits and checked byte arithmetic.
Boundary tests must cover each limit, cycles, and expanding recursion such as
f<N>() calling f<N+1>(). Emit the request chain on exhaustion; never truncate
the generated program or fall back to runtime generic dispatch.

## Incremental compilation and live swapping

Cache specialization dependencies on the template body/signature, canonical
arguments, concrete field layouts, referenced constants, and called helpers.
Recheck changed files under the existing full-file authority rule. Reuse
unchanged concrete function artifacts only when their semantic dependencies are
unchanged. No textual substitution cache may bypass semantic invalidation.

Changing a constant's spelling from 24 to 12+12 leaves its canonical argument
unchanged; correct rechecking may still reuse generated artifacts. Changing it
to 32 changes type identity and potentially layout. Compare the actual resulting
state under the existing migration/compatibility rules; generic capacity changes
must never be assumed compatible just because the template name is unchanged.

Preserve transactional candidate compilation and all-or-nothing commit. Failed
instantiation or layout rejection leaves running code and state intact, as does
on_code_swap failure. The current spec uses direct internal calls within a
generation: specialization does not introduce permanent host-held function
pointers. Lifecycle/extern/host-required declarations must be concrete in v1;
use concrete wrappers around generic helpers where necessary.

## Acceptance fixtures and delivery slices

These slices are planning units for later tasks, not implementation completion.
Each feature slice must run a concrete fixture through the production pipeline;
parser-only success cannot establish support.

| Slice | Deliverable | Executable acceptance |
| --- | --- | --- |
| 1 | N:i32 structs/functions, constant evaluation, concrete identity | Fixed i32 storage at capacities 4/8; inferred and explicit calls; JIT/AOT/Wasm |
| 2 | T:type with mixed parameters and existing copy/view semantics | Buffer<i32,4>, Buffer<f32,8>, nested supported struct elements; generic receiver helpers |
| 3 | Diagnostics, effects, tooling, cache and swap closure | Definition navigation, atomic edits, selective reuse, failed-instantiation/swap preservation |
| 4 | Reviewed reusable stdlib consumers | Bounded buffer/pool helpers and Rig2D<N> with documented supported element operations |

All slices include negative coverage as they introduce behavior; slice 3 closes
cross-surface coverage and is required before advertising the feature generally.
Task #532 remains a separate prerequisite for exercising receiver-owned nested
struct arrays; passing scalar-array generics does not prove that path. The
existing rigging extraction can retain capacity 24 until this proposal lands.

The acceptance matrix must include:

- Kind/arity errors: Buffer<24, f32>, missing arguments, runtime values, unbound T,
  duplicate parameters, writes to N, invalid storage types.
- Constants: equal expressions intern identically; overflow and division by zero;
  cycles; negative offset accepted as a value but invalid array extent rejected.
- Types: distinct modules with identically spelled declarations do not collide;
  nested generic types; illegal recursive layouts; unknown types; layout overflow.
- Calls: inference from receiver and fixed arrays; conflicting arguments; view
  capacity cannot infer N; explicit calls coexist with comparisons and shifts;
  ambiguous/overlapping patterns produce stable diagnostics.
- Semantics: scalar copies, supported struct views, unsupported composite copies,
  nested receivers, two independent owners, count/full/empty behavior, effect
  violations, and definition-site name lookup.
- Bounds: first/last valid index and invalid neighbors, through fixed arrays,
  views, and nested generic fields; retain traps unless compiler-proven safe.
- Backends: identical asserted results from JIT, linked AOT, and executed Wasm;
  specialized nested calls and recursion; matching concrete state layout reports.
- Incremental/swap: template-body edit invalidates affected instantiations only;
  unchanged helpers reuse artifacts; constant/layout changes tracked transitively;
  rejected candidates and failing swap hooks preserve old code and data.
- Resource policy: near-limit success and deterministic failure for excessive
  instantiations, type identifiers, depth, constant evaluation, and layout size.

Use repository-prescribed Cargo cache wrapping and named owning test targets.
Exact test target names should be established by each implementation slice;
final release gating uses tools/validate_repo.sh plus the applicable Web and
language-service acceptance suites. This document does not claim those new
fixtures or generic syntax have been implemented or tested.

## Design decisions for review

The principal commitment is that generic argument position makes an i32 value
compile-time: const is enforced by checking, while type explicitly distinguishes
type parameters. Explicit expression calls use ::<> for a predictable grammar.
Instantiation-time checks make basic shared storage practical before a traits
system, with clear diagnostics and documented element-operation limits.

Deferred decisions are richer constraints/traits, generic enums, defaults (for
example Rig2D defaulting to 24), const functions, additional value kinds,
specialization overrides, and generic callback/function values. They are not
required to deliver the initial bounded collections and rigging use cases.

Theory gained: existing fixed-array layout and reference/view rules already
define concrete generic instances; the new compiler responsibility is binding,
checking, and caching those instances before ordinary backend lowering. A
second library can reuse that mechanism without introducing allocation or a
different ownership model.

Visual evidence: not applicable to this compiler design document.
