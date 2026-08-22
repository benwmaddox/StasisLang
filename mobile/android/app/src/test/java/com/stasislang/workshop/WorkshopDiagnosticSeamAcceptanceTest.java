package com.stasislang.workshop;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public class WorkshopDiagnosticSeamAcceptanceTest {
    @Test public void casesAreTheSingleOrderedNativeSequence() {
        assertArrayEquals(new String[] {"parse", "extern_resolution", "runtime_entry",
                "render_schema", "missing_resource"},
                WorkshopDiagnosticSeamAcceptance.caseNames());
    }

    @Test public void mutationsUseFunctionBoundariesInsteadOfFixtureWhitespace() {
        String source = "function render(): i32 {\n"
                + "  pong_game_render();\n  pong_host_render();\n  return 0;\n}\n\n"
                + "function on_code_swap(): void {\n  pong_game_on_code_swap();\n}\n";
        String render = WorkshopDiagnosticSeamAcceptance.insertAfterInFunction(source,
                "function render(): i32 {", "pong_host_render();", "\n  gfx_cmd_i32[1] = 99;");
        assertTrue(render.contains("pong_host_render();\n  gfx_cmd_i32[1] = 99;"));
        String resource = WorkshopDiagnosticSeamAcceptance.insertAfterInFunction(source,
                "function on_code_swap(): void {", "function on_code_swap(): void {",
                "\n  load_missing();");
        assertTrue(resource.contains("function on_code_swap(): void {\n  load_missing();"));
    }
}
