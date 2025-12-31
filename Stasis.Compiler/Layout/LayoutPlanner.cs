using System;
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
            // Struct array → SoA fields (e.g., global asteroids: Asteroid[10])
            var count = int.TryParse(arrayType.SizeToken?.Text, out var parsed) ? parsed : 1;
            foreach (var field in structDecl.Fields)
            {
                var fieldBytes = PlanStructArrayField(structDecl.Name.Text, field, count, ref fields);
                size += fieldBytes;
            }
        }
        else if (global.Type is NamedTypeSyntax namedType && _structs.TryGetValue(namedType.Name, out var structInstance))
        {
            // Struct instance → flatten fields (e.g., global state: GameState)
            foreach (var field in structInstance.Fields)
            {
                var fieldBytes = PlanStructField(global.Name.Text, structInstance.Name.Text, field, ref fields);
                size += fieldBytes;
            }
        }
        else if (global.Type is NamedTypeSyntax primitiveNamed)
        {
            var bytes = SizeOf(primitiveNamed);
            _offset = Align(_offset, bytes);
            var fieldType = GetFieldType(primitiveNamed);
            fields.Add(new FieldLayout(global.Name.Text, _offset, bytes, fieldType));
            _offset += bytes;
            size = bytes;
        }
        else if (global.Type is ArrayTypeSyntax arrayPrim && arrayPrim.ElementType is NamedTypeSyntax prim)
        {
            var elemSize = SizeOf(prim);
            var count = int.TryParse(arrayPrim.SizeToken?.Text, out var parsed) ? parsed : 1;
            var bytes = elemSize * count;
            _offset = Align(_offset, elemSize);
            var fieldType = GetFieldType(prim);
            fields.Add(new FieldLayout(global.Name.Text, _offset, bytes, fieldType, count));
            _offset += bytes;
            size = bytes;
        }
        else
        {
            // Unknown type; still reserve nothing to keep deterministic offsets.
        }

        var firstField = fields.FirstOrDefault();
        var firstOffset = firstField is null ? _offset : firstField.Offset;
        return new GlobalLayout(global.Name.Text, firstOffset, size, fields);
    }

    private int PlanStructField(string globalName, string structName, StructFieldSyntax field, ref List<FieldLayout> fields)
    {
        var totalBytes = 0;

        if (field.Type is ArrayTypeSyntax arrayType && arrayType.ElementType is NamedTypeSyntax nestedStruct && _structs.TryGetValue(nestedStruct.Name, out var nestedStructDecl))
        {
            // Nested struct array → SoA transformation (e.g., asteroids: Asteroid[8] inside GameState)
            var count = int.TryParse(arrayType.SizeToken?.Text, out var parsed) ? parsed : 1;
            foreach (var nestedField in nestedStructDecl.Fields)
            {
                var nestedBytes = PlanStructArrayField($"{globalName}__{field.Identifier.Text}", nestedField, count, ref fields);
                totalBytes += nestedBytes;
            }
        }
        else if (field.Type is NamedTypeSyntax namedField && _structs.TryGetValue(namedField.Name, out var structInstance))
        {
            // Nested struct instance → recursively flatten (e.g., ship: Ship inside GameState)
            foreach (var nestedField in structInstance.Fields)
            {
                var nestedBytes = PlanStructField($"{globalName}__{field.Identifier.Text}", structInstance.Name.Text, nestedField, ref fields);
                totalBytes += nestedBytes;
            }
        }
        else
        {
            // Scalar or primitive array field
            var bytes = SizeOf(field.Type);
            var arrayCount = 1;
            if (field.Type is ArrayTypeSyntax arr && int.TryParse(arr.SizeToken?.Text, out var cnt) && cnt > 0)
            {
                arrayCount = cnt;
            }
            var divisor = arrayCount;
            _offset = Align(_offset, bytes > 0 ? (bytes / divisor) : 4);
            var fieldType = GetFieldType(field.Type);
            fields.Add(new FieldLayout($"{globalName}__{field.Identifier.Text}", _offset, bytes, fieldType, arrayCount));
            _offset += bytes;
            totalBytes = bytes;
        }

        return totalBytes;
    }

    private int PlanStructArrayField(string prefix, StructFieldSyntax field, int count, ref List<FieldLayout> fields)
    {
        if (field.Type is NamedTypeSyntax namedField && _structs.TryGetValue(namedField.Name, out var nestedStruct))
        {
            var totalBytes = 0;
            foreach (var nestedField in nestedStruct.Fields)
            {
                totalBytes += PlanStructArrayField($"{prefix}__{field.Identifier.Text}", nestedField, count, ref fields);
            }
            return totalBytes;
        }

        var bytesPerElement = SizeOf(field.Type);
        var bytes = bytesPerElement * count;
        _offset = Align(_offset, AlignmentOf(field.Type));
        var fieldType = GetFieldType(field.Type);
        fields.Add(new FieldLayout($"{prefix}__{field.Identifier.Text}", _offset, bytes, fieldType, count));
        _offset += bytes;
        return bytes;
    }

    private int AlignmentOf(TypeSyntax type)
    {
        if (type is ArrayTypeSyntax array)
        {
            return Math.Max(1, SizeOf(array.ElementType));
        }

        return Math.Max(1, SizeOf(type));
    }

    private static FieldType GetFieldType(TypeSyntax type)
    {
        var typeName = type switch
        {
            NamedTypeSyntax named => named.Name,
            ArrayTypeSyntax array when array.ElementType is NamedTypeSyntax elem => elem.Name,
            _ => ""
        };

        return typeName switch
        {
            "bool" => FieldType.Bool,
            "u8" => FieldType.U8,
            "u16" => FieldType.U16,
            "u32" => FieldType.U32,
            "i32" => FieldType.I32,
            "f32" => FieldType.F32,
            "f64" => FieldType.F64,
            "string" or "utf8" or "ascii" => FieldType.String,
            _ => FieldType.Unknown
        };
    }

    private int SizeOf(TypeSyntax type)
    {
        return type switch
        {
            NamedTypeSyntax named => SizeOfNamed(named.Name),
            ArrayTypeSyntax array => SizeOfArray(array),
            _ => 0
        };
    }

    private int SizeOfArray(ArrayTypeSyntax array)
    {
        var count = int.TryParse(array.SizeToken?.Text, out var parsed) ? parsed : 1;
        if (array.ElementType is NamedTypeSyntax named)
        {
            var headerSize = HeaderSizeFor(named.Name);
            if (headerSize > 0)
            {
                return headerSize + count;
            }
        }

        return SizeOf(array.ElementType) * count;
    }

    private static int HeaderSizeFor(string name) =>
        name switch
        {
            "string" => 8,
            "utf8" => 8,
            "ascii" => 4,
            _ => 0
        };

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
            "string" => IntPtr.Size, // bare string is a pointer; string[N] uses utf8 header + payload.
            "utf8" => IntPtr.Size,
            "ascii" => IntPtr.Size,
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
