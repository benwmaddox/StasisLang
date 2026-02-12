using Stasis.Compiler;

namespace Stasis.Compiler.IR;

public sealed record LowerResult(string Ir, IReadOnlyList<Diagnostic> Diagnostics);
