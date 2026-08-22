package com.stasislang.workshop;

import android.util.Log;

import org.json.JSONArray;
import org.json.JSONObject;


/** Acceptance-only Rust/C/JNI/Java diagnostic round trip. */
final class WorkshopDiagnosticSeamAcceptance {
    private static final String LOG_TAG = "StasisWorkshop";
    private static final String MISSING_EXTERN = "extern function IT031_missing_extern(): void;";

    private WorkshopDiagnosticSeamAcceptance() {}

    static String[] caseNames() {
        return new String[] {"parse", "extern_resolution", "runtime_entry",
                "render_schema", "missing_resource"};
    }

    static String run(MainActivity activity, String projectRoot) {
        String original = null;
        JSONArray cases = new JSONArray();
        JSONObject baselineRuntime = null;
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
            String renderSchemaSource = original.replace(
                    "    pong_host_render();\n    return 0;\n}\n\nfunction on_code_swap()",
                    "    pong_host_render();\n    gfx_cmd_i32[1] = 99;\n    return 0;\n}\n\nfunction on_code_swap()");
            if (renderSchemaSource.equals(original)) {
                throw new IllegalStateException("render-schema mutation anchor was not found");
            }
            activity.acceptanceReplaceSource(projectRoot, renderSchemaSource);
            requireCompileReady(activity.acceptanceCompile(projectRoot), "render-schema setup");
            String renderMessage = activity.runIt031Frame(projectRoot);
            WorkshopNativeDiagnostic render = activity.acceptanceNativeDiagnostic(renderMessage);
            requireCode(render, "render_schema", "stasis.renderSchema");
            requireContext(render, null, "render", null);
            cases.put(caseEvidence(activity, "render_schema", render, renderMessage, null));

            String missingResource = original.replace(
                    "function on_code_swap(): void {\n    pong_game_on_code_swap();",
                    "function on_code_swap(): void {\n    load_sprite_from(PongHost.ball_sprite, "
                            + "\"assets/IT031_missing.svg\", 32, 32);\n"
                            + "    pong_game_on_code_swap();");
            if (missingResource.equals(original)) {
                throw new IllegalStateException("resource mutation anchor was not found");
            }
            activity.acceptanceReplaceSource(projectRoot, missingResource);
            requireCompileReady(activity.acceptanceCompile(projectRoot), "resource setup");
            String nativeResource = activity.runIt031Frame(projectRoot);
            WorkshopNativeDiagnostic resource = activity.acceptanceNativeDiagnostic(nativeResource);
            requireCode(resource, "resource", "stasis.missingResource");
            requireContext(resource, null, null, "assets/IT031_missing.svg");
            cases.put(caseEvidence(activity, "missing_resource", resource, nativeResource, null));

            JSONObject cleanup = restore(activity, projectRoot, original, baselineRuntime);
            JSONObject result = new JSONObject().put("schema", "stasis.workshop_diagnostic_seam.v1")
                    .put("test_id", "IT-031").put("event", "diagnostic_seam")
                    .put("status", "passed").put("ordered", true).put("cases", cases)
                    .put("cleanup_receipt", cleanup);
            Log.i(LOG_TAG, "Stasis Workshop IT-031: " + result);
            return result.toString();
        } catch (Exception error) {
            JSONObject cleanup = new JSONObject();
            try {
                if (original != null) cleanup = restore(activity, projectRoot, original, baselineRuntime);
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
                        .put("cases", cases).put("cleanup_receipt", cleanup).toString();
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
        JSONObject evidence = new JSONObject().put("name", name)
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
        return evidence;
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
            JSONObject baselineRuntime)
            throws Exception {
        activity.acceptanceReplaceSource(projectRoot, original);
        String compile = activity.acceptanceCompile(projectRoot);
        requireCompileReady(compile, "final cleanup compile");
        requireEquals("passed", activity.runIt031Frame(projectRoot), "final cleanup frame");
        JSONObject restoredRuntime = activity.acceptanceRuntimeState(projectRoot);
        requireRuntimeIdentity(restoredRuntime, "cleanup runtime");
        if (baselineRuntime != null
                && (!baselineRuntime.optString("source_fingerprint").equals(
                        restoredRuntime.optString("source_fingerprint"))
                        || restoredRuntime.optInt("generation", -1)
                        <= baselineRuntime.optInt("generation", -1))) {
            throw new IllegalStateException("cleanup runtime identity differs from baseline");
        }
        return new JSONObject().put("status", "Restored").put("compile", compile)
                .put("frame", "passed")
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
