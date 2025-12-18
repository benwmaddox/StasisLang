using System.Text;

namespace Stasis.Compiler.IR.Cranelift;

/// <summary>
/// Builder for Cranelift modules.
/// Generates CLIF (Cranelift Intermediate Format) text representation.
///
/// Note: This is a scaffolding implementation that generates CLIF text.
/// Full implementation will use native Cranelift bindings for JIT compilation.
/// </summary>
public sealed class CraneliftModuleBuilder : IDisposable
{
    private readonly string _moduleName;
    private readonly StringBuilder _globals = new();
    private readonly StringBuilder _externals = new();
    private readonly StringBuilder _functions = new();
    private readonly List<string> _functionNames = new();
    private readonly HashSet<string> _externalFunctions = new();
    private readonly Dictionary<string, CraneliftTypeMapper.ClifType> _globalTypes = new();
    private readonly Dictionary<string, string> _stringLiterals = new(); // maps literal value -> global name
    private readonly CraneliftTypeMapper _typeMapper = new();
    private int _stringLiteralCounter;

    public CraneliftModuleBuilder(string moduleName)
    {
        _moduleName = moduleName;
    }

    public CraneliftTypeMapper TypeMapper => _typeMapper;
    public IReadOnlyDictionary<string, CraneliftTypeMapper.ClifType> GlobalTypes => _globalTypes;
    public IReadOnlySet<string> ExternalFunctions => _externalFunctions;
    public IReadOnlyDictionary<string, string> StringLiterals => _stringLiterals;

    /// <summary>
    /// Defines a global variable.
    /// </summary>
    public void DefineGlobal(string name, CraneliftTypeMapper.ClifType type)
    {
        _globalTypes[name] = type;
        _globals.AppendLine($"global {name}: {FormatType(type)}");
    }

    /// <summary>
    /// Defines a global array with the given element count.
    /// </summary>
    public void DefineGlobalArray(string name, CraneliftTypeMapper.ClifType elementType, int length)
    {
        _globalTypes[name] = elementType;
        _globals.AppendLine($"global {name}: {FormatType(elementType)}[{length}]");
    }

    /// <summary>
    /// Declares an external function (imported from C runtime).
    /// </summary>
    public void DeclareExternal(string name, CraneliftTypeMapper.ClifType returnType, params CraneliftTypeMapper.ClifType[] paramTypes)
    {
        if (_externalFunctions.Contains(name))
        {
            return;
        }
        _externalFunctions.Add(name);
        var paramStr = string.Join(", ", paramTypes.Select(FormatType));
        var retStr = FormatReturnType(returnType);
        _externals.AppendLine($"external {name}({paramStr}){retStr} windows_fastcall");
    }

    /// <summary>
    /// Defines a string literal as global data.
    /// Returns the global name for this string literal.
    /// </summary>
    public string DefineStringLiteral(string value)
    {
        if (_stringLiterals.TryGetValue(value, out var existingName))
        {
            return existingName;
        }

        var globalName = $"str_{_stringLiteralCounter++}";
        _stringLiterals[value] = globalName;

        // For now, store as i64 (pointer type)
        // TODO: Actually emit string data bytes
        _globalTypes[globalName] = CraneliftTypeMapper.ClifType.I64;
        _globals.AppendLine($"global {globalName}: i64 ; \"{EscapeString(value)}\"");

        return globalName;
    }

    private static string EscapeString(string s)
    {
        return s.Replace("\\", "\\\\")
                .Replace("\"", "\\\"")
                .Replace("\n", "\\n")
                .Replace("\r", "\\r")
                .Replace("\t", "\\t");
    }

    /// <summary>
    /// Defines a function signature (stub without body).
    /// </summary>
    public void DefineFunction(string name, CraneliftTypeMapper.ClifType returnType, params CraneliftTypeMapper.ClifType[] paramTypes)
    {
        _functionNames.Add(name);
        var paramStr = string.Join(", ", paramTypes.Select(FormatType));
        var retStr = FormatReturnType(returnType);
        _functions.AppendLine($"function %{name}({paramStr}){retStr} windows_fastcall {{");
        _functions.AppendLine($"block0:");
        _functions.AppendLine($"    ; TODO: function body");
        if (returnType != CraneliftTypeMapper.ClifType.Void)
        {
            _functions.AppendLine($"    v0 = iconst.i32 0");
            _functions.AppendLine($"    return v0");
        }
        else
        {
            _functions.AppendLine($"    return");
        }
        _functions.AppendLine($"}}");
        _functions.AppendLine();
    }

    /// <summary>
    /// Defines a function with a complete body.
    /// </summary>
    public void DefineFunctionWithBody(
        string name,
        CraneliftTypeMapper.ClifType returnType,
        CraneliftTypeMapper.ClifType[] paramTypes,
        string body)
    {
        _functionNames.Add(name);
        var paramStr = string.Join(", ", paramTypes.Select(FormatType));
        var retStr = FormatReturnType(returnType);
        _functions.AppendLine($"function %{name}({paramStr}){retStr} windows_fastcall {{");
        _functions.Append(body);
        _functions.AppendLine($"}}");
        _functions.AppendLine();
    }

    /// <summary>
    /// Emits the module as CLIF text.
    /// </summary>
    public string EmitToString()
    {
        var sb = new StringBuilder();
        sb.AppendLine($"; Cranelift module: {_moduleName}");
        sb.AppendLine($"; Generated by Stasis compiler");
        sb.AppendLine();

        if (_globals.Length > 0)
        {
            sb.AppendLine("; === Globals ===");
            sb.Append(_globals);
            sb.AppendLine();
        }

        if (_externals.Length > 0)
        {
            sb.AppendLine("; === External Functions ===");
            sb.Append(_externals);
            sb.AppendLine();
        }

        if (_functions.Length > 0)
        {
            sb.AppendLine("; === Functions ===");
            sb.Append(_functions);
        }

        return sb.ToString();
    }

    public void Dispose()
    {
        // Nothing to dispose in scaffolding implementation
    }

    private static string FormatType(CraneliftTypeMapper.ClifType type) =>
        type switch
        {
            CraneliftTypeMapper.ClifType.Void => "void",
            CraneliftTypeMapper.ClifType.I8 => "i8",
            CraneliftTypeMapper.ClifType.I16 => "i16",
            CraneliftTypeMapper.ClifType.I32 => "i32",
            CraneliftTypeMapper.ClifType.I64 => "i64",
            CraneliftTypeMapper.ClifType.F32 => "f32",
            CraneliftTypeMapper.ClifType.F64 => "f64",
            CraneliftTypeMapper.ClifType.B1 => "b1",
            CraneliftTypeMapper.ClifType.R64 => "r64",
            _ => "i32"
        };

    private static string FormatReturnType(CraneliftTypeMapper.ClifType type)
    {
        if (type == CraneliftTypeMapper.ClifType.Void)
            return string.Empty;
        if (type == CraneliftTypeMapper.ClifType.I32)
            return " -> i32";
        return $" -> {FormatType(type)}";
    }
}
