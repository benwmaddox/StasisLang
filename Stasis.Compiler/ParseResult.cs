using Stasis.Compiler.Syntax;

namespace Stasis.Compiler;

public sealed record ParseResult(CompilationUnitSyntax CompilationUnit, IReadOnlyList<Diagnostic> Diagnostics);
