using LLVMSharp.Interop;
using Stasis.Compiler.Layout;
using Stasis.Compiler.Semantic;
using Stasis.Compiler.Syntax;
using System.Globalization;

namespace Stasis.Compiler.IR;

/// <summary>
/// End-to-end lowering of parsed and analyzed Stasis code into an LLVM module.
/// Emits SoA globals, function prototypes, and basic bodies for simple expressions/returns.
/// </summary>
public sealed class ModuleLowerer
{
    public string LowerToIr(CompilationUnitSyntax compilationUnit, SemanticResult semantic, LayoutPlan layout, string moduleName = "module")
    {
        using var builder = new LlvmModuleBuilder(moduleName);
        EmitGlobals(compilationUnit, semantic.Symbols, builder);
        EmitFunctionSignatures(compilationUnit, semantic.Symbols, builder);

        var lowerer = new FunctionLowerer(builder, semantic.Symbols, layout);
        lowerer.Lower(compilationUnit);
        return builder.EmitToString();
    }

    private static void EmitGlobals(CompilationUnitSyntax compilationUnit, IReadOnlyDictionary<string, Symbol> symbols, LlvmModuleBuilder builder)
    {
        var structs = compilationUnit.Declarations
            .OfType<StructDeclarationSyntax>()
            .ToDictionary(s => s.Name.Text, s => s, StringComparer.Ordinal);

        foreach (var global in compilationUnit.Declarations.OfType<GlobalDeclarationSyntax>())
        {
            switch (global.Type)
            {
                case ArrayTypeSyntax array when array.ElementType is NamedTypeSyntax named && structs.TryGetValue(named.Name, out var structDecl):
                    {
                        var length = ParseArrayLength(array.SizeToken.Text);
                        foreach (var field in structDecl.Fields)
                        {
                            var fieldType = ResolveType(field.Type, symbols);
                            var llvmElem = builder.TypeMapper.Map(fieldType);
                            builder.DefineGlobalArray($"{structDecl.Name.Text}_{field.Identifier.Text}", llvmElem, length);
                        }

                        break;
                    }
                case ArrayTypeSyntax array:
                    {
                        var elementType = ResolveType(array.ElementType, symbols);
                        var llvmElem = builder.TypeMapper.Map(elementType);
                        var length = ParseArrayLength(array.SizeToken.Text);
                        builder.DefineGlobalArray(global.Name.Text, llvmElem, length);
                        break;
                    }
                case NamedTypeSyntax named:
                    {
                        var type = ResolveType(named, symbols);
                        var llvmType = builder.TypeMapper.Map(type);
                        builder.DefineGlobalScalar(global.Name.Text, llvmType);
                        break;
                    }
            }
        }
    }

    private static uint ParseArrayLength(string text) =>
        uint.TryParse(text, out var n) ? n : 0;

    private sealed class FunctionLowerer
    {
        private readonly LlvmModuleBuilder _moduleBuilder;
        private readonly IReadOnlyDictionary<string, Symbol> _symbols;
        private Dictionary<string, StructDeclarationSyntax> _structs = new(StringComparer.Ordinal);
        public FunctionLowerer(LlvmModuleBuilder moduleBuilder, IReadOnlyDictionary<string, Symbol> symbols, LayoutPlan layout)
        {
            _moduleBuilder = moduleBuilder;
            _symbols = symbols;
        }

        public void Lower(CompilationUnitSyntax compilationUnit)
        {
            _structs = compilationUnit.Declarations
                .OfType<StructDeclarationSyntax>()
                .ToDictionary(s => s.Name.Text, s => s, StringComparer.Ordinal);

            foreach (var fn in compilationUnit.Declarations.OfType<FunctionDeclarationSyntax>())
            {
                LowerFunction(fn);
            }

            foreach (var test in compilationUnit.Declarations.OfType<TestDeclarationSyntax>())
            {
                LowerFunction(test);
            }
        }

        private void LowerFunction(FunctionDeclarationSyntax fn)
        {
            LowerFunctionCore(fn.Name.Text, fn.Parameters, fn.ReturnType, fn.Body);
        }

        private void LowerFunction(TestDeclarationSyntax test)
        {
            LowerFunctionCore(test.Name.Text, test.Parameters, test.ReturnType, test.Body);
        }

        private readonly record struct LocalBinding(LLVMValueRef Value, LLVMTypeRef Type, bool IsAddress);

        private void LowerFunctionCore(string name, IReadOnlyList<ParameterSyntax> parameters, TypeSyntax? returnType, BlockStatementSyntax body)
        {
            var function = _moduleBuilder.Module.GetNamedFunction(name);
            if (function.Handle == IntPtr.Zero)
            {
                return;
            }

            using var builder = _moduleBuilder.Context.CreateBuilder();
            var entry = function.AppendBasicBlock("entry");
            builder.PositionAtEnd(entry);

            var locals = new Dictionary<string, LocalBinding>(StringComparer.Ordinal);

            for (int i = 0; i < parameters.Count; i++)
            {
                var param = parameters[i];
                var paramVal = function.GetParam((uint)i);
                var paramType = ResolveType(param.Type, _symbols);
                locals[param.Name.Text] = new LocalBinding(paramVal, _moduleBuilder.TypeMapper.Map(paramType), false);
            }

            foreach (var stmt in body.Statements)
            {
                switch (stmt)
                {
                    case VariableDeclarationSyntax decl:
                        LowerVariableDeclaration(builder, decl, locals);
                        break;
                    case ExpressionStatementSyntax exprStmt:
                        LowerExpression(builder, exprStmt.Expression, locals);
                        break;
                    case ReturnStatementSyntax ret:
                        if (ret.Expression is null)
                        {
                            builder.BuildRetVoid();
                        }
                        else
                        {
                            var value = LowerExpression(builder, ret.Expression, locals);
                            builder.BuildRet(value);
                        }

                        return;
                    default:
                        break;
                }
            }

            var isVoid = returnType is null || (returnType is NamedTypeSyntax named && string.Equals(named.Name, "void", StringComparison.Ordinal));
            if (isVoid)
            {
                builder.BuildRetVoid();
            }
        }

        private void LowerVariableDeclaration(LLVMBuilderRef builder, VariableDeclarationSyntax decl, Dictionary<string, LocalBinding> locals)
        {
            if (decl.Type is null)
            {
                return;
            }

            var type = ResolveType(decl.Type, _symbols);
            var llvmType = _moduleBuilder.TypeMapper.Map(type);
            var alloca = builder.BuildAlloca(llvmType, decl.Name.Text);
            locals[decl.Name.Text] = new LocalBinding(alloca, llvmType, true);
        }

        private LLVMValueRef LowerExpression(LLVMBuilderRef builder, ExpressionSyntax expr, Dictionary<string, LocalBinding> locals)
        {
            switch (expr)
            {
                case LiteralExpressionSyntax lit:
                    return LowerLiteral(lit);
                case IdentifierExpressionSyntax id:
                    if (locals.TryGetValue(id.Identifier.Text, out var value))
                    {
                        if (value.IsAddress)
                        {
                            return builder.BuildLoad2(value.Type, value.Value, id.Identifier.Text);
                        }

                        return value.Value;
                    }

                    if (_symbols.TryGetValue(id.Identifier.Text, out var sym) && sym.Kind == SymbolKind.Global && sym.Type is not null)
                    {
                        var global = _moduleBuilder.Module.GetNamedGlobal(id.Identifier.Text);
                        var type = _moduleBuilder.TypeMapper.Map(sym.Type);
                        return builder.BuildLoad2(type, global, id.Identifier.Text);
                    }

                    return ConstI32(0);
                case MemberAccessExpressionSyntax member:
                    return LowerMemberAccess(builder, member, locals);
                case ArrayAccessExpressionSyntax arr:
                    return LowerArrayAccess(builder, arr, null, locals);
                case OperatorCallExpressionSyntax op:
                    return LowerOperatorCall(builder, op, locals);
                default:
                    return ConstI32(0);
            }
        }

        private LLVMValueRef LowerLiteral(LiteralExpressionSyntax lit)
        {
            switch (lit.Literal.Kind)
            {
                case TokenKind.IntegerLiteral when int.TryParse(lit.Literal.Text, out var ival):
                    return ConstI32(ival);
                case TokenKind.FloatLiteral when float.TryParse(lit.Literal.Text, NumberStyles.Float, CultureInfo.InvariantCulture, out var fval):
                    return LLVMValueRef.CreateConstReal(_moduleBuilder.TypeMapper.Map(new PrimitiveTypeSymbol("f32")), fval);
                case TokenKind.TrueKeyword:
                    return ConstI32(1);
                case TokenKind.FalseKeyword:
                    return ConstI32(0);
                default:
                    return ConstI32(0);
            }
        }

        private LLVMValueRef LowerOperatorCall(LLVMBuilderRef builder, OperatorCallExpressionSyntax op, Dictionary<string, LocalBinding> locals)
        {
            var opText = op.OperatorToken.Text;
            if (opText == "=")
            {
                var receiver = op.Receiver as IdentifierExpressionSyntax;
                if (receiver is not null && locals.TryGetValue(receiver.Identifier.Text, out var target))
                {
                    var value = LowerExpression(builder, op.Arguments[0], locals);
                    if (target.IsAddress)
                    {
                        builder.BuildStore(value, target.Value);
                    }

                    return value;
                }

                if (receiver is not null && _symbols.TryGetValue(receiver.Identifier.Text, out var sym) && sym.Kind == SymbolKind.Global && sym.Type is not null)
                {
                    var value = LowerExpression(builder, op.Arguments[0], locals);
                    var global = _moduleBuilder.Module.GetNamedGlobal(receiver.Identifier.Text);
                    var type = _moduleBuilder.TypeMapper.Map(sym.Type);
                    builder.BuildStore(value, global);
                    return value;
                }

                if (op.Receiver is ArrayAccessExpressionSyntax arr)
                {
                    if (TryLowerArrayElementPointer(builder, arr, null, locals, out var ptr, out var elemType))
                    {
                        var value = LowerExpression(builder, op.Arguments[0], locals);
                        builder.BuildStore(value, ptr);
                        return value;
                    }
                }

                if (op.Receiver is MemberAccessExpressionSyntax member && member.Receiver is ArrayAccessExpressionSyntax arrRecv)
                {
                    if (TryLowerArrayElementPointer(builder, arrRecv, member.Member.Text, locals, out var ptr, out _))
                    {
                        var value = LowerExpression(builder, op.Arguments[0], locals);
                        builder.BuildStore(value, ptr);
                        return value;
                    }
                }

                return LLVMValueRef.CreateConstInt(_moduleBuilder.TypeMapper.Map(new PrimitiveTypeSymbol("i32")), 0, false);
            }

            var lhs = LowerExpression(builder, op.Receiver, locals);
            var rhs = LowerExpression(builder, op.Arguments[0], locals);
            return opText switch
            {
                "+" => builder.BuildAdd(lhs, rhs, "addtmp"),
                "-" => builder.BuildSub(lhs, rhs, "subtmp"),
                "*" => builder.BuildMul(lhs, rhs, "multmp"),
                "/" => builder.BuildSDiv(lhs, rhs, "divtmp"),
                "%" => builder.BuildSRem(lhs, rhs, "remtmp"),
                _ => lhs
            };
        }

        private LLVMValueRef LowerArrayAccess(LLVMBuilderRef builder, ArrayAccessExpressionSyntax arr, Dictionary<string, LocalBinding> locals)
        {
            if (TryLowerArrayElementPointer(builder, arr, fieldName: null, locals, out var ptr, out var elemType))
            {
                return builder.BuildLoad2(elemType, ptr, "elemload");
            }

            return ConstI32(0);
        }

        private LLVMValueRef LowerMemberAccess(LLVMBuilderRef builder, MemberAccessExpressionSyntax member, Dictionary<string, LocalBinding> locals)
        {
            if (member.Receiver is ArrayAccessExpressionSyntax arr)
            {
                if (TryLowerArrayElementPointer(builder, arr, member.Member.Text, locals, out var ptr, out var elemType))
                {
                    return builder.BuildLoad2(elemType, ptr, "fieldload");
                }
            }

            return ConstI32(0);
        }

        private LLVMValueRef LowerArrayAccess(LLVMBuilderRef builder, ArrayAccessExpressionSyntax arr, string? fieldName, Dictionary<string, LocalBinding> locals)
        {
            if (TryLowerArrayElementPointer(builder, arr, fieldName, locals, out var ptr, out var elemType))
            {
                return builder.BuildLoad2(elemType, ptr, "elemload");
            }

            return ConstI32(0);
        }

        private bool TryLowerArrayElementPointer(LLVMBuilderRef builder, ArrayAccessExpressionSyntax arr, string? fieldName, Dictionary<string, LocalBinding> locals, out LLVMValueRef ptr, out LLVMTypeRef elemType)
        {
            ptr = default;
            elemType = default;

            if (arr.Receiver is IdentifierExpressionSyntax id)
            {
                if (_symbols.TryGetValue(id.Identifier.Text, out var sym) && sym.Kind == SymbolKind.Global && sym.Type is ArrayTypeSymbol arrayType)
                {
                    var zero = ConstI32(0);
                    var index = LowerExpression(builder, arr.Index, locals);

                    if (arrayType.ElementType is NamedTypeSymbol namedElem && fieldName is not null && _structs.TryGetValue(namedElem.TypeName, out var structDecl))
                    {
                        var field = structDecl.Fields.FirstOrDefault(f => string.Equals(f.Identifier.Text, fieldName, StringComparison.Ordinal));
                        if (field is not null)
                        {
                            var fieldType = ResolveType(field.Type, _symbols);
                            elemType = _moduleBuilder.TypeMapper.Map(fieldType);
                            var fieldGlobalName = $"{namedElem.TypeName}_{fieldName}";
                            var fieldGlobal = _moduleBuilder.Module.GetNamedGlobal(fieldGlobalName);
                            if (fieldGlobal.Handle != IntPtr.Zero)
                            {
                                ptr = builder.BuildGEP2(elemType, fieldGlobal, new[] { zero, index }, "fieldaddr");
                                return true;
                            }
                        }
                    }
                    else
                    {
                        var global = _moduleBuilder.Module.GetNamedGlobal(id.Identifier.Text);
                        elemType = _moduleBuilder.TypeMapper.Map(arrayType.ElementType);
                        ptr = builder.BuildGEP2(elemType, global, new[] { zero, index }, "elemaddr");
                        return true;
                    }
                }
            }

            return false;
        }

        private LLVMValueRef ConstI32(int value) =>
            LLVMValueRef.CreateConstInt(_moduleBuilder.TypeMapper.Map(new PrimitiveTypeSymbol("i32")), (ulong)value, true);
    }

    private static void EmitFunctionSignatures(CompilationUnitSyntax compilationUnit, IReadOnlyDictionary<string, Symbol> symbols, LlvmModuleBuilder builder)
    {
        foreach (var fn in compilationUnit.Declarations.OfType<FunctionDeclarationSyntax>())
        {
            EmitFunction(builder, symbols, fn.Name.Text, fn.ReturnType, fn.Parameters);
        }

        foreach (var test in compilationUnit.Declarations.OfType<TestDeclarationSyntax>())
        {
            EmitFunction(builder, symbols, test.Name.Text, test.ReturnType, test.Parameters);
        }
    }

    private static void EmitFunction(LlvmModuleBuilder builder, IReadOnlyDictionary<string, Symbol> symbols, string name, TypeSyntax? returnType, IReadOnlyList<ParameterSyntax> parameters)
    {
        var ret = returnType is null
            ? LLVMTypeRef.Void
            : builder.TypeMapper.Map(ResolveType(returnType, symbols));

        var paramTypes = parameters
            .Select(p => builder.TypeMapper.Map(ResolveType(p.Type, symbols)))
            .ToArray();

        builder.DefineFunction(name, ret, paramTypes);
    }

    private static TypeSymbol ResolveType(TypeSyntax syntax, IReadOnlyDictionary<string, Symbol> symbols)
    {
        switch (syntax)
        {
            case NamedTypeSyntax named:
                if (symbols.TryGetValue(named.Name, out var sym) && sym.Type is not null)
                {
                    return sym.Type;
                }

                if (string.Equals(named.Name, "void", StringComparison.Ordinal))
                {
                    return new VoidTypeSymbol();
                }

                return new NamedTypeSymbol(named.Name);
            case ArrayTypeSyntax array:
                var element = ResolveType(array.ElementType, symbols);
                var size = int.TryParse(array.SizeToken.Text, out var parsed) ? parsed : 0;
                return new ArrayTypeSymbol(element, size);
            default:
                return new NamedTypeSymbol("unknown");
        }
    }
}
