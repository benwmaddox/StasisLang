package com.stasislang.workshop;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertThrows;
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
                + "  pong_game_render();\n  return 0;\n}\n\n"
                + "function on_code_swap(): void {\n  pong_game_on_code_swap();\n}\n";
        String render = WorkshopDiagnosticSeamAcceptance.insertBeforeFunctionAnchor(source,
                "function render(): i32 {", "return 0;", "\n  gfx_cmd_i32[1] = 99;\n");
        assertTrue(render.indexOf("gfx_cmd_i32[1] = 99;") < render.indexOf("return 0;"));
        String resource = WorkshopDiagnosticSeamAcceptance.insertAfterInFunction(source,
                "function on_code_swap(): void {", "function on_code_swap(): void {",
                "\n  load_missing();");
        assertTrue(resource.contains("function on_code_swap(): void {\n  load_missing();"));
    }

    @Test public void renderMutationHandlesCrLfAndInlineBodies() {
        String source = "function render(): i32 { return 0; }\r\n"
                + "function later(): i32 { return 1; }\r\n";
        String mutated = WorkshopDiagnosticSeamAcceptance.insertBeforeFunctionAnchor(source,
                "function render(): i32 {", "return 0;", " gfx_cmd_i32[1] = 99; ");
        assertEquals("function render(): i32 {  gfx_cmd_i32[1] = 99; return 0; }\r\n"
                + "function later(): i32 { return 1; }\r\n", mutated);
    }

    @Test public void renderMutationRejectsMissingBodyBoundary() {
        assertThrows(IllegalStateException.class, () ->
                WorkshopDiagnosticSeamAcceptance.insertBeforeFunctionAnchor(
                        "function render(): i32 { gfx_cmd_i32[0] = 1; }\n",
                        "function render(): i32 {", "return 0;", " mutation; "));
        String source = "function render(): i32 { return 0;\n"
                + "function later(): i32 { return 1; }\n";
        assertThrows(IllegalStateException.class, () ->
                WorkshopDiagnosticSeamAcceptance.insertBeforeFunctionAnchor(source,
                        "function render(): i32 {", "return 0;", " mutation; "));
    }
}
