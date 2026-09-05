package com.stasislang.workshop;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;

import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

public class WorkshopDiagnosticSeamAcceptanceTest {
    @Rule public final TemporaryFolder temporaryFolder = new TemporaryFolder();

    @Test public void casesAreTheSingleOrderedNativeSequence() {
        assertArrayEquals(new String[] {"parse", "extern_resolution", "runtime_entry",
                "render_schema", "missing_resource"},
                WorkshopDiagnosticSeamAcceptance.caseNames());
    }

    @Test public void mutationsUseFunctionBoundariesInsteadOfFixtureWhitespace() {
        String source = "function render(): i32 {\n"
                + "  pong_game_render();\n  return 0;\n}\n\n"
                + "function on_code_swap(): void {\n  pong_game_on_code_swap();\n}\n";
        String render = WorkshopDiagnosticSeamAcceptance.renderSchemaSource(source);
        assertTrue(render.indexOf("IT031_corrupt_render_schema();")
                < render.indexOf("return 0;"));
        assertFalse(render.contains("gfx_cmd_"));
        assertFalse(render.contains("/internal/gfx_cmd.stasis"));
        String resource = WorkshopDiagnosticSeamAcceptance.insertAfterInFunction(source,
                "function on_code_swap(): void {", "function on_code_swap(): void {",
                "\n  gfx_load_sprite(\"assets/IT031_missing.svg\", 32, 32);");
        String withExtern = WorkshopDiagnosticSeamAcceptance.ensureGfxLoadSpriteExtern(resource);
        assertTrue(withExtern.startsWith("extern function gfx_load_sprite(path: string, "
                + "max_w: i32, max_h: i32): i32;\n"));
        assertTrue(withExtern.contains("function on_code_swap(): void {\n"
                + "  gfx_load_sprite(\"assets/IT031_missing.svg\", 32, 32);"));
    }

    @Test public void renderSchemaPrivateAccessIsConfinedToFixedTestSeam() {
        assertEquals("tests/stasis/seams/it031_render_schema.stasis",
                WorkshopDiagnosticSeamAcceptance.RENDER_SCHEMA_HELPER_PATH);
        String helper = WorkshopDiagnosticSeamAcceptance.RENDER_SCHEMA_HELPER_SOURCE;
        assertTrue(helper.startsWith("import \"/.stasis_cache/toolchain/src/stdlib/internal/"
                + "gfx_cmd.stasis\";\n"));
        assertTrue(helper.contains("function IT031_corrupt_render_schema(): void {"));
        assertTrue(helper.contains("gfx_cmd_i32[1] = 99;"));

        String publicMutation = WorkshopDiagnosticSeamAcceptance.renderSchemaSource(
                "function render(): i32 { return 0; }\n");
        assertTrue(publicMutation.startsWith(
                "import \"/tests/stasis/seams/it031_render_schema.stasis\";\n"));
        assertFalse(publicMutation.contains("gfx_cmd_"));
        assertFalse(publicMutation.contains("/internal/gfx_cmd.stasis"));
    }

    @Test public void renderSchemaHelperRefusesOverwriteAndCleansUp() throws Exception {
        File project = temporaryFolder.newFolder("it031-project");
        WorkshopDiagnosticSeamAcceptance.createRenderSchemaHelper(project.getPath());
        File helper = new File(project,
                WorkshopDiagnosticSeamAcceptance.RENDER_SCHEMA_HELPER_PATH);
        assertEquals(WorkshopDiagnosticSeamAcceptance.RENDER_SCHEMA_HELPER_SOURCE,
                new String(Files.readAllBytes(helper.toPath()), StandardCharsets.UTF_8));
        assertThrows(IOException.class, () ->
                WorkshopDiagnosticSeamAcceptance.createRenderSchemaHelper(project.getPath()));
        assertEquals(WorkshopDiagnosticSeamAcceptance.RENDER_SCHEMA_HELPER_SOURCE,
                new String(Files.readAllBytes(helper.toPath()), StandardCharsets.UTF_8));

        WorkshopDiagnosticSeamAcceptance.deleteRenderSchemaHelper(project.getPath());
        assertFalse(helper.exists());
        WorkshopDiagnosticSeamAcceptance.deleteRenderSchemaHelper(project.getPath());
    }

    @Test public void resourceExternIsNotDuplicatedWhenAlreadyPresent() {
        String declaration = "extern function gfx_load_sprite(path: string, max_w: i32, "
                + "max_h: i32): i32;";
        String source = declaration + "\nfunction on_code_swap(): void {}\n";
        String ensured = WorkshopDiagnosticSeamAcceptance.ensureGfxLoadSpriteExtern(source);
        assertEquals(source, ensured);
        assertEquals(ensured.indexOf(declaration), ensured.lastIndexOf(declaration));
    }

    @Test public void resourceExternRecognizesFormattedRenamedDeclaration() {
        String source = "function @extern(\"host_gfx_load\") gfx_load_sprite(\r\n"
                + "  asset_path: string, width: i32, height: i32\r\n"
                + "): i32;\r\nfunction on_code_swap(): void {}\r\n";
        assertEquals(source, WorkshopDiagnosticSeamAcceptance.ensureGfxLoadSpriteExtern(source));
    }

    @Test public void resourceExternIgnoresCommentAndStringMentions() {
        String source = "// function gfx_load_sprite(path: string, w: i32, h: i32): i32;\n"
                + "const note: string = \"function gfx_load_sprite(path: string, w: i32, h: i32): i32;\";\n"
                + "function on_code_swap(): void {}\n";
        String ensured = WorkshopDiagnosticSeamAcceptance.ensureGfxLoadSpriteExtern(source);
        assertTrue(ensured.startsWith("extern function gfx_load_sprite(path: string, "
                + "max_w: i32, max_h: i32): i32;\n"));
    }

    @Test public void resourceExternIgnoresMultilineAndUnterminatedBlockComments() {
        String source = "/* function gfx_load_sprite(path: string, w: i32, h: i32): i32;\n"
                + "   still commented */\n"
                + "/* unterminated function gfx_load_sprite(path: string, w: i32, h: i32): i32;";
        String ensured = WorkshopDiagnosticSeamAcceptance.ensureGfxLoadSpriteExtern(source);
        assertTrue(ensured.startsWith("extern function gfx_load_sprite(path: string, "
                + "max_w: i32, max_h: i32): i32;\n"));
    }

    @Test public void renderMutationHandlesCrLfAndInlineBodies() {
        String source = "function render(): i32 { return 0; }\r\n"
                + "function later(): i32 { return 1; }\r\n";
        String mutated = WorkshopDiagnosticSeamAcceptance.renderSchemaSource(source);
        assertEquals("import \"/tests/stasis/seams/it031_render_schema.stasis\";\n"
                + "function render(): i32 { \n    IT031_corrupt_render_schema();\n"
                + "return 0; }\r\n"
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
