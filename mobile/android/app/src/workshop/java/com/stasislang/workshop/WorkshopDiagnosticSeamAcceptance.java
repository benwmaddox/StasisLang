package com.stasislang.workshop;

import android.util.Log;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;

import org.json.JSONArray;
import org.json.JSONObject;


/** Acceptance-only Rust/C/JNI/Java diagnostic round trip. */
final class WorkshopDiagnosticSeamAcceptance {
    private static final String LOG_TAG = "StasisWorkshop";
    private static final String MISSING_EXTERN = "extern function IT031_missing_extern(): void;";
    private static final String RESOURCE_EXTERN =
            "extern function gfx_load_sprite(path: string, max_w: i32, max_h: i32): i32;";
    static final String RENDER_SCHEMA_HELPER_PATH =
            "tests/stasis/seams/it031_render_schema.stasis";
    static final String RENDER_SCHEMA_HELPER_SOURCE =
            "import \"/.stasis_cache/toolchain/src/stdlib/internal/gfx_cmd.stasis\";\n\n"
                    + "function IT031_corrupt_render_schema(): void {\n"
                    + "    gfx_cmd_i32[1] = 99;\n"
                    + "}\n";
    private static final String RENDER_SCHEMA_IMPORT =
            "import \"/tests/stasis/seams/it031_render_schema.stasis\";";
    private static final String RENDER_SCHEMA_CALL = "IT031_corrupt_render_schema();";

    private WorkshopDiagnosticSeamAcceptance() {}

    static String[] caseNames() {
        return new String[] {"parse", "extern_resolution", "runtime_entry",
                "render_schema", "missing_resource"};
    }

    static String run(MainActivity activity, String projectRoot) {
        String original = null;
        JSONArray cases = new JSONArray();
        JSONObject baselineRuntime = null;
        boolean renderSchemaHelperOwned = false;
        try {
            original = activity.acceptanceReadSource(projectRoot);
            requireCompileReady(activity.acceptanceCompile(projectRoot), "baseline compile");
            requireEquals("passed", activity.runIt031Frame(projectRoot), "baseline frame");
            baselineRuntime = activity.acceptanceRuntimeState(projectRoot);
            requireRuntimeIdentity(baselineRuntime, "baseline runtime");
            runCompileCase(activity, projectRoot, original.substring(0,
                    Math.max(0, original.lastIndexOf('}'))), "parse", "stasis.parse", cases);
            String unresolvedExtern = original.replace("function tick(): i32 {",
                    "function tick(): i32 {\n    IT031_missing_extern();")
                    + "\n" + MISSING_EXTERN + "\n";
            runCompileCase(activity, projectRoot, unresolvedExtern,
                    "extern_resolution", "stasis.unresolvedExtern", cases);
            String badRuntime = original.replace("function tick(): i32", "function tick(value: i32): i32");
            activity.acceptanceReplaceSource(projectRoot, badRuntime);
            requireCompileReady(activity.acceptanceCompile(projectRoot), "runtime-entry setup");
            String runtimeMessage = activity.runIt031Frame(projectRoot);
            WorkshopNativeDiagnostic runtime = activity.acceptanceNativeDiagnostic(runtimeMessage);
            requireCode(runtime, "runtime_entry", "stasis.runtimeEntry");
            requireContext(runtime, null, "tick", null);
            cases.put(caseEvidence(activity, "runtime_entry", runtime, runtimeMessage, null));

            activity.acceptanceReplaceSource(projectRoot, original);
            requireCompileReady(activity.acceptanceCompile(projectRoot), "render-schema baseline");
            requireEquals("passed", activity.runIt031Frame(projectRoot), "render-schema baseline frame");
            createRenderSchemaHelper(projectRoot);
            renderSchemaHelperOwned = true;
            String renderSchemaSource = renderSchemaSource(original);
            activity.acceptanceReplaceSource(projectRoot, renderSchemaSource);
            requireCompileReady(activity.acceptanceCompile(projectRoot), "render-schema setup");
            String renderMessage = activity.runIt031Frame(projectRoot);
            WorkshopNativeDiagnostic render = activity.acceptanceNativeDiagnostic(renderMessage);
            requireCode(render, "render_schema", "stasis.renderSchema");
            requireContext(render, null, "render", null);
            cases.put(caseEvidence(activity, "render_schema", render, renderMessage, null));

            String resourceSource = ensureGfxLoadSpriteExtern(original);
            String missingResource = insertAfterInFunction(resourceSource,
                    "function on_code_swap(): void {", "function on_code_swap(): void {",
                    "\n    gfx_load_sprite(\"assets/IT031_missing.svg\", 32, 32);");
            activity.acceptanceReplaceSource(projectRoot, missingResource);
            requireCompileReady(activity.acceptanceCompile(projectRoot), "resource setup");
            String nativeResource = activity.runIt031Frame(projectRoot);
            WorkshopNativeDiagnostic resource = activity.acceptanceNativeDiagnostic(nativeResource);
            requireCode(resource, "resource", "stasis.missingResource");
            requireContext(resource, null, null, "assets/IT031_missing.svg");
            cases.put(caseEvidence(activity, "missing_resource", resource, nativeResource, null));

            JSONObject cleanup = restore(activity, projectRoot, original, baselineRuntime,
                    renderSchemaHelperOwned);
            renderSchemaHelperOwned = false;
            JSONObject result = new JSONObject().put("schema", "stasis.workshop_diagnostic_seam.v1")
                    .put("test_id", "IT-031").put("event", "diagnostic_seam")
                    .put("status", "passed").put("ordered", true).put("cases", cases)
                    .put("cleanup_receipt", cleanup);
            JSONObject summary = new JSONObject().put("schema", "stasis.workshop_diagnostic_seam.v1")
                    .put("test_id", "IT-031").put("event", "diagnostic_seam")
                    .put("status", "passed").put("ordered", true)
                    .put("case_count", cases.length()).put("case_names", caseNames(cases))
                    .put("cleanup_receipt", cleanup);
            Log.i(LOG_TAG, "Stasis Workshop IT-031: " + summary);
            return result.toString();
        } catch (Exception error) {
            JSONObject cleanup = new JSONObject();
            try {
                if (original != null) {
                    cleanup = restore(activity, projectRoot, original, baselineRuntime,
                            renderSchemaHelperOwned);
                } else if (renderSchemaHelperOwned) {
                    deleteRenderSchemaHelper(projectRoot);
                }
            } catch (Exception cleanupError) {
                try { cleanup.put("status", "failed").put("error", cleanupError.toString()); }
                catch (Exception ignored) { }
            }
            String reason = error.getMessage() == null ? error.getClass().getSimpleName()
                    : error.getMessage();
            String output;
            try {
                output = new JSONObject().put("schema", "stasis.workshop_diagnostic_seam.v1")
                        .put("test_id", "IT-031").put("event", "diagnostic_seam")
                        .put("status", "failed").put("error", reason)
                        .put("case_count", cases.length()).put("case_names", caseNames(cases))
                        .put("cleanup_receipt", cleanup).toString();
            } catch (Exception serializationError) {
                output = "{\"schema\":\"stasis.workshop_diagnostic_seam.v1\","
                        + "\"test_id\":\"IT-031\",\"event\":\"diagnostic_seam\","
                        + "\"status\":\"failed\",\"error\":" + JSONObject.quote(reason) + "}";
            }
            Log.e(LOG_TAG, "Stasis Workshop IT-031: " + output);
            return output;
        }
    }

    private static void runCompileCase(MainActivity activity, String projectRoot, String source,
            String expectedStage, String expectedCode, JSONArray cases) throws Exception {
        activity.acceptanceReplaceSource(projectRoot, source);
        String nativeMessage = activity.acceptanceCompile(projectRoot);
        WorkshopNativeDiagnostic nativeDiagnostic = activity.acceptanceNativeDiagnostic(nativeMessage);
        requireCode(nativeDiagnostic, expectedStage, expectedCode);
        requireContext(nativeDiagnostic,
                ("parse".equals(expectedStage) || "extern_resolution".equals(expectedStage))
                        ? "src/main.stasis" : null,
                "extern_resolution".equals(expectedStage) ? "IT031_missing_extern"
                        : ("runtime_entry".equals(expectedStage) ? "tick"
                                : ("parse".equals(expectedStage) ? "on_code_swap" : null)), null);
        JSONObject expectedLocation = "parse".equals(expectedStage)
                ? expectedParseLocation(source) : null;
        cases.put(caseEvidence(activity, expectedStage, nativeDiagnostic, nativeMessage,
                expectedLocation));
    }

    static String insertAfterInFunction(String source, String declaration, String anchor,
            String insertion) {
        int functionStart = source.lastIndexOf(declaration);
        int nextFunction = functionStart < 0 ? -1
                : source.indexOf("\nfunction ", functionStart + declaration.length());
        int anchorStart = functionStart < 0 ? -1 : source.indexOf(anchor, functionStart);
        if (functionStart < 0 || anchorStart < 0
                || (nextFunction >= 0 && anchorStart >= nextFunction)) {
            throw new IllegalStateException("mutation anchor was not found in " + declaration);
        }
        int insertionPoint = anchorStart + anchor.length();
        return source.substring(0, insertionPoint) + insertion + source.substring(insertionPoint);
    }

    static String ensureGfxLoadSpriteExtern(String source) {
        if (hasTopLevelGfxLoadSpriteDeclaration(source)) return source;
        return RESOURCE_EXTERN + "\n" + source;
    }

    private static boolean hasTopLevelGfxLoadSpriteDeclaration(String source) {
        String code = maskStringsAndComments(source);
        int depth = 0;
        boolean functionDeclaration = false;
        int index = 0;
        while (index < code.length()) {
            char current = code.charAt(index);
            if (current == '{') {
                depth++;
                functionDeclaration = false;
                index++;
                continue;
            }
            if (current == '}') {
                depth = Math.max(0, depth - 1);
                functionDeclaration = false;
                index++;
                continue;
            }
            if (current == ';') {
                functionDeclaration = false;
                index++;
                continue;
            }
            if (!Character.isJavaIdentifierStart(current)) {
                index++;
                continue;
            }
            int end = index + 1;
            while (end < code.length() && Character.isJavaIdentifierPart(code.charAt(end))) {
                end++;
            }
            String token = code.substring(index, end);
            if (depth == 0 && "function".equals(token)) {
                functionDeclaration = true;
            } else if (depth == 0 && functionDeclaration
                    && "gfx_load_sprite".equals(token)) {
                int after = end;
                while (after < code.length() && Character.isWhitespace(code.charAt(after))) {
                    after++;
                }
                if (after < code.length() && code.charAt(after) == '(') return true;
            }
            index = end;
        }
        return false;
    }

    private static String maskStringsAndComments(String source) {
        char[] masked = source.toCharArray();
        boolean inString = false;
        boolean escaped = false;
        boolean inLineComment = false;
        boolean inBlockComment = false;
        for (int index = 0; index < masked.length; index++) {
            char current = masked[index];
            if (inLineComment) {
                if (current == '\n') inLineComment = false;
                else masked[index] = ' ';
                continue;
            }
            if (inBlockComment) {
                if (current == '*' && index + 1 < masked.length && masked[index + 1] == '/') {
                    masked[index] = ' ';
                    masked[++index] = ' ';
                    inBlockComment = false;
                } else if (current != '\n') {
                    masked[index] = ' ';
                }
                continue;
            }
            if (inString) {
                if (escaped) escaped = false;
                else if (current == '\\') escaped = true;
                else if (current == '"') inString = false;
                if (current != '\n') masked[index] = ' ';
                continue;
            }
            if (current == '/' && index + 1 < masked.length && masked[index + 1] == '/') {
                masked[index] = ' ';
                masked[++index] = ' ';
                inLineComment = true;
            } else if (current == '/' && index + 1 < masked.length && masked[index + 1] == '*') {
                masked[index] = ' ';
                masked[++index] = ' ';
                inBlockComment = true;
            } else if (current == '"') {
                masked[index] = ' ';
                inString = true;
            }
        }
        return new String(masked);
    }

    static String insertBeforeFunctionAnchor(String source, String declaration, String anchor,
            String insertion) {
        int functionStart = source.lastIndexOf(declaration);
        if (functionStart < 0) {
            throw new IllegalStateException("function declaration was not found in " + declaration);
        }
        int bodyStart = functionStart + declaration.length() - 1;
        if (bodyStart < functionStart || bodyStart >= source.length()
                || source.charAt(bodyStart) != '{') {
            throw new IllegalStateException("function body was not found in " + declaration);
        }
        int depth = 0;
        int anchorStart = -1;
        boolean inString = false;
        boolean escaped = false;
        boolean inLineComment = false;
        for (int index = bodyStart; index < source.length(); index++) {
            char current = source.charAt(index);
            if (inLineComment) {
                if (current == '\n') inLineComment = false;
                continue;
            }
            if (inString) {
                if (escaped) escaped = false;
                else if (current == '\\') escaped = true;
                else if (current == '"') inString = false;
                continue;
            }
            if (current == '/' && index + 1 < source.length()
                    && source.charAt(index + 1) == '/') {
                inLineComment = true;
                index++;
                continue;
            }
            if (current == '"') {
                inString = true;
                continue;
            }
            if (current == '{') {
                depth++;
                continue;
            }
            if (current == '}') {
                depth--;
                if (depth == 0) break;
                continue;
            }
            if (depth == 1 && startsFunctionDeclaration(source, index)) {
                throw new IllegalStateException("function body crossed next top-level function in "
                        + declaration);
            }
            if (source.startsWith(anchor, index)) {
                anchorStart = index;
                index += anchor.length() - 1;
            }
        }
        if (depth != 0) {
            throw new IllegalStateException("function closing brace was not found in " + declaration);
        }
        if (anchorStart < 0) {
            throw new IllegalStateException("function anchor was not found in " + declaration);
        }
        return source.substring(0, anchorStart) + insertion + source.substring(anchorStart);
    }

    static String renderSchemaSource(String source) {
        String imported = RENDER_SCHEMA_IMPORT + "\n" + source;
        return insertBeforeFunctionAnchor(imported, "function render(): i32 {", "return 0;",
                "\n    " + RENDER_SCHEMA_CALL + "\n");
    }

    static void createRenderSchemaHelper(String projectRoot) throws IOException {
        File helper = renderSchemaHelperFile(projectRoot);
        File parent = helper.getParentFile();
        if (!parent.isDirectory() && !parent.mkdirs() && !parent.isDirectory()) {
            throw new IOException("could not create IT-031 render-schema helper directory");
        }
        if (!helper.createNewFile()) {
            throw new IOException("refusing to overwrite IT-031 render-schema helper: "
                    + RENDER_SCHEMA_HELPER_PATH);
        }
        boolean written = false;
        try (FileOutputStream output = new FileOutputStream(helper, false)) {
            output.write(RENDER_SCHEMA_HELPER_SOURCE.getBytes(StandardCharsets.UTF_8));
            written = true;
        } finally {
            if (!written && helper.exists() && !helper.delete()) {
                Log.e(LOG_TAG, "could not remove incomplete IT-031 render-schema helper");
            }
        }
    }

    static void deleteRenderSchemaHelper(String projectRoot) throws IOException {
        File helper = renderSchemaHelperFile(projectRoot);
        if (helper.exists() && !helper.delete()) {
            throw new IOException("could not remove IT-031 render-schema helper");
        }
    }

    private static File renderSchemaHelperFile(String projectRoot) throws IOException {
        File root = new File(projectRoot).getCanonicalFile();
        File helper = new File(root, RENDER_SCHEMA_HELPER_PATH).getCanonicalFile();
        if (!helper.getPath().startsWith(root.getPath() + File.separator)) {
            throw new IOException("IT-031 render-schema helper escaped project root");
        }
        return helper;
    }

    private static boolean startsFunctionDeclaration(String source, int index) {
        if (index + "function".length() > source.length()
                || !source.startsWith("function", index)) return false;
        if (index > 0 && Character.isJavaIdentifierPart(source.charAt(index - 1))) return false;
        int after = index + "function".length();
        return after == source.length()
                || !Character.isJavaIdentifierPart(source.charAt(after));
    }

    private static JSONObject caseEvidence(MainActivity activity, String name,
            WorkshopNativeDiagnostic nativeDiagnostic, String nativeMessage,
            JSONObject expectedLocation) throws Exception {
        JSONObject display = activity.acceptanceDisplayDiagnostic(nativeMessage);
        String displayedText = display.optString("displayed_text", "");
        WorkshopNativeDiagnostic uiDiagnostic = WorkshopNativeDiagnostic.fromJson(
                display.optJSONObject("diagnostic"));
        if (nativeDiagnostic == null || uiDiagnostic == null
                || !nativeDiagnostic.equals(uiDiagnostic)) {
            throw new IllegalStateException(name + " Java diagnostic differs from native envelope");
        }
        if (nativeDiagnostic.detail == null || !displayedText.contains(nativeDiagnostic.detail)) {
            throw new IllegalStateException(name + " UI display lost native detail");
        }
        JSONObject evidence = new JSONObject().put("test_id", "IT-031")
                .put("name", name)
                .put("native", nativeDiagnostic.toJson())
                .put("ui", uiDiagnostic.toJson()).put("displayed_text", displayedText)
                .put("equal", true);
        if ("parse".equals(name)) {
            JSONObject actualLocation = parseLocation(nativeMessage);
            if (!expectedLocation.toString().equals(actualLocation.toString())) {
                throw new IllegalStateException("parse diagnostic location differs from source: expected "
                        + expectedLocation + " actual " + actualLocation);
            }
            evidence.put("location", new JSONObject().put("expected", expectedLocation)
                    .put("actual", actualLocation));
        }
        Log.i(LOG_TAG, "Stasis Workshop IT-031 case: " + evidence);
        return evidence;
    }

    private static JSONArray caseNames(JSONArray cases) throws Exception {
        JSONArray names = new JSONArray();
        for (int index = 0; index < cases.length(); index++) {
            JSONObject evidence = cases.optJSONObject(index);
            if (evidence != null) names.put(evidence.optString("name", ""));
        }
        return names;
    }

    private static JSONObject expectedParseLocation(String source) throws Exception {
        int start = source.lastIndexOf("function on_code_swap");
        if (start < 0) throw new IllegalStateException("parse source lost on_code_swap");
        return locationForOffset(source, start, source.length());
    }

    private static JSONObject locationForOffset(String source, int start, int end) throws Exception {
        int line = 1;
        int lineStart = 0;
        for (int index = 0; index < start; index++) {
            if (source.charAt(index) == '\n') { line++; lineStart = index + 1; }
        }
        int endLine = line;
        int endLineStart = lineStart;
        for (int index = start; index < end; index++) {
            if (source.charAt(index) == '\n') { endLine++; endLineStart = index + 1; }
        }
        return new JSONObject().put("line", line).put("column", start - lineStart + 1)
                .put("end_line", endLine).put("end_column", end - endLineStart + 1);
    }

    private static JSONObject parseLocation(String message) throws Exception {
        int line = parsePositiveField(message, "diagnostic_line");
        int column = parsePositiveField(message, "diagnostic_column");
        int endLine = parsePositiveField(message, "diagnostic_end_line");
        int endColumn = parsePositiveField(message, "diagnostic_end_column");
        if (endLine < line || (endLine == line && endColumn < column)) {
            throw new IllegalStateException("parse diagnostic span is reversed");
        }
        return new JSONObject().put("line", line).put("column", column)
                .put("end_line", endLine).put("end_column", endColumn);
    }

    private static int parsePositiveField(String message, String key) {
        String marker = "|" + key + "=";
        int start = message.indexOf(marker);
        if (start < 0) throw new IllegalStateException("parse diagnostic lost " + key);
        start += marker.length();
        int end = message.indexOf('|', start);
        String value = end < 0 ? message.substring(start) : message.substring(start, end);
        int parsed;
        try { parsed = Integer.parseInt(value); }
        catch (NumberFormatException error) {
            throw new IllegalStateException("parse diagnostic has invalid " + key);
        }
        if (parsed <= 0) throw new IllegalStateException("parse diagnostic has zero " + key);
        return parsed;
    }

    private static JSONObject restore(MainActivity activity, String projectRoot, String original,
            JSONObject baselineRuntime, boolean renderSchemaHelperOwned)
            throws Exception {
        Exception restoreFailure = null;
        try {
            activity.acceptanceReplaceSource(projectRoot, original);
        } catch (Exception error) {
            restoreFailure = error;
        }
        if (renderSchemaHelperOwned) {
            try {
                deleteRenderSchemaHelper(projectRoot);
            } catch (Exception error) {
                if (restoreFailure == null) restoreFailure = error;
                else restoreFailure.addSuppressed(error);
            }
        }
        if (restoreFailure != null) throw restoreFailure;
        String compile = activity.acceptanceCompile(projectRoot);
        requireCompileReady(compile, "final cleanup compile");
        requireEquals("passed", activity.runIt031Frame(projectRoot), "final cleanup frame");
        JSONObject restoredRuntime = activity.acceptanceRuntimeState(projectRoot);
        requireRuntimeIdentity(restoredRuntime, "cleanup runtime");
        JSONObject ui = activity.acceptanceRecoverAfterHealthyFrame(compile);
        if (ui.optBoolean("blocking_error_visible", true)
                || !ui.optBoolean("status_healthy", false)
                || !ui.optBoolean("compile_ready", false)
                || !ui.optBoolean("compile_attempted", false)
                || !ui.optBoolean("game_runtime_active", false)
                || ui.optString("displayed_status", "").isEmpty()) {
            throw new IllegalStateException("cleanup UI recovery did not clear blocking status: " + ui);
        }
        if (baselineRuntime != null
                && (!baselineRuntime.optString("source_fingerprint").equals(
                        restoredRuntime.optString("source_fingerprint"))
                        || restoredRuntime.optInt("generation", -1)
                        <= baselineRuntime.optInt("generation", -1))) {
            throw new IllegalStateException("cleanup runtime identity differs from baseline");
        }
        return new JSONObject().put("status", "Restored").put("compile", compile)
                .put("frame", "passed")
                .put("ui", ui)
                .put("source_fingerprint", restoredRuntime.optString("source_fingerprint"))
                .put("generation", restoredRuntime.optInt("generation", -1))
                .put("baseline_source_fingerprint", baselineRuntime == null ? JSONObject.NULL
                        : baselineRuntime.optString("source_fingerprint"))
                .put("baseline_generation", baselineRuntime == null ? JSONObject.NULL
                        : baselineRuntime.optInt("generation", -1));
    }

    private static void requireRuntimeIdentity(JSONObject runtime, String phase) {
        if (runtime == null || !"live_session".equals(runtime.optString("source"))
                || runtime.optInt("generation", 0) <= 0
                || runtime.optString("source_fingerprint", "").isEmpty()) {
            throw new IllegalStateException(phase + " lacks runtime identity: " + runtime);
        }
    }

    private static void requireCode(WorkshopNativeDiagnostic diagnostic, String stage, String code) {
        if (diagnostic == null || !stage.equals(diagnostic.stage) || !code.equals(diagnostic.code)
                || diagnostic.detail == null || diagnostic.detail.isEmpty()
                || diagnostic.causes.isEmpty()) {
            throw new IllegalStateException(stage + " diagnostic lost native detail: " + diagnostic);
        }
    }

    private static void requireCompileReady(String value, String phase) {
        if (value == null || !value.startsWith("CompileReady") || !value.contains("status=0")) {
            throw new IllegalStateException(phase + " did not compile: " + value);
        }
    }

    private static void requireContext(WorkshopNativeDiagnostic diagnostic, String file,
            String symbol, String resource) {
        if (file != null && !file.equals(diagnostic.file)
                || symbol != null && !symbol.equals(diagnostic.symbol)
                || resource != null && !resource.equals(diagnostic.resource)) {
            throw new IllegalStateException("diagnostic context was not preserved: " + diagnostic);
        }
    }

    private static void requireEquals(String expected, String actual, String phase) {
        if (!expected.equals(actual)) throw new IllegalStateException(phase + " failed: " + actual);
    }
}
