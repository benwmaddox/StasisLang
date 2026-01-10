using System;
using System.IO;
using Xunit;

namespace Stasis.Compiler.Tests;

public class StdlibSignatureTests
{
    [Fact]
    public void GraphicsStdlib_GfxDrawSpriteSignature_MatchesRuntime()
    {
        var repoRoot = FindRepoRoot();
        var path = Path.Combine(repoRoot, "src", "stdlib", "graphics.stasis");
        var content = File.ReadAllText(path);

        Assert.Contains(
            "extern function gfx_draw_sprite(handle: i32, x: i32, y: i32, w: i32, h: i32, rot_deg: i32, a: i32): void;",
            content,
            StringComparison.Ordinal);
    }

    private static string FindRepoRoot()
    {
        var dir = new DirectoryInfo(AppContext.BaseDirectory);
        while (dir != null && !File.Exists(Path.Combine(dir.FullName, "Stasis.sln")))
        {
            dir = dir.Parent;
        }

        if (dir == null)
        {
            throw new DirectoryNotFoundException("Could not find repo root (Stasis.sln).");
        }

        return dir.FullName;
    }
}
