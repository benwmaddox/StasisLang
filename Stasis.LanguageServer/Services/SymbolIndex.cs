namespace Stasis.LanguageServer.Services;

using Stasis.Compiler.Syntax;

public sealed class SymbolIndex
{
    private readonly Dictionary<string, StructSymbol> _structs = new(StringComparer.Ordinal);
    private readonly Dictionary<string, EnumSymbol> _enums = new(StringComparer.Ordinal);

    public static SymbolIndex Build(CompilationUnitSyntax compilationUnit)
    {
        var index = new SymbolIndex();

        foreach (var decl in compilationUnit.Declarations)
        {
            switch (decl)
            {
                case StructDeclarationSyntax s:
                    index._structs[s.Name.Text] = new StructSymbol(
                        s.Name.Text,
                        s.Fields.Select(f => new StructFieldSymbol(
                            f.Identifier.Text,
                            TypeSyntaxToString(f.Type))).ToArray());
                    break;
                case EnumDeclarationSyntax e:
                    index._enums[e.Name.Text] = new EnumSymbol(
                        e.Name.Text,
                        e.Members.Select(m => m.Identifier.Text).ToArray());
                    break;
            }
        }

        return index;
    }

    public bool IsStruct(string name) => _structs.ContainsKey(name);
    public bool IsEnum(string name) => _enums.ContainsKey(name);

    public StructSymbol? GetStruct(string name) =>
        _structs.TryGetValue(name, out var s) ? s : null;

    public EnumSymbol? GetEnum(string name) =>
        _enums.TryGetValue(name, out var e) ? e : null;

    private static string TypeSyntaxToString(TypeSyntax type) =>
        type switch
        {
            NamedTypeSyntax named => named.Name,
            ArrayTypeSyntax arr when string.IsNullOrEmpty(arr.SizeText) => $"{TypeSyntaxToString(arr.ElementType)}[]",
            ArrayTypeSyntax arr => $"{TypeSyntaxToString(arr.ElementType)}[{arr.SizeText}]",
            _ => "unknown"
        };
}

public sealed record StructSymbol(string Name, IReadOnlyList<StructFieldSymbol> Fields);
public sealed record StructFieldSymbol(string Name, string TypeText);
public sealed record EnumSymbol(string Name, IReadOnlyList<string> Members);

