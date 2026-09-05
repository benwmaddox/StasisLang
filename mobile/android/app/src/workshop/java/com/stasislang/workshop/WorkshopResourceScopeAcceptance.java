package com.stasislang.workshop;

import android.util.Log;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;

/** Acceptance-only proof that project and GLES generations own Workshop resources. */
final class WorkshopResourceScopeAcceptance {
    private static final String LOG_TAG = "StasisWorkshop";
    private static final String SCHEMA = "stasis.workshop_resource_scope.v1";
    private static final String CACHED_TEXT_MARKER =
            "state.label.load_text_from(state.font, \"cached parity\");";
    private static final String DIRECT_TEXT_MARKER =
            "draw_text(font, \"direct parity\", 64.0, 252.0, 0.95, 0.95, 1.0, 1.0);";

    private WorkshopResourceScopeAcceptance() {}

    static String run(MainActivity activity) {
        WorkshopProjectRegistry.ProjectInfo original = null;
        WorkshopProjectRegistry.ProjectInfo alpha = null;
        WorkshopProjectRegistry.ProjectInfo beta = null;
        try {
            original = activeProject(activity);
            alpha = WorkshopProjectRegistry.createFromTemplate(activity,
                    "IT029 Alpha", WorkshopTemplateCatalog.RENDER_ACCEPTANCE_TEMPLATE_ID);
            beta = WorkshopProjectRegistry.createFromTemplate(activity,
                    "IT029 Beta", WorkshopTemplateCatalog.RENDER_ACCEPTANCE_TEMPLATE_ID);
            activity.resetIt029ResourceMetrics();

            JSONObject alphaFirst = activateCustomizeRender(activity, alpha, "alpha", 1);
            logCase(alphaFirst);
            JSONObject betaBefore = activateCustomizeRender(activity, beta, "beta", 2);
            logCase(betaBefore);

            int generationBefore = betaBefore.getJSONObject("resources")
                    .getInt("lifecycle_renderer_generation");
            // Isolate the surface-restore epoch from ordinary project activation uploads.
            activity.resetIt029ResourceMetrics();
            if (!activity.recreateIt029Surface()) {
                throw new IllegalStateException("real EGL context recreation did not advance");
            }
            JSONObject betaAfter = activity.runIt029Frame(beta.root.getAbsolutePath(),
                    "project_b_after_recreation", 3);
            logCase(betaAfter);

            if (!activity.activateProject(alpha)) {
                throw new IllegalStateException("could not switch back to IT-029 alpha");
            }
            JSONObject alphaReturn = activity.runIt029Frame(alpha.root.getAbsolutePath(),
                    "project_a_return", 4);
            logCase(alphaReturn);

            validate(alphaFirst, betaBefore, betaAfter, alphaReturn, generationBefore);
            JSONObject cleanup = cleanup(activity, original, alpha, beta);
            if (!"Restored".equals(cleanup.optString("status"))) {
                throw new IllegalStateException("IT-029 cleanup failed: "
                        + cleanup.optString("error", "unknown cleanup error"));
            }
            JSONObject result = new JSONObject()
                    .put("schema", SCHEMA).put("test_id", "IT-029")
                    .put("event", "resource_scope").put("status", "passed")
                    .put("ordered", true).put("same_handles", true)
                    .put("distinct_projects", true).put("distinct_assets", true)
                    .put("surface_recreated", true).put("restore_once", true)
                    .put("bounded", true)
                    .put("captures", new JSONArray()
                            .put(alphaFirst.getString("capture_path"))
                            .put(betaBefore.getString("capture_path"))
                            .put(betaAfter.getString("capture_path"))
                            .put(alphaReturn.getString("capture_path")))
                    .put("cleanup", cleanup);
            original = null;
            alpha = null;
            beta = null;
            Log.i(LOG_TAG, "Stasis Workshop IT-029: " + result);
            return result.toString();
        } catch (Exception error) {
            JSONObject cleanup = cleanup(activity, original, alpha, beta);
            String reason = error.getMessage() == null ? error.getClass().getSimpleName()
                    : error.getMessage();
            JSONObject failed = new JSONObject();
            try {
                failed.put("schema", SCHEMA).put("test_id", "IT-029")
                        .put("event", "resource_scope").put("status", "failed")
                        .put("error", reason).put("cleanup", cleanup);
            } catch (Exception ignored) {
                // Scalar JSON construction cannot fail.
            }
            Log.e(LOG_TAG, "Stasis Workshop IT-029: " + failed);
            return failed.toString();
        }
    }

    private static void logCase(JSONObject value) {
        Log.i(LOG_TAG, "Stasis Workshop IT-029 case: " + value);
    }

    private static JSONObject activateCustomizeRender(MainActivity activity,
            WorkshopProjectRegistry.ProjectInfo project, String identity, int sequence)
            throws Exception {
        if (!activity.activateProject(project)) {
            throw new IllegalStateException("could not activate IT-029 " + identity);
        }
        customize(project.root, identity);
        String compile = activity.acceptanceCompile(project.root.getAbsolutePath());
        if (compile == null || !compile.startsWith("CompileReady")
                || !compile.contains("status=0")) {
            throw new IllegalStateException("IT-029 " + identity + " compile failed: " + compile);
        }
        return activity.runIt029Frame(project.root.getAbsolutePath(),
                "project_" + ("alpha".equals(identity) ? "a" : "b")
                        + ("alpha".equals(identity) ? "_first" : "_before_recreation"),
                sequence);
    }

    private static void customize(File root, String identity) throws Exception {
        boolean alpha = "alpha".equals(identity);
        File mainFile = new File(root, "src/main.stasis");
        String cachedText = alpha ? "cached alpha!" : "cached beta!!";
        String main = replaceRequiredOnce(read(mainFile), CACHED_TEXT_MARKER,
                CACHED_TEXT_MARKER.replace("cached parity", cachedText), "cached text");
        write(mainFile, main);

        File frameFile = new File(root, "src/frame.stasis");
        String frame = customizeDirectText(read(frameFile),
                alpha ? "scope alpha!!" : "scope beta!!!");
        write(frameFile, frame);

        File sprite = new File(root, "assets/full_canvas.svg");
        String color = alpha ? "#1261a0" : "#a03812";
        String svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"640\" height=\"360\""
                + " viewBox=\"0 0 640 360\"><rect width=\"640\" height=\"360\" fill=\""
                + color + "\"/><circle cx=\"320\" cy=\"180\" r=\"72\" fill=\"#ffffff\""
                + " fill-opacity=\"0.22\"/></svg>\n";
        write(sprite, svg);
        File manifestFile = new File(root, WorkshopAssetManifest.RELATIVE_PATH);
        JSONObject manifest = new JSONObject(read(manifestFile));
        JSONArray assets = manifest.getJSONArray("assets");
        for (int index = 0; index < assets.length(); index += 1) {
            JSONObject asset = assets.getJSONObject(index);
            if ("assets/full_canvas.svg".equals(asset.optString("path"))) {
                asset.put("content_sha256", sha256(svg.getBytes(StandardCharsets.UTF_8)));
            }
        }
        write(manifestFile, manifest.toString(2) + "\n");
    }

    static String customizeDirectText(String source, String text) {
        if (text.getBytes(StandardCharsets.UTF_8).length != 13) {
            throw new IllegalArgumentException("IT-029 text must be 13 UTF-8 bytes");
        }
        return replaceRequiredOnce(source, DIRECT_TEXT_MARKER,
                DIRECT_TEXT_MARKER.replace("direct parity", text), "direct text");
    }

    private static String replaceRequiredOnce(String source, String marker, String replacement,
            String description) {
        int first = source.indexOf(marker);
        if (first < 0) {
            throw new IllegalStateException("IT-029 " + description + " marker is missing");
        }
        if (source.indexOf(marker, first + marker.length()) >= 0) {
            throw new IllegalStateException("IT-029 " + description + " marker is ambiguous");
        }
        return source.substring(0, first) + replacement + source.substring(first + marker.length());
    }

    private static void validate(JSONObject alphaFirst, JSONObject betaBefore,
            JSONObject betaAfter, JSONObject alphaReturn, int generationBefore) throws Exception {
        JSONObject alphaResources = alphaFirst.getJSONObject("resources");
        JSONObject betaResources = betaBefore.getJSONObject("resources");
        JSONObject restored = betaAfter.getJSONObject("resources");
        if (alphaFirst.getString("project_root").equals(betaBefore.getString("project_root"))) {
            throw new IllegalStateException("project roots collided");
        }
        for (String field : new String[] {"sprite_handles", "font_handles",
                "cached_text_handles"}) {
            if (!alphaFirst.getJSONArray(field).toString()
                    .equals(betaBefore.getJSONArray(field).toString())) {
                throw new IllegalStateException(field + " did not intentionally collide");
            }
        }
        String alphaText = alphaFirst.getString("direct_text_sha256");
        String betaText = betaBefore.getString("direct_text_sha256");
        if (alphaText.equals(betaText)
                || alphaFirst.getString("capture_sha256")
                        .equals(betaBefore.getString("capture_sha256"))) {
            throw new IllegalStateException("project asset/text captures did not prove identity");
        }
        if (!betaText.equals(betaAfter.getString("direct_text_sha256"))
                || !alphaText.equals(alphaReturn.getString("direct_text_sha256"))) {
            throw new IllegalStateException("surface/project restore changed direct text identity");
        }
        long alphaTrace = alphaFirst.getLong("command_trace");
        long betaTrace = betaBefore.getLong("command_trace");
        if (alphaTrace == betaTrace
                || betaTrace != betaAfter.getLong("command_trace")
                || alphaTrace != alphaReturn.getLong("command_trace")) {
            throw new IllegalStateException("surface/project restore changed logical command trace");
        }
        if (!betaResources.getJSONArray("identities").toString()
                        .equals(restored.getJSONArray("identities").toString())
                || !alphaResources.getJSONArray("identities").toString()
                        .equals(alphaReturn.getJSONObject("resources")
                                .getJSONArray("identities").toString())
                || alphaResources.getJSONArray("identities").toString()
                        .equals(betaResources.getJSONArray("identities").toString())) {
            throw new IllegalStateException("resource identities crossed a project or surface epoch");
        }
        if (restored.getInt("lifecycle_renderer_generation") <= generationBefore
                || restored.getInt("stale_generation_rejections") <= 0
                || !restored.getBoolean("resources_ready")) {
            throw new IllegalStateException("stale generation was not rejected and restored");
        }
        if (restored.getInt("duplicate_restore_uploads") != 0
                || restored.getInt("restore_uploads") != 5) {
            throw new IllegalStateException("a resource restored more than once in its epoch");
        }
        if (restored.getInt("maximum_atlas_pages") > 2
                || restored.getInt("maximum_live_regions") > 6
                || restored.getInt("maximum_text_textures") > 2
                || restored.getInt("maximum_font_entries") > 1
                || alphaResources.getInt("project_switches") > 3
                || betaResources.getInt("project_switches") > 3) {
            throw new IllegalStateException("resource counts exceeded IT-029 bounds");
        }
    }

    private static WorkshopProjectRegistry.ProjectInfo activeProject(MainActivity activity)
            throws Exception {
        String root = new File(activity.projectRootPath()).getCanonicalPath();
        for (WorkshopProjectRegistry.ProjectInfo project : WorkshopProjectRegistry.list(activity)) {
            if (project.root.getCanonicalPath().equals(root)) return project;
        }
        throw new IllegalStateException("active project is not registered");
    }

    private static JSONObject cleanup(MainActivity activity,
            WorkshopProjectRegistry.ProjectInfo original,
            WorkshopProjectRegistry.ProjectInfo alpha,
            WorkshopProjectRegistry.ProjectInfo beta) {
        JSONObject result = new JSONObject();
        try {
            if (original != null && !activity.activateProject(original)) {
                throw new IllegalStateException("could not restore packaged project");
            }
            if (original != null) {
                JSONObject healthy = activity.runIt029Frame(original.root.getAbsolutePath(),
                        "packaged_cleanup", 5);
                result.put("frame_status", healthy.getString("status"))
                        .put("frame_token", healthy.getInt("frame_token"));
            }
            if (alpha != null && alpha.root.exists()) WorkshopProjectRegistry.deleteProject(activity, alpha);
            if (beta != null && beta.root.exists()) WorkshopProjectRegistry.deleteProject(activity, beta);
            result.put("status", "Restored");
        } catch (Exception error) {
            try {
                result.put("status", "failed").put("error", error.getMessage());
            } catch (Exception ignored) {
                // Scalar JSON construction cannot fail.
            }
        }
        return result;
    }

    private static String read(File file) throws Exception {
        FileInputStream input = new FileInputStream(file);
        try {
            byte[] bytes = new byte[(int)file.length()];
            int offset = 0;
            while (offset < bytes.length) {
                int count = input.read(bytes, offset, bytes.length - offset);
                if (count < 0) throw new IllegalStateException("short read: " + file);
                offset += count;
            }
            return new String(bytes, StandardCharsets.UTF_8);
        } finally {
            input.close();
        }
    }

    private static void write(File file, String text) throws Exception {
        FileOutputStream output = new FileOutputStream(file);
        try {
            output.write(text.getBytes(StandardCharsets.UTF_8));
            output.getFD().sync();
        } finally {
            output.close();
        }
    }

    private static String sha256(byte[] bytes) throws Exception {
        byte[] digest = MessageDigest.getInstance("SHA-256").digest(bytes);
        StringBuilder hex = new StringBuilder();
        for (byte value : digest) hex.append(String.format(java.util.Locale.US, "%02x", value & 0xff));
        return hex.toString();
    }
}
