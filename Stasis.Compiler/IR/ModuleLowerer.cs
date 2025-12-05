using LLVMSharp.Interop;
using Stasis.Compiler.Layout;
using Stasis.Compiler.Semantic;
using Stasis.Compiler.Syntax;

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
        EmitGlobals(layout, builder);
        EmitFunctionSignatures(compilationUnit, semantic.Symbols, builder);

        var lowerer = new FunctionLowerer(builder, semantic.Symbols, layout);
        lowerer.Lower(compilationUnit);
        return builder.EmitToString();
    }

    private static void EmitGlobals(LayoutPlan layout, LlvmModuleBuilder builder)
    {
        foreach (var global in layout.Globals)
        {
            if (global.Fields.Count > 0)
            {
                foreach (var field in global.Fields)
                {
                    var length = (uint)Math.Max(field.Size, 1);
                    builder.DefineGlobalArray(field.Name, LLVMTypeRef.Int8, length);
                }
            }
            else
            {
                var length = (uint)Math.Max(global.Size, 1);
                builder.DefineGlobalArray(global.Name, LLVMTypeRef.Int8, length);
            }
        }
    }

    private sealed class FunctionLowerer
    {
        private readonly LlvmModuleBuilder _moduleBuilder;
        private readonly IReadOnlyDictionary<string, Symbol> _symbols;
        private readonly LayoutPlan _layout;

        public FunctionLowerer(LlvmModuleBuilder moduleBuilder, IReadOnlyDictionary<string, Symbol> symbols, LayoutPlan layout)
        {
            _moduleBuilder = moduleBuilder;
            _symbols = symbols;
            _layout = layout;
        }

        public void Lower(CompilationUnitSyntax compilationUnit)
        {
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

                    return LLVMValueRef.CreateConstInt(_moduleBuilder.TypeMapper.Map(new PrimitiveTypeSymbol("i32")), 0, false);
                case OperatorCallExpressionSyntax op:
                    return LowerOperatorCall(builder, op, locals);
                default:
                    return LLVMValueRef.CreateConstInt(_moduleBuilder.TypeMapper.Map(new PrimitiveTypeSymbol("i32")), 0, false);
            }
        }

        private LLVMValueRef LowerLiteral(LiteralExpressionSyntax lit)
        {
            if (int.TryParse(lit.Literal.Text, out var value))
            {
                return LLVMValueRef.CreateConstInt(_moduleBuilder.TypeMapper.Map(new PrimitiveTypeSymbol("i32")), (ulong)value, true);
            }

            return LLVMValueRef.CreateConstInt(_moduleBuilder.TypeMapper.Map(new PrimitiveTypeSymbol("i32")), 0, false);
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
