using Stasis.Compiler.IR;
using Stasis.Compiler.Syntax;

namespace Stasis.Compiler.Tests;

public class ReachabilityTests
{
    [Fact]
    public void Selects_matching_receiver_overload_for_function_form_calls()
    {
        var compilationUnit = Parse("""
            struct Enemy { hp: i32; }
            struct Hero { hp: i32; }

            function damage(enemy: Enemy, amount: i32): i32 {
                return amount;
            }

            function damage(hero: Hero, amount: i32): i32 {
                return amount.+(100);
            }

            function main(): i32 {
                let enemy: Enemy = 0;
                return damage(enemy, 1);
            }
            """);

        var reachable = Reachability.CollectReachableFunctions(compilationUnit, includeTests: false, allowFallback: false);

        Assert.Contains("main|<none>", reachable);
        Assert.Contains("damage|Enemy", reachable);
        Assert.DoesNotContain("damage|Hero", reachable);
    }

    [Fact]
    public void Selects_matching_receiver_overload_for_receiver_form_calls()
    {
        var compilationUnit = Parse("""
            struct Enemy { hp: i32; }
            struct Hero { hp: i32; }

            function damage(enemy: Enemy, amount: i32): i32 {
                return amount;
            }

            function damage(hero: Hero, amount: i32): i32 {
                return amount.+(100);
            }

            function main(): i32 {
                let enemy: Enemy = 0;
                return enemy.damage(1);
            }
            """);

        var reachable = Reachability.CollectReachableFunctions(compilationUnit, includeTests: false, allowFallback: false);

        Assert.Contains("main|<none>", reachable);
        Assert.Contains("damage|Enemy", reachable);
        Assert.DoesNotContain("damage|Hero", reachable);
    }

    [Fact]
    public void Does_not_mark_function_reachable_for_shadowed_local_call_name()
    {
        var compilationUnit = Parse("""
            struct Enemy { hp: i32; }

            function damage(enemy: Enemy, amount: i32): i32 {
                return amount;
            }

            function main(): i32 {
                let enemy: Enemy = 0;
                let damage: i32 = 0;
                return damage(enemy, 1);
            }
            """);

        var reachable = Reachability.CollectReachableFunctions(compilationUnit, includeTests: false, allowFallback: false);

        Assert.Contains("main|<none>", reachable);
        Assert.DoesNotContain("damage|Enemy", reachable);
    }

    [Fact]
    public void Selects_matching_receiver_overload_for_nested_call_first_argument()
    {
        var compilationUnit = Parse("""
            struct Enemy { hp: i32; }
            struct Hero { hp: i32; }

            function make_enemy(): Enemy {
                let enemy: Enemy = 0;
                return enemy;
            }

            function damage(enemy: Enemy, amount: i32): i32 {
                return amount;
            }

            function damage(hero: Hero, amount: i32): i32 {
                return amount.+(100);
            }

            function main(): i32 {
                return damage(make_enemy(), 1);
            }
            """);

        var reachable = Reachability.CollectReachableFunctions(compilationUnit, includeTests: false, allowFallback: false);

        Assert.Contains("main|<none>", reachable);
        Assert.Contains("make_enemy|<none>", reachable);
        Assert.Contains("damage|Enemy", reachable);
        Assert.DoesNotContain("damage|Hero", reachable);
    }

    private static CompilationUnitSyntax Parse(string source)
    {
        var parse = Parser.Parse(source);
        Assert.Empty(parse.Diagnostics);
        return parse.CompilationUnit;
    }
}
