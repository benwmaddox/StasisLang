using Stasis.Compiler.Layout;
using Stasis.Compiler.Semantic;

namespace Stasis.Compiler.Tests;

public class LayoutTests
{
    private LayoutPlan Plan(string source)
    {
        var parse = Parser.Parse(source);
        var sema = new SemanticAnalyzer().Analyze(parse.CompilationUnit);
        return new LayoutPlanner(parse.CompilationUnit, sema.Symbols).Plan();
    }

    [Fact]
    public void Computes_soa_offsets_for_struct_array()
    {
        var plan = Plan("""
            struct Player { hp: u8; score: i32; }
            global players: Player[2];
            """);

        var global = Assert.Single(plan.Globals);
        Assert.Equal("players", global.Name);
        Assert.Equal(2 + 8, global.Size); // hp (2 bytes) + score (aligned to 4, 8 bytes)

        Assert.Collection(global.Fields,
            f =>
            {
                Assert.Equal("Player__hp", f.Name);
                Assert.Equal(0, f.Offset);
                Assert.Equal(2, f.Size);
            },
            f =>
            {
                Assert.Equal("Player__score", f.Name);
                Assert.Equal(4, f.Offset); // aligned to 4 after 2 bytes
                Assert.Equal(8, f.Size);
            });
    }

    [Fact]
    public void Computes_primitive_array_layout()
    {
        var plan = Plan("""
            global temps: f32[3];
            """);

        var global = Assert.Single(plan.Globals);
        Assert.Equal("temps", global.Name);
        Assert.Equal(12, global.Size);
        Assert.Equal(0, global.Fields.Single().Offset);
    }

    [Fact]
    public void Computes_primitive_scalar_layout()
    {
        var plan = Plan("""
            global flag: bool;
            """);

        var global = Assert.Single(plan.Globals);
        Assert.Equal("flag", global.Name);
        Assert.Equal(1, global.Size);
        Assert.Equal(0, global.Fields.Single().Offset);
    }

    [Fact]
    public void Computes_utf8_buffer_layout()
    {
        var plan = Plan("""
            global name: utf8[16];
            """);

        var global = Assert.Single(plan.Globals);
        var field = Assert.Single(global.Fields);
        Assert.Equal("name", global.Name);
        Assert.Equal(FieldType.String, field.Type);
        Assert.Equal(16, field.ArrayCount);
        Assert.Equal(8 + 16, field.Size);
        Assert.Equal(0, field.Offset);
    }

    [Fact]
    public void Treats_enum_as_i32_in_layout()
    {
        var plan = Plan("""
            enum E { A, B }
            global es: E[3];
            """);

        var global = Assert.Single(plan.Globals);
        var field = Assert.Single(global.Fields);
        Assert.Equal(FieldType.I32, field.Type);
        Assert.Equal(3, field.ArrayCount);
        Assert.Equal(12, field.Size);
    }
}
