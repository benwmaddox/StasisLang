package com.stasislang.workshop;

import android.content.Context;
import android.os.Build;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.File;
import java.nio.charset.StandardCharsets;

final class AndroidSupportBundle {
    private static final int MAX_PROJECT_FILES = 1024;

    private AndroidSupportBundle() {}

    static String build(Context context, WorkshopProjectRegistry.ProjectInfo project, File projectRoot,
            String compileResult, String githubOperation, String githubState) throws Exception {
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
                .put("previous_crash", AndroidCrashStore.safeSummary(context));
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

    private static String safeState(String value) { return safeEnum(value, "unknown"); }
    private static String safeOperation(String value) { return safeEnum(value, "none"); }
    private static String safeOrigin(String value) { return safeEnum(value, "unknown"); }

    private static String safeEnum(String value, String fallback) {
        if (value == null || !value.matches("[A-Za-z0-9_-]{1,64}")) return fallback;
        return value;
    }
}
