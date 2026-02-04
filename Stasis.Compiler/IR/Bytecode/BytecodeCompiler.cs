using System.Collections.Immutable;
using System.Globalization;
using System.Text;
using Stasis.Compiler.Syntax;

namespace Stasis.Compiler.IR.Bytecode;

public static class BytecodeCompiler
{
    public static BytecodeCompileResult Compile(CompilationUnitSyntax unit, string moduleName = "module")
    {
        var diagnostics = new List<Diagnostic>();

        var builder = new BytecodeBuilder();
        var globals = new Dictionary<string, (int index, BytecodeValueKind kind)>(StringComparer.Ordinal);
        foreach (var g in unit.Declarations.OfType<GlobalDeclarationSyntax>())
        {
            if (!TryMapTypeToKind(g.Type, out var kind) || kind == BytecodeValueKind.Void)
            {
                diagnostics.Add(new Diagnostic($"Bytecode backend: unsupported global type '{FormatType(g.Type)}'.", g.Type.Span));
                continue;
            }

            var idx = kind == BytecodeValueKind.F32
                ? builder.DeclareGlobalF32(g.Name.Text)
                : builder.DeclareGlobalI32(g.Name.Text);

            globals[g.Name.Text] = (idx, kind);
        }

        var functions = unit.Declarations
            .OfType<FunctionDeclarationSyntax>()
            .Where(f => !f.IsExtern && f.Body is not null)
            .ToList();

        foreach (var f in functions)
        {
            if (f.Body is null)
            {
                continue;
            }

            if (!TryMapTypeToKind(f.ReturnType, out var returnKind))
            {
                diagnostics.Add(new Diagnostic($"Bytecode backend: unsupported return type for '{f.Name.Text}'.", f.ReturnType?.Span ?? f.Name.Span));
                continue;
            }

            var locals = new Dictionary<string, (int index, BytecodeValueKind kind)>(StringComparer.Ordinal);
            var paramKinds = ImmutableArray.CreateBuilder<BytecodeValueKind>(f.Parameters.Count);

            var localIndex = 0;
            foreach (var p in f.Parameters)
            {
                if (!TryMapTypeToKind(p.Type, out var pk) || pk == BytecodeValueKind.Void)
                {
                    diagnostics.Add(new Diagnostic($"Bytecode backend: unsupported parameter type '{FormatType(p.Type)}' in '{f.Name.Text}'.", p.Type.Span));
                    continue;
                }
                locals[p.Name.Text] = (localIndex++, pk);
                paramKinds.Add(pk);
            }

            var letDecls = new List<VariableDeclarationSyntax>();
            CollectLets(f.Body, letDecls);
            foreach (var v in letDecls)
            {
                if (locals.ContainsKey(v.Name.Text))
                {
                    diagnostics.Add(new Diagnostic($"Bytecode backend: duplicate local '{v.Name.Text}' in '{f.Name.Text}'.", v.Name.Span));
                    continue;
                }

                var kind = BytecodeValueKind.I32;
                if (v.Type is not null)
                {
                    if (!TryMapTypeToKind(v.Type, out kind) || kind == BytecodeValueKind.Void)
                    {
                        diagnostics.Add(new Diagnostic($"Bytecode backend: unsupported local type '{FormatType(v.Type)}' in '{f.Name.Text}'.", v.Type.Span));
                        continue;
                    }
                }
                else if (v.Initializer is LiteralExpressionSyntax lit && TryMapLiteralToKind(lit.Literal, out var lk))
                {
                    kind = lk;
                }

                locals[v.Name.Text] = (localIndex++, kind);
            }

            var fb = builder.DefineFunction(
                $"{moduleName}__{f.Name.Text}",
                returnKind,
                paramKinds.ToImmutable(),
                localIndex);

            var emitter = new FunctionEmitter(fb, locals, globals, diagnostics);
            emitter.LowerBlock(f.Body);
            emitter.EnsureReturn(returnKind);
            fb.Finish();
        }

        if (diagnostics.Count > 0)
        {
            return BytecodeCompileResult.Fail(diagnostics);
        }

        var module = builder.Build();
        return BytecodeCompileResult.Ok(module, Disassemble(module));
    }

    private static void CollectLets(BlockStatementSyntax block, List<VariableDeclarationSyntax> lets)
    {
        foreach (var stmt in block.Statements)
        {
            if (stmt is VariableDeclarationSyntax v)
            {
                lets.Add(v);
                continue;
            }

            if (stmt is IfStatementSyntax iff)
            {
                CollectLets(iff.ThenBlock, lets);
                if (iff.ElseBlock is not null)
                {
                    CollectLets(iff.ElseBlock, lets);
                }
                continue;
            }

            if (stmt is ForStatementSyntax fr)
            {
                CollectLets(fr.Body, lets);
                continue;
            }

            if (stmt is BlockStatementSyntax nested)
            {
                CollectLets(nested, lets);
            }
        }
    }

    private static string Disassemble(BytecodeModule module)
    {
        var sb = new StringBuilder();
        sb.AppendLine("; Bytecode module");
        sb.AppendLine("; Globals:");
        for (var i = 0; i < module.Globals.Length; i++)
        {
            var g = module.Globals[i];
            sb.Append(i.ToString(CultureInfo.InvariantCulture)).Append(": ").Append(g.Name).Append(" : ").Append(g.Kind).AppendLine();
        }
        sb.AppendLine("; Functions:");
        for (var i = 0; i < module.Functions.Length; i++)
        {
            var f = module.Functions[i];
            sb.Append("fn[").Append(i.ToString(CultureInfo.InvariantCulture)).Append("] ").Append(f.Name);
            sb.Append(" locals=").Append(f.LocalCount.ToString(CultureInfo.InvariantCulture));
            sb.Append(" ret=").Append(f.ReturnKind);
            sb.Append(" params=").Append(string.Join(",", f.ParamKinds));
            sb.AppendLine();
            for (var ip = 0; ip < f.Code.Length; ip++)
            {
                var inst = f.Code[ip];
                sb.Append("  ").Append(ip.ToString(CultureInfo.InvariantCulture)).Append(": ").Append(inst.Op);
                if (inst.A != 0 || inst.B != 0)
                {
                    sb.Append(' ').Append(inst.A.ToString(CultureInfo.InvariantCulture));
                    if (inst.B != 0)
                    {
                        sb.Append(' ').Append(inst.B.ToString(CultureInfo.InvariantCulture));
                    }
                }
                sb.AppendLine();
            }
            sb.AppendLine();
        }
        return sb.ToString();
    }

    private static bool TryMapTypeToKind(TypeSyntax? type, out BytecodeValueKind kind)
    {
        if (type is null)
        {
            kind = BytecodeValueKind.Void;
            return true;
        }

        if (type is not NamedTypeSyntax named)
        {
            kind = BytecodeValueKind.Void;
            return false;
        }

        switch (named.Name)
        {
            case "i32":
            case "bool":
                kind = BytecodeValueKind.I32;
                return true;
            case "f32":
                kind = BytecodeValueKind.F32;
                return true;
            default:
                kind = BytecodeValueKind.Void;
                return false;
        }
    }

    private static bool TryMapLiteralToKind(Token lit, out BytecodeValueKind kind)
    {
        switch (lit.Kind)
        {
            case TokenKind.IntegerLiteral:
            case TokenKind.TrueKeyword:
            case TokenKind.FalseKeyword:
                kind = BytecodeValueKind.I32;
                return true;
            case TokenKind.FloatLiteral:
                kind = BytecodeValueKind.F32;
                return true;
            default:
                kind = BytecodeValueKind.Void;
                return false;
        }
    }

    private static string FormatType(TypeSyntax type) =>
        type switch
        {
            NamedTypeSyntax n => n.Name,
            ArrayTypeSyntax a => $"{FormatType(a.ElementType)}[{a.SizeText}]",
            _ => "?"
        };

    private sealed class FunctionEmitter
    {
        private readonly BytecodeBuilder.FunctionBuilder _fb;
        private readonly Dictionary<string, (int index, BytecodeValueKind kind)> _locals;
        private readonly Dictionary<string, (int index, BytecodeValueKind kind)> _globals;
        private readonly List<Diagnostic> _diagnostics;

        public FunctionEmitter(
            BytecodeBuilder.FunctionBuilder fb,
            Dictionary<string, (int index, BytecodeValueKind kind)> locals,
            Dictionary<string, (int index, BytecodeValueKind kind)> globals,
            List<Diagnostic> diagnostics)
        {
            _fb = fb;
            _locals = locals;
            _globals = globals;
            _diagnostics = diagnostics;
        }

        public void LowerBlock(BlockStatementSyntax block)
        {
            foreach (var stmt in block.Statements)
            {
                LowerStatement(stmt);
            }
        }

        public void EnsureReturn(BytecodeValueKind returnKind)
        {
            if (returnKind == BytecodeValueKind.Void)
            {
                _fb.Emit(BytecodeOp.ReturnVoid);
            }
            else if (returnKind == BytecodeValueKind.F32)
            {
                _fb.Emit(BytecodeOp.ConstF32, BitConverter.SingleToInt32Bits(0.0f));
                _fb.Emit(BytecodeOp.ReturnF32);
            }
            else
            {
                _fb.Emit(BytecodeOp.ConstI32, 0);
                _fb.Emit(BytecodeOp.ReturnI32);
            }
        }

        private void LowerStatement(StatementSyntax stmt)
        {
            switch (stmt)
            {
                case VariableDeclarationSyntax v:
                    LowerVar(v);
                    break;
                case ExpressionStatementSyntax es:
                    LowerExpr(es.Expression);
                    _fb.Emit(BytecodeOp.Pop);
                    break;
                case ReturnStatementSyntax r:
                    LowerReturn(r);
                    break;
                case IfStatementSyntax iff:
                    LowerIf(iff);
                    break;
                case ForStatementSyntax fr:
                    LowerFor(fr);
                    break;
                case BlockStatementSyntax b:
                    LowerBlock(b);
                    break;
                default:
                    _diagnostics.Add(new Diagnostic($"Bytecode backend: unsupported statement {stmt.GetType().Name}.", stmt.Span));
                    break;
            }
        }

        private void LowerVar(VariableDeclarationSyntax v)
        {
            if (!_locals.TryGetValue(v.Name.Text, out var local))
            {
                _diagnostics.Add(new Diagnostic($"Bytecode backend: unknown local '{v.Name.Text}'.", v.Name.Span));
                return;
            }

            if (v.Initializer is null)
            {
                EmitDefault(local.kind);
                EmitStoreLocal(local.index, local.kind);
                return;
            }

            var initKind = InferKind(v.Initializer);
            LowerExpr(v.Initializer);
            EmitCoerce(initKind, local.kind, v.Initializer.Span);
            EmitStoreLocal(local.index, local.kind);
        }

        private void LowerReturn(ReturnStatementSyntax r)
        {
            if (r.Expression is null)
            {
                _fb.Emit(BytecodeOp.ReturnVoid);
                return;
            }

            var k = InferKind(r.Expression);
            LowerExpr(r.Expression);
            _fb.Emit(k == BytecodeValueKind.F32 ? BytecodeOp.ReturnF32 : BytecodeOp.ReturnI32);
        }

        private void LowerIf(IfStatementSyntax iff)
        {
            LowerExpr(iff.Condition);
            var jzToElse = _fb.Emit(BytecodeOp.JumpIfZeroI32, 0);
            LowerBlock(iff.ThenBlock);
            if (iff.ElseBlock is null)
            {
                _fb.PatchJump(jzToElse, _fb.CurrentIp);
                return;
            }
            var jToMerge = _fb.Emit(BytecodeOp.Jump, 0);
            var elseIp = _fb.CurrentIp;
            _fb.PatchJump(jzToElse, elseIp);
            LowerBlock(iff.ElseBlock);
            _fb.PatchJump(jToMerge, _fb.CurrentIp);
        }

        private void LowerFor(ForStatementSyntax fr)
        {
            if (fr.Initializer is not null)
            {
                LowerExpr(fr.Initializer);
                _fb.Emit(BytecodeOp.Pop);
            }

            var loopIp = _fb.CurrentIp;

            if (fr.Condition is not null)
            {
                LowerExpr(fr.Condition);
            }
            else
            {
                _fb.Emit(BytecodeOp.ConstI32, 1);
            }

            var jzToEnd = _fb.Emit(BytecodeOp.JumpIfZeroI32, 0);
            LowerBlock(fr.Body);
            if (fr.Step is not null)
            {
                LowerExpr(fr.Step);
                _fb.Emit(BytecodeOp.Pop);
            }
            _fb.Emit(BytecodeOp.Jump, loopIp);
            _fb.PatchJump(jzToEnd, _fb.CurrentIp);
        }

        private void LowerExpr(ExpressionSyntax expr)
        {
            switch (expr)
            {
                case ParenthesizedExpressionSyntax p:
                    LowerExpr(p.Expression);
                    return;
                case LiteralExpressionSyntax lit:
                    LowerLiteral(lit);
                    return;
                case IdentifierExpressionSyntax id:
                    LowerIdLoad(id);
                    return;
                case UnaryExpressionSyntax un:
                    LowerUnary(un);
                    return;
                case AssignmentExpressionSyntax assign:
                    LowerAssign(assign);
                    return;
                case BinaryExpressionSyntax bin:
                    LowerBinary(bin);
                    return;
                default:
                    _diagnostics.Add(new Diagnostic($"Bytecode backend: unsupported expression {expr.GetType().Name}.", expr.Span));
                    _fb.Emit(BytecodeOp.ConstI32, 0);
                    return;
            }
        }

        private void LowerLiteral(LiteralExpressionSyntax lit)
        {
            switch (lit.Literal.Kind)
            {
                case TokenKind.IntegerLiteral:
                    if (!int.TryParse(lit.Literal.Text, NumberStyles.Integer, CultureInfo.InvariantCulture, out var i))
                    {
                        _diagnostics.Add(new Diagnostic($"Bytecode backend: invalid integer literal '{lit.Literal.Text}'.", lit.Span));
                        _fb.Emit(BytecodeOp.ConstI32, 0);
                        return;
                    }
                    _fb.Emit(BytecodeOp.ConstI32, i);
                    return;
                case TokenKind.FloatLiteral:
                    if (!float.TryParse(lit.Literal.Text, NumberStyles.Float, CultureInfo.InvariantCulture, out var f))
                    {
                        _diagnostics.Add(new Diagnostic($"Bytecode backend: invalid float literal '{lit.Literal.Text}'.", lit.Span));
                        _fb.Emit(BytecodeOp.ConstF32, BitConverter.SingleToInt32Bits(0.0f));
                        return;
                    }
                    _fb.Emit(BytecodeOp.ConstF32, BitConverter.SingleToInt32Bits(f));
                    return;
                case TokenKind.TrueKeyword:
                    _fb.Emit(BytecodeOp.ConstI32, 1);
                    return;
                case TokenKind.FalseKeyword:
                    _fb.Emit(BytecodeOp.ConstI32, 0);
                    return;
                default:
                    _diagnostics.Add(new Diagnostic("Bytecode backend: unsupported literal.", lit.Span));
                    _fb.Emit(BytecodeOp.ConstI32, 0);
                    return;
            }
        }

        private void LowerIdLoad(IdentifierExpressionSyntax id)
        {
            var name = id.Identifier.Text;
            if (_locals.TryGetValue(name, out var local))
            {
                EmitLoadLocal(local.index, local.kind);
                return;
            }
            if (_globals.TryGetValue(name, out var g))
            {
                EmitLoadGlobal(g.index, g.kind);
                return;
            }
            _diagnostics.Add(new Diagnostic($"Bytecode backend: unknown identifier '{name}'.", id.Identifier.Span));
            _fb.Emit(BytecodeOp.ConstI32, 0);
        }

        private void LowerUnary(UnaryExpressionSyntax un)
        {
            var k = InferKind(un.Operand);
            LowerExpr(un.Operand);
            if (un.OperatorToken.Kind == TokenKind.Minus)
            {
                _fb.Emit(k == BytecodeValueKind.F32 ? BytecodeOp.NegF32 : BytecodeOp.NegI32);
                return;
            }
            if (un.OperatorToken.Kind == TokenKind.Bang)
            {
                _fb.Emit(BytecodeOp.NotI32);
                return;
            }
            _diagnostics.Add(new Diagnostic($"Bytecode backend: unsupported unary operator '{un.OperatorToken.Text}'.", un.OperatorToken.Span));
        }

        private void LowerAssign(AssignmentExpressionSyntax assign)
        {
            if (assign.Left is not IdentifierExpressionSyntax id)
            {
                _diagnostics.Add(new Diagnostic("Bytecode backend: assignment target must be an identifier.", assign.Left.Span));
                LowerExpr(assign.Right);
                return;
            }

            var name = id.Identifier.Text;
            if (_locals.TryGetValue(name, out var local))
            {
                LowerAssignToLocal(local, assign);
                return;
            }
            if (_globals.TryGetValue(name, out var g))
            {
                LowerAssignToGlobal(g, assign);
                return;
            }

            _diagnostics.Add(new Diagnostic($"Bytecode backend: unknown identifier '{name}'.", id.Identifier.Span));
            LowerExpr(assign.Right);
        }

        private void LowerAssignToLocal((int index, BytecodeValueKind kind) local, AssignmentExpressionSyntax assign)
        {
            var rhsKind = InferKind(assign.Right);
            if (assign.OperatorToken.Kind == TokenKind.Equal)
            {
                LowerExpr(assign.Right);
                EmitCoerce(rhsKind, local.kind, assign.Right.Span);
                _fb.Emit(BytecodeOp.Dup);
                EmitStoreLocal(local.index, local.kind);
                return;
            }

            if (assign.OperatorToken.Kind is TokenKind.PlusEqual or TokenKind.MinusEqual or TokenKind.StarEqual or TokenKind.SlashEqual)
            {
                EmitLoadLocal(local.index, local.kind);
                LowerExpr(assign.Right);
                EmitCoerce(rhsKind, local.kind, assign.Right.Span);
                EmitBinaryOp(local.kind, assign.OperatorToken.Kind);
                _fb.Emit(BytecodeOp.Dup);
                EmitStoreLocal(local.index, local.kind);
                return;
            }

            _diagnostics.Add(new Diagnostic($"Bytecode backend: unsupported assignment operator '{assign.OperatorToken.Text}'.", assign.OperatorToken.Span));
            LowerExpr(assign.Right);
        }

        private void LowerAssignToGlobal((int index, BytecodeValueKind kind) g, AssignmentExpressionSyntax assign)
        {
            var rhsKind = InferKind(assign.Right);
            if (assign.OperatorToken.Kind == TokenKind.Equal)
            {
                LowerExpr(assign.Right);
                EmitCoerce(rhsKind, g.kind, assign.Right.Span);
                _fb.Emit(BytecodeOp.Dup);
                EmitStoreGlobal(g.index, g.kind);
                return;
            }

            if (assign.OperatorToken.Kind is TokenKind.PlusEqual or TokenKind.MinusEqual or TokenKind.StarEqual or TokenKind.SlashEqual)
            {
                EmitLoadGlobal(g.index, g.kind);
                LowerExpr(assign.Right);
                EmitCoerce(rhsKind, g.kind, assign.Right.Span);
                EmitBinaryOp(g.kind, assign.OperatorToken.Kind);
                _fb.Emit(BytecodeOp.Dup);
                EmitStoreGlobal(g.index, g.kind);
                return;
            }

            _diagnostics.Add(new Diagnostic($"Bytecode backend: unsupported assignment operator '{assign.OperatorToken.Text}'.", assign.OperatorToken.Span));
            LowerExpr(assign.Right);
        }

        private void LowerBinary(BinaryExpressionSyntax bin)
        {
            var lk = InferKind(bin.Left);
            var rk = InferKind(bin.Right);
            var arithKind = lk == BytecodeValueKind.F32 || rk == BytecodeValueKind.F32 ? BytecodeValueKind.F32 : BytecodeValueKind.I32;

            if (bin.OperatorToken.Kind is TokenKind.Plus or TokenKind.Minus or TokenKind.Star or TokenKind.Slash)
            {
                LowerExpr(bin.Left);
                EmitCoerce(lk, arithKind, bin.Left.Span);
                LowerExpr(bin.Right);
                EmitCoerce(rk, arithKind, bin.Right.Span);
                EmitBinaryOp(arithKind, bin.OperatorToken.Kind);
                return;
            }

            if (bin.OperatorToken.Kind is TokenKind.EqualEqual or TokenKind.BangEqual or TokenKind.Less or TokenKind.LessEqual or TokenKind.Greater or TokenKind.GreaterEqual)
            {
                LowerExpr(bin.Left);
                EmitCoerce(lk, arithKind, bin.Left.Span);
                LowerExpr(bin.Right);
                EmitCoerce(rk, arithKind, bin.Right.Span);
                EmitCompareOp(arithKind, bin.OperatorToken.Kind);
                return;
            }

            _diagnostics.Add(new Diagnostic($"Bytecode backend: unsupported binary operator '{bin.OperatorToken.Text}'.", bin.OperatorToken.Span));
            _fb.Emit(BytecodeOp.ConstI32, 0);
        }

        private BytecodeValueKind InferKind(ExpressionSyntax expr)
        {
            switch (expr)
            {
                case ParenthesizedExpressionSyntax p:
                    return InferKind(p.Expression);
                case LiteralExpressionSyntax lit:
                    return TryMapLiteralToKind(lit.Literal, out var k) ? k : BytecodeValueKind.I32;
                case IdentifierExpressionSyntax id:
                    {
                        var name = id.Identifier.Text;
                        if (_locals.TryGetValue(name, out var local)) return local.kind;
                        if (_globals.TryGetValue(name, out var g)) return g.kind;
                        return BytecodeValueKind.I32;
                    }
                case UnaryExpressionSyntax un:
                    return un.OperatorToken.Kind == TokenKind.Bang ? BytecodeValueKind.I32 : InferKind(un.Operand);
                case AssignmentExpressionSyntax assign:
                    return InferKind(assign.Right);
                case BinaryExpressionSyntax bin:
                    if (bin.OperatorToken.Kind is TokenKind.EqualEqual or TokenKind.BangEqual or TokenKind.Less or TokenKind.LessEqual or TokenKind.Greater or TokenKind.GreaterEqual)
                    {
                        return BytecodeValueKind.I32;
                    }
                    var lk = InferKind(bin.Left);
                    var rk = InferKind(bin.Right);
                    return lk == BytecodeValueKind.F32 || rk == BytecodeValueKind.F32 ? BytecodeValueKind.F32 : BytecodeValueKind.I32;
                default:
                    return BytecodeValueKind.I32;
            }
        }

        private void EmitDefault(BytecodeValueKind kind)
        {
            if (kind == BytecodeValueKind.F32)
            {
                _fb.Emit(BytecodeOp.ConstF32, BitConverter.SingleToInt32Bits(0.0f));
            }
            else
            {
                _fb.Emit(BytecodeOp.ConstI32, 0);
            }
        }

        private void EmitCoerce(BytecodeValueKind from, BytecodeValueKind to, SourceSpan span)
        {
            if (from == to)
            {
                return;
            }
            _diagnostics.Add(new Diagnostic($"Bytecode backend: cannot coerce {from} to {to} yet.", span));
        }

        private void EmitLoadLocal(int idx, BytecodeValueKind kind) =>
            _fb.Emit(kind == BytecodeValueKind.F32 ? BytecodeOp.LoadLocalF32 : BytecodeOp.LoadLocalI32, idx);

        private void EmitStoreLocal(int idx, BytecodeValueKind kind) =>
            _fb.Emit(kind == BytecodeValueKind.F32 ? BytecodeOp.StoreLocalF32 : BytecodeOp.StoreLocalI32, idx);

        private void EmitLoadGlobal(int idx, BytecodeValueKind kind) =>
            _fb.Emit(kind == BytecodeValueKind.F32 ? BytecodeOp.LoadGlobalF32 : BytecodeOp.LoadGlobalI32, idx);

        private void EmitStoreGlobal(int idx, BytecodeValueKind kind) =>
            _fb.Emit(kind == BytecodeValueKind.F32 ? BytecodeOp.StoreGlobalF32 : BytecodeOp.StoreGlobalI32, idx);

        private void EmitBinaryOp(BytecodeValueKind kind, TokenKind op)
        {
            if (kind == BytecodeValueKind.F32)
            {
                _fb.Emit(op switch
                {
                    TokenKind.Plus or TokenKind.PlusEqual => BytecodeOp.AddF32,
                    TokenKind.Minus or TokenKind.MinusEqual => BytecodeOp.SubF32,
                    TokenKind.Star or TokenKind.StarEqual => BytecodeOp.MulF32,
                    TokenKind.Slash or TokenKind.SlashEqual => BytecodeOp.DivF32,
                    _ => BytecodeOp.Nop
                });
                return;
            }

            _fb.Emit(op switch
            {
                TokenKind.Plus or TokenKind.PlusEqual => BytecodeOp.AddI32,
                TokenKind.Minus or TokenKind.MinusEqual => BytecodeOp.SubI32,
                TokenKind.Star or TokenKind.StarEqual => BytecodeOp.MulI32,
                TokenKind.Slash or TokenKind.SlashEqual => BytecodeOp.DivI32,
                _ => BytecodeOp.Nop
            });
        }

        private void EmitCompareOp(BytecodeValueKind kind, TokenKind op)
        {
            if (kind == BytecodeValueKind.F32)
            {
                _fb.Emit(op switch
                {
                    TokenKind.EqualEqual => BytecodeOp.CmpEqF32,
                    TokenKind.BangEqual => BytecodeOp.CmpNeF32,
                    TokenKind.Less => BytecodeOp.CmpLtF32,
                    TokenKind.LessEqual => BytecodeOp.CmpLeF32,
                    TokenKind.Greater => BytecodeOp.CmpGtF32,
                    TokenKind.GreaterEqual => BytecodeOp.CmpGeF32,
                    _ => BytecodeOp.Nop
                });
                return;
            }

            _fb.Emit(op switch
            {
                TokenKind.EqualEqual => BytecodeOp.CmpEqI32,
                TokenKind.BangEqual => BytecodeOp.CmpNeI32,
                TokenKind.Less => BytecodeOp.CmpLtI32,
                TokenKind.LessEqual => BytecodeOp.CmpLeI32,
                TokenKind.Greater => BytecodeOp.CmpGtI32,
                TokenKind.GreaterEqual => BytecodeOp.CmpGeI32,
                _ => BytecodeOp.Nop
            });
        }
    }
}
