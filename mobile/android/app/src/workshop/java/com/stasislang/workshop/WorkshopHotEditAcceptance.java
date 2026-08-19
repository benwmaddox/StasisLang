package com.stasislang.workshop;

import android.util.Log;

import org.json.JSONArray;
import org.json.JSONObject;

/** Acceptance-only proof of transactional Workshop hot-edit publication. */
final class WorkshopHotEditAcceptance {
    private static final String LOG_TAG = "StasisWorkshop";
    private static final String TICK_REVISION = "const IT028_TICK_REVISION: i32 = 1;";
    private static final String RENDER_REVISION = "const IT028_RENDER_REVISION: i32 = 1;";

    private WorkshopHotEditAcceptance() {}

    static String run(MainActivity activity, String projectRoot) {
        String original = null;
        String accepted = null;
        boolean acceptanceEnabled = false;
        try {
            original = activity.acceptanceReadSource(projectRoot);
            if (!original.endsWith("\n")) {
                throw new IllegalStateException("packaged source must end with a newline");
            }
            accepted = original.replace(TICK_REVISION,
                    "const IT028_TICK_REVISION: i32 = 2;")
                    .replace(RENDER_REVISION,
                            "const IT028_RENDER_REVISION: i32 = 2;");
            if (accepted.equals(original)) {
                throw new IllegalStateException("tagged IT-028 source revisions were not found");
            }

            requireRuntimeWrite(activity.acceptanceSetRuntimeI32(projectRoot,
                    "seam_it028_enabled", 1), "enable acceptance state");
            acceptanceEnabled = true;
            JSONObject baseline = new JSONObject(activity.runIt028Frame(projectRoot,
                    "baseline", 1));
            requirePassed(baseline, "baseline");
            logCase(baseline);

            activity.acceptanceReplaceSource(projectRoot, accepted);
            String validCompile = activity.acceptanceCompile(projectRoot);
            requireCompileReady(validCompile, "valid publication");
            JSONObject published = new JSONObject(activity.runIt028Frame(projectRoot,
                    "published", 2));
            requirePassed(published, "published");
            logCase(published);

            String lineEnding = accepted.contains("\r\n") ? "\r\n" : "\n";
            String originalHook = "function on_code_swap(): void {" + lineEnding
                    + "    return;" + lineEnding + "}";
            String invalidHook = "function on_code_swap(): void {" + lineEnding
                    + "    IT028_missing_target(); return;" + lineEnding + "}";
            if (accepted.indexOf(originalHook) < 0
                    || accepted.indexOf(originalHook) != accepted.lastIndexOf(originalHook)) {
                throw new IllegalStateException("expected exactly one on_code_swap hook");
            }
            int hookLine = sourceLineOf(accepted, originalHook);
            String invalid = accepted.replace(originalHook, invalidHook);
            activity.acceptanceReplaceSource(projectRoot, invalid);
            String invalidCompile = activity.acceptanceCompile(projectRoot);
            if (!invalidCompile.startsWith("CompileError")) {
                throw new IllegalStateException("invalid edit unexpectedly compiled");
            }
            JSONObject invalidDiagnostic = activity.acceptanceCompileDiagnostic(invalidCompile);
            invalidDiagnostic.remove("raw");
            JSONObject diagnostic = invalidDiagnostic.optJSONObject("diagnostic");
            JSONObject expectedDiagnostic = expectedDiagnostic(hookLine);
            if (diagnostic == null || !diagnosticsEqual(diagnostic, expectedDiagnostic)) {
                throw new IllegalStateException("invalid edit omitted exact structured diagnostic");
            }

            JSONObject rolledBack = new JSONObject(activity.runIt028Frame(projectRoot,
                    "post_invalid", 3));
            requirePassed(rolledBack, "post-invalid");
            logCase(rolledBack);

            activity.acceptanceReplaceSource(projectRoot, accepted);
            String restoreReceipt = activity.acceptanceCompile(projectRoot);
            requireNoChangeReceipt(restoreReceipt);
            JSONObject summary = validateCases(baseline, published, rolledBack);
            summary.put("hook_source_line", hookLine);
            summary.put("invalid_compile", invalidDiagnostic);
            summary.put("restore_receipt", new JSONObject()
                    .put("status", "NoChange")
                    .put("compile", restoreReceipt));

            JSONObject cleanup = restoreOriginal(activity, projectRoot, original, acceptanceEnabled,
                    baseline, published);
            acceptanceEnabled = false;
            summary.put("cleanup_receipt", cleanup);
            JSONArray cases = new JSONArray().put(baseline).put(published).put(rolledBack);
            JSONObject result = new JSONObject()
                    .put("schema", "stasis.workshop_hot_edit.v1")
                    .put("test_id", "IT-028").put("event", "hot_edit")
                    .put("status", "passed").put("ordered", true).put("unique", true)
                    .put("atomic", true).put("cases", cases)
                    .put("hook_source_line", hookLine)
                    .put("invalid_compile", invalidDiagnostic)
                    .put("restore_receipt", summary.getJSONObject("restore_receipt"))
                    .put("cleanup_receipt", cleanup);
            Log.i(LOG_TAG, "Stasis Workshop IT-028: " + result);
            return result.toString();
        } catch (Exception error) {
            JSONObject cleanup = bestEffortRestore(activity, projectRoot, original, acceptanceEnabled);
            String reason = error.getMessage() == null ? error.getClass().getSimpleName()
                    : error.getMessage();
            if (!"Restored".equals(cleanup.optString("status"))) {
                reason += "; IT-028 cleanup failed: " + cleanup.optString("error");
            }
            return failed(reason, cleanup);
        }
    }

    private static void logCase(JSONObject frame) {
        Log.i(LOG_TAG, "Stasis Workshop IT-028 case: " + frame);
    }

    private static JSONObject restoreOriginal(MainActivity activity, String projectRoot,
            String original, boolean acceptanceEnabled, JSONObject baseline,
            JSONObject published) throws Exception {
        if (acceptanceEnabled) {
            requireRuntimeWrite(activity.acceptanceSetRuntimeI32(projectRoot,
                    "seam_it028_enabled", 0), "disable acceptance state");
        }
        activity.acceptanceReplaceSource(projectRoot, original);
        String cleanupCompile = activity.acceptanceCompile(projectRoot);
        requireCompileReady(cleanupCompile, "original-source cleanup");
        JSONObject cleanupFrame = new JSONObject(activity.runIt028Frame(projectRoot, "cleanup", 4));
        requirePassed(cleanupFrame, "cleanup");
        JSONObject cleanupRuntime = cleanupFrame.getJSONObject("runtime");
        JSONObject baselineRuntime = baseline.getJSONObject("runtime");
        JSONObject publishedRuntime = published.getJSONObject("runtime");
        if (!baselineRuntime.getString("source_fingerprint")
                        .equals(cleanupRuntime.getString("source_fingerprint"))
                || cleanupRuntime.getLong("generation")
                        != publishedRuntime.getLong("generation") + 1
                || cleanupFrame.getJSONObject("render").getJSONObject("marker")
                        .optBoolean("active", true)) {
            throw new IllegalStateException("original-source cleanup did not restore inactive packaged code");
        }
        return new JSONObject().put("status", "Restored")
                .put("compile", cleanupCompile).put("frame", cleanupFrame);
    }

    private static JSONObject bestEffortRestore(MainActivity activity, String projectRoot,
            String original, boolean acceptanceEnabled) {
        JSONObject outcome = new JSONObject();
        String failure = null;
        if (acceptanceEnabled) {
            try {
                String disable = activity.acceptanceSetRuntimeI32(projectRoot,
                        "seam_it028_enabled", 0);
                requireRuntimeWrite(disable, "cleanup disable acceptance state");
                outcome.put("disable", disable);
            } catch (Exception cleanupError) {
                failure = cleanupErrorMessage(cleanupError);
            }
        }
        if (original == null) {
            failure = appendCleanupFailure(failure,
                    "original source was unavailable for cleanup");
        } else {
            try {
                activity.acceptanceReplaceSource(projectRoot, original);
                String cleanupCompile = activity.acceptanceCompile(projectRoot);
                requireCompileReady(cleanupCompile, "cleanup original source");
                outcome.put("compile", cleanupCompile);
                JSONObject cleanupFrame = new JSONObject(activity.runIt028Frame(
                        projectRoot, "cleanup", 4));
                requirePassed(cleanupFrame, "cleanup");
                if (cleanupFrame.getJSONObject("render").getJSONObject("marker")
                        .optBoolean("active", true)) {
                    throw new IllegalStateException("cleanup frame retained acceptance marker: "
                            + cleanupFrame);
                }
                outcome.put("frame", cleanupFrame);
            } catch (Exception cleanupError) {
                failure = appendCleanupFailure(failure, cleanupErrorMessage(cleanupError));
            }
        }
        if (failure == null) {
            try {
                outcome.put("status", "Restored");
            } catch (Exception ignored) {
                // JSONObject construction cannot fail for this scalar field.
            }
        } else {
            try {
                outcome.put("status", "failed").put("error", failure);
            } catch (Exception ignored) {
                // JSONObject construction cannot fail for these scalar fields.
            }
            Log.e(LOG_TAG, "Stasis Workshop IT-028 cleanup failed: " + failure);
        }
        return outcome;
    }

    private static String cleanupErrorMessage(Exception error) {
        return error.getMessage() == null ? error.getClass().getSimpleName() : error.getMessage();
    }

    private static String appendCleanupFailure(String previous, String current) {
        return previous == null ? current : previous + "; " + current;
    }

    private static JSONObject validateCases(JSONObject baseline, JSONObject published,
            JSONObject rolledBack) throws Exception {
        JSONObject baselineRuntime = baseline.getJSONObject("runtime");
        JSONObject publishedRuntime = published.getJSONObject("runtime");
        JSONObject rolledBackRuntime = rolledBack.getJSONObject("runtime");
        long baselineGeneration = baselineRuntime.getLong("generation");
        long publishedGeneration = publishedRuntime.getLong("generation");
        long rolledBackGeneration = rolledBackRuntime.getLong("generation");
        String baselineSource = baselineRuntime.getString("source_fingerprint");
        String publishedSource = publishedRuntime.getString("source_fingerprint");
        String rolledBackSource = rolledBackRuntime.getString("source_fingerprint");
        if (publishedGeneration != baselineGeneration + 1
                || rolledBackGeneration != publishedGeneration
                || baselineSource.equals(publishedSource)
                || !publishedSource.equals(rolledBackSource)) {
            throw new IllegalStateException("generation/source publication was not atomic");
        }
        JSONObject baselineGuest = baseline.getJSONObject("guest");
        JSONObject publishedGuest = published.getJSONObject("guest");
        JSONObject rolledBackGuest = rolledBack.getJSONObject("guest");
        if (baselineGuest.getInt("tick_revision") != 1
                || baselineGuest.getInt("render_revision") != 1
                || baselineGuest.getInt("state_counter") != 1
                || publishedGuest.getInt("tick_revision") != 2
                || publishedGuest.getInt("render_revision") != 2
                || publishedGuest.getInt("state_counter") != 2
                || rolledBackGuest.getInt("tick_revision") != 2
                || rolledBackGuest.getInt("render_revision") != 2
                || rolledBackGuest.getInt("state_counter") != 3) {
            throw new IllegalStateException("tick/render revision markers mixed generations");
        }
        long baselineTrace = baseline.getJSONObject("render").getLong("trace");
        long publishedTrace = published.getJSONObject("render").getLong("trace");
        long rolledBackTrace = rolledBack.getJSONObject("render").getLong("trace");
        if (baselineTrace == publishedTrace || publishedTrace != rolledBackTrace) {
            throw new IllegalStateException("render traces did not prove publication boundary");
        }
        return new JSONObject().put("baseline_generation", baselineGeneration)
                .put("published_generation", publishedGeneration)
                .put("rolled_back_generation", rolledBackGeneration)
                .put("baseline_source", baselineSource)
                .put("published_source", publishedSource);
    }

    private static void requireNoChangeReceipt(String result) {
        if (result == null || !result.startsWith("CompileReady")
                || !result.contains("reload=NoChange") || !result.contains("status=0")) {
            throw new IllegalStateException("accepted source restore was not exact NoChange: " + result);
        }
    }

    private static int sourceLineOf(String source, String text) {
        int offset = source.indexOf(text);
        if (offset < 0) throw new IllegalStateException("hook text was not found");
        int line = 1;
        for (int index = 0; index < offset; index += 1) {
            if (source.charAt(index) == '\n') line += 1;
        }
        return line;
    }

    private static JSONObject expectedDiagnostic(int hookLine) throws Exception {
        return new JSONObject().put("file", "src/main.stasis")
                .put("line", hookLine).put("column", 31)
                .put("end_line", hookLine + 2).put("end_column", 2)
                .put("symbol", "on_code_swap")
                .put("message", "unknown call target 'IT028_missing_target'");
    }

    private static boolean diagnosticsEqual(JSONObject actual, JSONObject expected) {
        if (actual.length() != expected.length()) return false;
        for (String key : new String[] {"file", "line", "column", "end_line", "end_column",
                "symbol", "message"}) {
            if (!expected.opt(key).equals(actual.opt(key))) return false;
        }
        return true;
    }

    private static void requirePassed(JSONObject frame, String phase) throws Exception {
        if (!"passed".equals(frame.optString("status"))
            || frame.optBoolean("java_only", true)
            || frame.optInt("fallback", -1) != 0
            || frame.optInt("stub", -1) != 0) {
            throw new IllegalStateException("IT-028 " + phase
                    + " frame was not native evidence: " + frame);
        }
    }

    private static void requireCompileReady(String result, String phase) {
        if (result == null || !result.startsWith("CompileReady") || !result.contains("status=0")) {
            throw new IllegalStateException(phase + " did not return CompileReady");
        }
    }

    private static void requireRuntimeWrite(String result, String phase) {
        if (result == null || !result.startsWith("StateSet:") || result.startsWith("StateError:")) {
            throw new IllegalStateException(phase + " failed: " + result);
        }
    }

    private static String failed(String reason, JSONObject cleanup) {
        String output = "{\"schema\":\"stasis.workshop_hot_edit.v1\","
                + "\"test_id\":\"IT-028\",\"event\":\"hot_edit\","
                + "\"status\":\"failed\",\"error\":" + JSONObject.quote(reason);
        if (cleanup != null) output += ",\"cleanup\":" + cleanup;
        output += "}";
        Log.e(LOG_TAG, "Stasis Workshop IT-028: " + output);
        return output;
    }
}
