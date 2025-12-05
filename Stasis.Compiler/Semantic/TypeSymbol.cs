namespace Stasis.Compiler.Semantic;

public abstract record TypeSymbol(string Name);

public sealed record PrimitiveTypeSymbol(string PrimitiveName) : TypeSymbol(PrimitiveName);

public sealed record NamedTypeSymbol(string TypeName) : TypeSymbol(TypeName);

public sealed record ArrayTypeSymbol(TypeSymbol ElementType, int Size) : TypeSymbol($"{ElementType.Name}[{Size}]");

public sealed record VoidTypeSymbol() : TypeSymbol("void");
