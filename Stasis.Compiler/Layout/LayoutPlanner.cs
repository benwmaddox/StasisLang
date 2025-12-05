using Stasis.Compiler.Semantic;
using Stasis.Compiler.Syntax;

namespace Stasis.Compiler.Layout;

public sealed class LayoutPlanner
{
    private readonly CompilationUnitSyntax _compilationUnit;
    private readonly IReadOnlyDictionary<string, Symbol> _symbols;
    private readonly Dictionary<string, StructDeclarationSyntax> _structs;
    private int _offset;

    public LayoutPlanner(CompilationUnitSyntax compilationUnit, IReadOnlyDictionary<string, Symbol> symbols)
    {
        _compilationUnit = compilationUnit;
        _symbols = symbols;
        _structs = _compilationUnit.Declarations
            .OfType<StructDeclarationSyntax>()
            .ToDictionary(s => s.Name.Text, s => s, StringComparer.Ordinal);
    }

    public LayoutPlan Plan()
    {
        var globals = new List<GlobalLayout>();
        foreach (var global in _compilationUnit.Declarations.OfType<GlobalDeclarationSyntax>())
        {
            var layout = PlanGlobal(global);
            globals.Add(layout);
        }

        return new LayoutPlan(globals, _offset);
    }

    private GlobalLayout PlanGlobal(GlobalDeclarationSyntax global)
    {
        var fields = new List<FieldLayout>();
        var size = 0;

        if (global.Type is ArrayTypeSyntax arrayType && arrayType.ElementType is NamedTypeSyntax named && _structs.TryGetValue(named.Name, out var structDecl))
        {
            // Struct array → SoA fields
            foreach (var field in structDecl.Fields)
            {
                var fieldSize = SizeOf(field.Type);
                var count = int.TryParse(arrayType.SizeToken.Text, out var parsed) ? parsed : 1;
                var bytes = fieldSize * count;
                _offset = Align(_offset, fieldSize);
                fields.Add(new FieldLayout($"{structDecl.Name.Text}_{field.Identifier.Text}", _offset, bytes));
                _offset += bytes;
                size += bytes;
            }
        }
        else if (global.Type is NamedTypeSyntax primitiveNamed)
        {
            var bytes = SizeOf(primitiveNamed);
            _offset = Align(_offset, bytes);
            fields.Add(new FieldLayout(global.Name.Text, _offset, bytes));
            _offset += bytes;
            size = bytes;
        }
        else if (global.Type is ArrayTypeSyntax arrayPrim && arrayPrim.ElementType is NamedTypeSyntax prim)
        {
            var elemSize = SizeOf(prim);
            var count = int.TryParse(arrayPrim.SizeToken.Text, out var parsed) ? parsed : 1;
            var bytes = elemSize * count;
            _offset = Align(_offset, elemSize);
            fields.Add(new FieldLayout(global.Name.Text, _offset, bytes));
            _offset += bytes;
            size = bytes;
        }
        else
        {
            // Unknown type; still reserve nothing to keep deterministic offsets.
        }

        return new GlobalLayout(global.Name.Text, fields.FirstOrDefault()?.Offset ?? _offset, size, fields);
    }

    private int SizeOf(TypeSyntax type)
    {
        return type switch
        {
            NamedTypeSyntax named => SizeOfNamed(named.Name),
            ArrayTypeSyntax array => SizeOf(array.ElementType) * (int.TryParse(array.SizeToken.Text, out var count) ? count : 1),
            _ => 0
        };
    }

    private int SizeOfNamed(string name)
    {
        return name switch
        {
            "bool" => 1,
            "u8" => 1,
            "u16" => 2,
            "u32" => 4,
            "i32" => 4,
            "f32" => 4,
            "f64" => 8,
            "string" => 1, // string[N] is represented via array types; bare string treated as byte here.
            _ => 4 // default alignment for unknown; structs handled separately above.
        };
    }

    private static int Align(int offset, int alignment)
    {
        if (alignment <= 1)
        {
            return offset;
        }

        var remainder = offset % alignment;
        return remainder == 0 ? offset : offset + (alignment - remainder);
    }
}
