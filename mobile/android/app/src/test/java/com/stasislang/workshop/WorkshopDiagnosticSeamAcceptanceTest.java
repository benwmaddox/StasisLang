package com.stasislang.workshop;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

import org.json.JSONObject;
import org.junit.Test;

public class WorkshopDiagnosticSeamAcceptanceTest {

    @Test public void casesAreTheSingleOrderedNativeSequence() {
        assertArrayEquals(new String[] {"parse", "extern_resolution", "runtime_entry",
                "render_construction", "missing_resource"},
                WorkshopDiagnosticSeamAcceptance.caseNames());
    }

    @Test public void mutationsUseFunctionBoundariesInsteadOfFixtureWhitespace() {
        String source = "function render(): i32 {\n"
                + "  pong_game_render();\n  return 0;\n}\n\n"
                + "function on_code_swap(): void {\n  pong_game_on_code_swap();\n}\n";
        String render = WorkshopDiagnosticSeamAcceptance.renderConstructionSource(source);
        assertTrue(render.indexOf("return 17;") < render.indexOf("pong_game_render();"));
        assertFalse(render.contains("gfx_cmd_"));
        assertFalse(render.contains("/internal/gfx_cmd.stasis"));
        String resource = WorkshopDiagnosticSeamAcceptance.missingResourceSource(source);
        assertTrue(resource.contains("function on_code_swap(): void {\n"
                + "    state.opaque.load_sprite_from(\"assets/IT031_missing.svg\", 32, 32);\n"));
        assertFalse(resource.contains("extern"));
        assertFalse(resource.contains("gfx_load_sprite"));
        assertFalse(resource.contains("let missing: Sprite"));

    }

    @Test public void renderConstructionMutationUsesOnlyPublicSource() {
        String mutated = WorkshopDiagnosticSeamAcceptance.renderConstructionSource(
                "function render(): i32 { pong_game_render(); return 0; }\n");
        assertTrue(mutated.startsWith("function render(): i32 {\n    return 17;\n"));
        assertFalse(mutated.contains("import"));
        assertFalse(mutated.contains("gfx_cmd_"));
    }

    @Test public void renderMutationHandlesCrLfAndInlineBodies() {
        String source = "function render(): i32 { return 0; }\r\n"
                + "function later(): i32 { return 1; }\r\n";
        String mutated = WorkshopDiagnosticSeamAcceptance.renderConstructionSource(source);
        assertEquals("function render(): i32 {\n    return 17;\n return 0; }\r\n"
                + "function later(): i32 { return 1; }\r\n", mutated);
    }

    @Test public void renderMutationRejectsMissingDeclaration() {
        assertThrows(IllegalStateException.class, () ->
                WorkshopDiagnosticSeamAcceptance.renderConstructionSource(
                        "function later(): i32 { return 1; }\n"));
    }

    @Test public void logEvidenceKeepsStructuredProofWithoutUnboundedDisplayText()
            throws Exception {
        JSONObject evidence = new JSONObject()
                .put("native", new JSONObject().put("detail", "specific detail"))
                .put("ui", new JSONObject().put("detail", "specific detail"))
                .put("displayed_text", "x".repeat(8000));
        JSONObject bounded = WorkshopDiagnosticSeamAcceptance.boundedLogEvidence(
                evidence, "specific detail");
        assertFalse(bounded.has("displayed_text"));
        assertEquals("specific detail", bounded.getString("displayed_detail"));
        assertTrue(bounded.getBoolean("displayed_text_contains_detail"));
        assertTrue(evidence.has("displayed_text"));
    }
}
