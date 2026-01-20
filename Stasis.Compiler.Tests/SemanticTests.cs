using Stasis.Compiler.Semantic;
using Stasis.Compiler.Syntax;

namespace Stasis.Compiler.Tests;

public class SemanticTests
{
    [Fact]
    public void Flags_unknown_type_in_global()
    {
        var source = """
            global bad: MissingType;
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Contains(sema.Diagnostics, d => d.Message.Contains("Unknown type"));
    }

    [Fact]
    public void Flags_let_without_type()
    {
        var source = """
            function f(): void {
                let x;
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Contains(sema.Diagnostics, d => d.Message.Contains("Local variables must declare a type"));
    }

    [Fact]
    public void Allows_local_struct_reference()
    {
        var source = """
            struct Player { hp: u8; }
            function f(): void {
                let p: Player = 0;
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Empty(sema.Diagnostics);
    }

    [Fact]
    public void Allows_parameter_struct_reference()
    {
        var source = """
            struct Player { hp: u8; }
            function f(p: Player): void {
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Empty(sema.Diagnostics);
    }

    [Fact]
    public void Flags_void_local()
    {
        var source = """
            function f(): void {
                let x: void;
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Contains(sema.Diagnostics, d => d.Message.Contains("primitive types, struct references, or arrays"));
    }

    [Fact]
    public void Allows_compound_assignment()
    {
        var source = """
            function f(): i32 {
                let x: i32 = 0;
                x += 2;
                return x;
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Empty(sema.Diagnostics);
    }

    [Fact]
    public void Flags_non_extern_function_declaration_without_body()
    {
        var source = """
            function f(): i32;
            """;

        var parse = Parser.Parse(source);
        Assert.Empty(parse.Diagnostics);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Contains(sema.Diagnostics, d => d.Message.Contains("missing a body", StringComparison.OrdinalIgnoreCase));
    }

    [Fact]
    public void Flags_read_of_uninitialized_local()
    {
        var source = """
            function f(): i32 {
                let x: i32;
                return x;
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Contains(sema.Diagnostics, d => d.Message.Contains("may be uninitialized", StringComparison.OrdinalIgnoreCase));
    }

    [Fact]
    public void Allows_read_after_assignment()
    {
        var source = """
            function f(): i32 {
                let x: i32;
                x = 2;
                return x;
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Empty(sema.Diagnostics);
    }

    [Fact]
    public void Flags_read_when_only_assigned_in_then_branch()
    {
        var source = """
            function f(flag: bool): i32 {
                let x: i32;
                if (flag) {
                    x = 1;
                }
                return x;
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Contains(sema.Diagnostics, d => d.Message.Contains("may be uninitialized", StringComparison.OrdinalIgnoreCase));
    }

    [Fact]
    public void Allows_read_when_assigned_in_both_branches()
    {
        var source = """
            function f(flag: bool): i32 {
                let x: i32;
                if (flag) {
                    x = 1;
                } else {
                    x = 2;
                }
                return x;
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Empty(sema.Diagnostics);
    }

    [Fact]
    public void Allows_void_return_type()
    {
        var source = """
            function f(): void {
                return;
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Empty(sema.Diagnostics);
    }

    [Fact]
    public void Allows_global_struct()
    {
        var source = """
            struct Player { hp: u8; }
            global players: Player[10];
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Empty(sema.Diagnostics);
        Assert.Contains("players", sema.Symbols.Keys);
    }

    [Fact]
    public void Flags_assignment_to_literal()
    {
        var source = """
            function f(): void {
                5 = 3;
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Contains(sema.Diagnostics, d => d.Message.Contains("assignable location"));
    }

    [Fact]
    public void Flags_operator_wrong_arity()
    {
        var source = """
            function f(): void {
                x.+(1, 2);
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Contains(sema.Diagnostics, d => d.Message.Contains("requires exactly one argument"));
    }

    [Fact]
    public void Flags_undefined_identifier()
    {
        var source = """
            function f(): void {
                hp = 1;
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Contains(sema.Diagnostics, d => d.Message.Contains("Undefined identifier 'hp'"));
    }

    [Fact]
    public void Allows_enum_declaration()
    {
        var source = """
            enum State { Idle, Jump, Run }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Empty(sema.Diagnostics);
        Assert.Contains("State", sema.Symbols.Keys);
        Assert.Equal(SymbolKind.Enum, sema.Symbols["State"].Kind);
    }

    [Fact]
    public void Adds_enum_members_as_constants()
    {
        var source = """
            enum State { Idle, Jump, Run }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Empty(sema.Diagnostics);
        Assert.Contains("State.Idle", sema.Symbols.Keys);
        Assert.Contains("State.Jump", sema.Symbols.Keys);
        Assert.Contains("State.Run", sema.Symbols.Keys);
        Assert.Equal(SymbolKind.Const, sema.Symbols["State.Idle"].Kind);
    }

    [Fact]
    public void Allows_enum_member_access()
    {
        var source = """
            enum State { Idle, Jump }
            function check_state(): State {
                let x: State = State.Idle;
                return x;
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Empty(sema.Diagnostics);
    }

    [Fact]
    public void Flags_invalid_enum_member()
    {
        var source = """
            enum State { Idle, Jump }
            function check_invalid(): State {
                return State.Invalid;
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Contains(sema.Diagnostics, d => d.Message.Contains("does not have a member named 'Invalid'"));
    }

    [Fact]
    public void Allows_enum_comparison()
    {
        var source = """
            enum State { Idle, Jump }
            function compare_state(state: State): bool {
                return state == State.Idle;
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Empty(sema.Diagnostics);
    }

    [Fact]
    public void Allows_enum_variable()
    {
        var source = """
            enum State { Idle, Jump }
            function use_enum(): void {
                let state: State = State.Idle;
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Empty(sema.Diagnostics);
    }

    [Fact]
    public void Flags_integer_assignment_to_enum()
    {
        var source = """
            enum State { Idle, Jump }
            function bad_assignment(): void {
                let state: State = 0;
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Contains(sema.Diagnostics, d => d.Message.Contains("Cannot assign") || d.Message.Contains("type mismatch"));
    }

    [Fact]
    public void Flags_wrong_enum_type_assignment()
    {
        var source = """
            enum State { Idle, Jump }
            enum Direction { North, South }
            function wrong_enum(): void {
                let state: State = Direction.North;
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Contains(sema.Diagnostics, d => d.Message.Contains("Cannot assign") || d.Message.Contains("type mismatch"));
    }

    [Fact]
    public void Allows_enum_to_enum_assignment()
    {
        var source = """
            enum State { Idle, Jump }
            function reassign(): void {
                let state: State = State.Idle;
                state = State.Jump;
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Empty(sema.Diagnostics);
    }

    [Fact]
    public void Allows_enum_comparison_with_member()
    {
        var source = """
            enum State { Idle, Jump }
            function check(state: State): bool {
                return state == State.Idle;
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Empty(sema.Diagnostics);
    }

    [Fact]
    public void Flags_enum_comparison_with_integer()
    {
        var source = """
            enum State { Idle, Jump }
            function bad_compare(state: State): bool {
                return state == 0;
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Contains(sema.Diagnostics, d => d.Message.Contains("Cannot compare"));
    }

    [Fact]
    public void Flags_enum_comparison_with_different_enum()
    {
        var source = """
            enum State { Idle, Jump }
            enum Direction { North, South }
            function wrong_compare(state: State, dir: Direction): bool {
                return state == dir;
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Contains(sema.Diagnostics, d => d.Message.Contains("Cannot compare"));
    }

    [Fact]
    public void Allows_clear_on_global_array_and_struct()
    {
        var source = """
            struct Inner { a: i32; b: u8[4]; }
            struct Outer { x: i32; inner: Inner; bytes: u8[8]; }
            global arr: u8[8];
            global state: Outer;
            function f(): void {
                arr.clear();
                state.clear();
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Empty(sema.Diagnostics);
    }

    [Fact]
    public void Flags_clear_on_local()
    {
        var source = """
            function f(): void {
                let buf: u8[4];
                buf.clear();
            }
            """;

        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);

        Assert.Contains(sema.Diagnostics, d => d.Message.Contains("only supported on globals", StringComparison.OrdinalIgnoreCase));
    }

}
