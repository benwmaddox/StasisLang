package com.stasislang.workshop;

import android.content.Context;
import android.os.Build;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.File;
import java.io.FileInputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;

final class AndroidSupportBundle {
    private static final int MAX_TRACE_READ_BYTES = 512 * 1024;
    private static final int MAX_TRACE_EVENTS = 50;
    private static final int MAX_PROJECT_FILES = 1024;

    private AndroidSupportBundle() {}

    static String build(Context context, WorkshopProjectRegistry.ProjectInfo project, File projectRoot,
            String compileResult, String githubOperation, String githubState, JSONArray outcomes,
            File traceFile) throws Exception {
        JSONObject bundle = new JSONObject()
                .put("format", "stasis-android-redacted-support-v1")
                .put("generated_at_ms", System.currentTimeMillis())
                .put("redaction", new JSONObject()
                        .put("credentials_excluded", true)
                        .put("source_and_prompts_excluded", true)
                        .put("media_bytes_and_names_excluded", true)
                        .put("absolute_paths_excluded", true))
                .put("app", new JSONObject()
                        .put("package", context.getPackageName())
                        .put("version_name", BuildConfig.VERSION_NAME)
                        .put("version_code", BuildConfig.VERSION_CODE))
                .put("device", new JSONObject()
                        .put("manufacturer", Build.MANUFACTURER)
                        .put("model", Build.MODEL)
                        .put("sdk", Build.VERSION.SDK_INT)
                        .put("supported_abis", new JSONArray(Build.SUPPORTED_ABIS)))
                .put("project", projectSummary(project, projectRoot))
                .put("compile", compileSummary(compileResult))
                .put("github_operation", new JSONObject()
                        .put("operation", safeOperation(githubOperation))
                        .put("state", safeState(githubState)))
                .put("ai_outcomes", safeOutcomes(outcomes))
                .put("previous_crash", AndroidCrashStore.safeSummary(context))
                .put("trace_events", safeTraceEvents(traceFile));
        String json = bundle.toString(2);
        if (json.getBytes(StandardCharsets.UTF_8).length > 1024 * 1024) {
            throw new IllegalStateException("redacted support bundle exceeds 1 MiB");
        }
        return json;
    }

    private static JSONObject projectSummary(WorkshopProjectRegistry.ProjectInfo project, File root) throws Exception {
        int[] counts = new int[4];
        countProjectFiles(root, counts);
        return new JSONObject()
                .put("registered", project != null)
                .put("format_version", WorkshopProjectRegistry.FORMAT_VERSION)
                .put("origin", project == null ? "legacy" : safeOrigin(project.origin))
                .put("stasis_file_count", counts[0])
                .put("image_file_count", counts[1])
                .put("audio_file_count", counts[2])
                .put("other_file_count", counts[3]);
    }

    private static void countProjectFiles(File current, int[] counts) throws Exception {
        if (current == null || !current.exists()) return;
        if (current.isDirectory()) {
            File[] children = current.listFiles();
            if (children == null) return;
            for (File child : children) {
                if (counts[0] + counts[1] + counts[2] + counts[3] >= MAX_PROJECT_FILES) return;
                countProjectFiles(child, counts);
            }
            return;
        }
        String path = current.getAbsolutePath().replace(File.separatorChar, '/');
        if (path.endsWith(".stasis")) counts[0]++;
        else if (path.contains("/assets/images/")) counts[1]++;
        else if (path.contains("/assets/audio/")) counts[2]++;
        else counts[3]++;
    }

    private static JSONObject compileSummary(String result) throws Exception {
        String value = result == null ? "" : result;
        return new JSONObject()
                .put("attempted", !value.isEmpty() && !"CompileNotRun".equals(value))
                .put("runnable", value.startsWith("CompileReady") && value.contains("status=0"))
                .put("reload", safeReload(value));
    }

    private static String safeReload(String compileResult) {
        for (String value : new String[] {"FastReload", "NoChange", "ResetRequired", "InitialCompile"}) {
            if (compileResult.contains(value)) return value;
        }
        return compileResult.isEmpty() || "CompileNotRun".equals(compileResult) ? "not_run" : "failed_or_unknown";
    }

    private static JSONArray safeOutcomes(JSONArray outcomes) throws Exception {
        JSONArray safe = new JSONArray();
        if (outcomes == null) return safe;
        for (int index = 0; index < outcomes.length() && safe.length() < 20; index++) {
            JSONObject outcome = outcomes.optJSONObject(index);
            if (outcome == null) continue;
            safe.put(new JSONObject()
                    .put("timestamp_ms", outcome.optLong("timestamp_ms", 0L))
                    .put("status", safeState(outcome.optString("status", "unknown")))
                    .put("has_usage", !outcome.optString("usage", "").isEmpty()));
        }
        return safe;
    }

    private static JSONArray safeTraceEvents(File traceFile) throws Exception {
        JSONArray safe = new JSONArray();
        for (String line : tailLines(traceFile)) {
            try {
                JSONObject entry = new JSONObject(line);
                JSONObject summary = new JSONObject()
                        .put("timestamp_ms", entry.optLong("timestamp_ms", 0L))
                        .put("event", safeEvent(entry.optString("event", "unknown")));
                JSONObject data = entry.optJSONObject("data");
                if (data != null) {
                    if (data.has("turn")) summary.put("turn", Math.max(0, data.optInt("turn", 0)));
                    if (data.has("status")) summary.put("status", safeState(data.optString("status", "")));
                    if (data.has("elapsed")) summary.put("elapsed_present", !data.optString("elapsed", "").isEmpty());
                    if (data.has("action")) summary.put("action", safeState(data.optString("action", "")));
                }
                safe.put(summary);
            } catch (Exception ignored) {
                safe.put(new JSONObject().put("event", "unreadable_trace_entry"));
            }
        }
        return safe;
    }

    private static List<String> tailLines(File file) throws Exception {
        ArrayList<String> result = new ArrayList<>();
        if (file == null || !file.isFile()) return result;
        FileInputStream input = new FileInputStream(file);
        try {
            long skip = Math.max(0L, file.length() - MAX_TRACE_READ_BYTES);
            while (skip > 0L) {
                long skipped = input.skip(skip);
                if (skipped <= 0L) break;
                skip -= skipped;
            }
            byte[] bytes = new byte[(int)Math.min(MAX_TRACE_READ_BYTES, file.length())];
            int offset = 0;
            while (offset < bytes.length) {
                int read = input.read(bytes, offset, bytes.length - offset);
                if (read < 0) break;
                offset += read;
            }
            String[] lines = new String(bytes, 0, offset, StandardCharsets.UTF_8).split("\\r?\\n");
            int start = Math.max(0, lines.length - MAX_TRACE_EVENTS);
            for (int index = start; index < lines.length; index++) if (!lines[index].trim().isEmpty()) result.add(lines[index]);
            return result;
        } finally {
            input.close();
        }
    }

    private static String safeEvent(String value) { return safeEnum(value, "unknown_event"); }
    private static String safeState(String value) { return safeEnum(value, "unknown"); }
    private static String safeOperation(String value) { return safeEnum(value, "none"); }
    private static String safeOrigin(String value) { return safeEnum(value, "unknown"); }

    private static String safeEnum(String value, String fallback) {
        if (value == null || !value.matches("[A-Za-z0-9_-]{1,64}")) return fallback;
        return value;
    }
}
