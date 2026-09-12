package com.stasislang.workshop;

import android.util.Log;

import org.json.JSONArray;
import org.json.JSONObject;

/** Acceptance-only bounded Workshop edit/surface soak. */
final class WorkshopSoakAcceptance {
    private static final String LOG_TAG = "StasisWorkshop";
    static final int FRAME_COUNT = 300;
    static final int[] EDIT_FRAMES = {75, 150, 225, 300};
    static final int[] SURFACE_FRAMES = {100, 200};
    private static final String TICK = "const IT028_TICK_REVISION: i32 = 1;";
    private static final String RENDER = "const IT028_RENDER_REVISION: i32 = 1;";

    private WorkshopSoakAcceptance() {}

    static String sourceForRevision(String original, int revision) {
        if (revision == 1) return original;
        String changed = original.replace(TICK,
                "const IT028_TICK_REVISION: i32 = " + revision + ";")
                .replace(RENDER,
                        "const IT028_RENDER_REVISION: i32 = " + revision + ";");
        if (changed.equals(original)) {
            throw new IllegalStateException("tagged IT-032 revision constants were not found");
        }
        return changed;
    }

    static int revisionAt(int frame) {
        if (frame >= 225 && frame < 300) return 4;
        if (frame >= 150 && frame < 225) return 3;
        if (frame >= 75 && frame < 150) return 2;
        return 1;
    }

    static boolean isMarker(int frame, int[] markers) {
        for (int marker : markers) if (marker == frame) return true;
        return false;
    }

    static String run(MainActivity activity, String projectRoot) {
        String original = null;
        boolean enabled = false;
        boolean soakActive = false;
        int[] originalGuest = null;
        int completed = 0;
        try {
            original = activity.acceptanceReadSource(projectRoot);
            requireCompile(activity.acceptanceCompile(projectRoot), "baseline");
            originalGuest = new int[] {
                    activity.acceptanceRuntimeI32(projectRoot, "seam_it028_tick_marker"),
                    activity.acceptanceRuntimeI32(projectRoot, "seam_it028_render_marker"),
                    activity.acceptanceRuntimeI32(projectRoot, "seam_it028_state_counter")};
            for (int value : originalGuest) {
                if (value == Integer.MIN_VALUE) {
                    throw new IllegalStateException("pre-soak guest state was unavailable");
                }
            }
            requireStateWrite(activity.acceptanceSetRuntimeI32(projectRoot,
                    "seam_it028_state_counter", 0), "reset counter");
            requireStateWrite(activity.acceptanceSetRuntimeI32(projectRoot,
                    "seam_it028_enabled", 1), "enable");
            enabled = true;
            activity.setIt032Active(true);
            soakActive = true;
            activity.resetIt029ResourceMetrics();

            JSONObject firstBuffers = null;
            JSONObject finalPresentation = null;
            int previousToken = -1;
            long previousGeneration = -1;
            int previousRevision = 1;
            long[] revisionTraces = new long[5];
            String[] revisionSources = new String[5];
            String packagedFingerprint = null;
            long finalTrace = 0L;
            JSONArray milestones = new JSONArray();
            JSONObject peaks = new JSONObject().put("lines", 0).put("rects", 0)
                    .put("sprites", 0).put("text", 0).put("text_bytes", 0)
                    .put("order", 0).put("clips", 0).put("sprite_runs", 0)
                    .put("atlas_pages", 0).put("live_regions", 0)
                    .put("text_textures", 0).put("font_entries", 0);

            for (int frame = 1; frame <= FRAME_COUNT; frame += 1) {
                int revision = revisionAt(frame);
                if (isMarker(frame, EDIT_FRAMES)) {
                    if (frame == FRAME_COUNT) {
                        requireStateWrite(activity.acceptanceSetRuntimeI32(projectRoot,
                                "seam_it028_enabled", 0), "disable");
                        enabled = false;
                        restoreGuest(activity, projectRoot, originalGuest);
                    }
                    activity.acceptanceReplaceSource(projectRoot,
                            sourceForRevision(original, revision));
                    requireCompile(activity.acceptanceCompile(projectRoot),
                            "publication at frame " + frame);
                }
                if (isMarker(frame, SURFACE_FRAMES)
                        && !activity.recreateIt032Surface()) {
                    throw new IllegalStateException("surface recreation timed out at frame " + frame);
                }

                JSONObject record = activity.runIt032Frame(projectRoot, frame);
                completed = frame;
                JSONObject runtime = record.getJSONObject("runtime");
                JSONObject guest = record.getJSONObject("guest");
                JSONObject buffers = record.getJSONObject("buffers");
                JSONObject resources = record.getJSONObject("resources");
                JSONObject presentation = record.getJSONObject("presentation");
                int token = record.getInt("frame_token");
                long generation = runtime.getLong("generation");
                String sourceFingerprint = runtime.getString("source_fingerprint");
                if (frame == 1) packagedFingerprint = runtime.getString("source_fingerprint");
                if (token <= previousToken) throw new IllegalStateException("frame tokens not ordered");
                if (presentation.getInt("presented_count") != frame
                        || presentation.getInt("last_frame_token") != token
                        || !presentation.optBoolean("tokens_ordered_unique")) {
                    throw new IllegalStateException("GLES presentation sequence mismatch at frame "
                            + frame);
                }
                finalPresentation = presentation;
                if (runtime.optBoolean("pending_candidate", true)) {
                    throw new IllegalStateException("frame used a pending runtime candidate");
                }
                if (firstBuffers == null) firstBuffers = buffers;
                requireStableBuffers(firstBuffers, buffers);
                requireNoDrops(buffers);
                requireResources(resources);
                if (frame < FRAME_COUNT) {
                    if (guest.getInt("tick_revision") != revision
                            || guest.getInt("render_revision") != revision
                            || guest.getInt("state_counter") != frame) {
                        throw new IllegalStateException("mixed guest generation at frame " + frame);
                    }
                    long trace = record.getLong("command_trace");
                    if (revisionTraces[revision] == 0L) revisionTraces[revision] = trace;
                    if (revisionSources[revision] == null) {
                        revisionSources[revision] = sourceFingerprint;
                    }
                    if (trace != revisionTraces[revision]) {
                        throw new IllegalStateException("trace changed within revision " + revision);
                    }
                    if (!sourceFingerprint.equals(revisionSources[revision])) {
                        throw new IllegalStateException("source changed within revision " + revision);
                    }
                } else {
                    finalTrace = record.getLong("command_trace");
                    if (!packagedFingerprint.equals(runtime.getString("source_fingerprint"))
                            || guest.getInt("tick_revision") != originalGuest[0]
                            || guest.getInt("render_revision") != originalGuest[1]
                            || guest.getInt("state_counter") != originalGuest[2]) {
                        throw new IllegalStateException("final packaged state was not restored");
                    }
                }
                if (previousGeneration >= 0
                        && ((revision == previousRevision && generation != previousGeneration)
                        || (revision != previousRevision
                                && generation != previousGeneration + 1))) {
                    throw new IllegalStateException("runtime generation sequence mismatch at frame " + frame);
                }
                updatePeaks(peaks, buffers, resources);
                if (frame == 1 || isMarker(frame, EDIT_FRAMES)
                        || isMarker(frame, SURFACE_FRAMES)) {
                    JSONObject milestone = new JSONObject().put("frame", frame)
                            .put("revision", revision).put("frame_token", token)
                            .put("generation", generation)
                            .put("source_fingerprint", runtime.getString("source_fingerprint"))
                            .put("command_trace", record.getLong("command_trace"))
                            .put("surface_generation",
                                    resources.getInt("lifecycle_surface_generation"))
                            .put("renderer_generation",
                                    resources.getInt("lifecycle_renderer_generation"))
                            .put("resource_surface_generation",
                                    resources.getInt("surface_generation"))
                            .put("resource_renderer_generation",
                                    resources.getInt("renderer_generation"));
                    milestones.put(milestone);
                    Log.i(LOG_TAG, "Stasis Workshop IT-032 milestone: " + milestone);
                }
                previousToken = token;
                previousGeneration = generation;
                previousRevision = revision;
            }
            for (int left = 1; left <= 4; left += 1) {
                if (revisionTraces[left] == 0L || revisionTraces[left] == finalTrace
                        || revisionSources[left] == null) {
                    throw new IllegalStateException("scripted revision trace was missing");
                }
                for (int right = left + 1; right <= 4; right += 1) {
                    if (revisionTraces[left] == revisionTraces[right]
                            || revisionSources[left].equals(revisionSources[right])) {
                        throw new IllegalStateException("source revisions shared identity or trace");
                    }
                }
            }

            JSONArray traceIdentities = new JSONArray();
            JSONArray sourceIdentities = new JSONArray();
            for (int revision = 1; revision <= 4; revision += 1) {
                traceIdentities.put(revisionTraces[revision]);
                sourceIdentities.put(revisionSources[revision]);
            }

            activity.setIt032Active(false);
            soakActive = false;
            JSONObject cleanup = cleanupReceipt(activity, projectRoot, original, enabled,
                    originalGuest);
            requireCleanup(cleanup);
            JSONObject result = new JSONObject()
                    .put("schema", "stasis.workshop_soak.v1")
                    .put("test_id", "IT-032").put("event", "bounded_soak")
                    .put("status", "passed").put("frame_count", FRAME_COUNT)
                    .put("edit_frames", new JSONArray(EDIT_FRAMES))
                    .put("surface_frames", new JSONArray(SURFACE_FRAMES))
                    .put("milestone_count", milestones.length()).put("milestones", milestones)
                    .put("peaks", peaks).put("cleanup_receipt", cleanup)
                    .put("buffer_contract", new JSONObject()
                            .put("direct", firstBuffers.optBoolean("direct"))
                            .put("stable_identity", true)
                            .put("i32_capacity", firstBuffers.getInt("i32_capacity"))
                            .put("f32_capacity", firstBuffers.getInt("f32_capacity"))
                            .put("u8_capacity", firstBuffers.getInt("u8_capacity"))
                            .put("zero_dropped_frames", FRAME_COUNT))
                    .put("gles_presented_count", finalPresentation.getInt("presented_count"))
                    .put("revision_traces", traceIdentities)
                    .put("revision_sources", sourceIdentities)
                    .put("final_packaged_trace", finalTrace)
                    .put("final_packaged_source", packagedFingerprint)
                    .put("ordered_unique_tokens", true).put("one_generation_per_frame", true)
                    .put("java_only", false).put("fallback", 0).put("stub", 0);
            Log.i(LOG_TAG, "Stasis Workshop IT-032: " + result);
            return result.toString();
        } catch (Exception error) {
            if (soakActive) activity.setIt032Active(false);
            JSONObject cleanup = cleanupReceipt(activity, projectRoot, original, enabled,
                    originalGuest);
            String reason = error.getMessage() == null ? error.getClass().getSimpleName()
                    : error.getMessage();
            try {
                String result = new JSONObject().put("schema", "stasis.workshop_soak.v1")
                        .put("test_id", "IT-032").put("event", "bounded_soak")
                        .put("status", "failed").put("completed_frames", completed)
                        .put("error", reason).put("cleanup_receipt", cleanup).toString();
                Log.e(LOG_TAG, "Stasis Workshop IT-032: " + result);
                return result;
            } catch (Exception ignored) {
                return "{\"test_id\":\"IT-032\",\"status\":\"failed\"}";
            }
        }
    }

    static void requireCleanup(JSONObject receipt) {
        if (!"Restored".equals(receipt.optString("status"))) {
            throw new IllegalStateException("IT-032 cleanup failed: " + receipt);
        }
    }

    private static void requireStableBuffers(JSONObject expected, JSONObject actual)
            throws Exception {
        for (String key : new String[] {"i32_identity", "f32_identity", "u8_identity",
                "i32_capacity", "f32_capacity", "u8_capacity"}) {
            if (expected.getLong(key) != actual.getLong(key)) {
                throw new IllegalStateException("direct buffer changed: " + key);
            }
        }
        if (!actual.optBoolean("direct")) throw new IllegalStateException("buffer was not direct");
    }

    private static void requireNoDrops(JSONObject buffers) throws Exception {
        for (String key : new String[] {"dropped_lines", "dropped_rects", "dropped_sprites",
                "dropped_text", "dropped_order", "dropped_clips", "dropped_sprite_runs"}) {
            if (buffers.getInt(key) != 0) throw new IllegalStateException("commands dropped: " + key);
        }
        if (buffers.getInt("line_count") > 10_000 || buffers.getInt("rect_count") > 10_000
                || buffers.getInt("sprite_count") > 4_096
                || buffers.getInt("text_count") > 2_048
                || buffers.getInt("text_bytes_used") > 65_536
                || buffers.getInt("order_count") > 16_656
                || buffers.getInt("clip_count") > 256
                || buffers.getInt("sprite_run_count") > 4_096) {
            throw new IllegalStateException("declared command capacity exceeded");
        }
    }

    private static void requireResources(JSONObject resources) throws Exception {
        int providerSurface = resources.getInt("surface_generation");
        int lifecycleSurface = resources.getInt("lifecycle_surface_generation");
        int providerRenderer = resources.getInt("renderer_generation");
        int lifecycleRenderer = resources.getInt("lifecycle_renderer_generation");
        boolean valid = resources.optBoolean("resources_ready")
                && providerSurface + 1 == lifecycleSurface
                && providerRenderer == lifecycleRenderer
                && providerSurface >= 1 && providerRenderer >= 1
                && resources.getInt("maximum_atlas_pages") <= 32
                && resources.getInt("maximum_live_regions") <= 4_096
                && resources.getInt("maximum_text_textures") <= 2_048
                && resources.getInt("maximum_font_entries") <= 2_048;
        if (!valid) {
            throw new IllegalStateException("resource lifecycle or peak was invalid: ready="
                    + resources.optBoolean("resources_ready") + " provider_surface="
                    + providerSurface + " lifecycle_surface=" + lifecycleSurface
                    + " provider_renderer=" + providerRenderer + " lifecycle_renderer="
                    + lifecycleRenderer + " atlas_pages="
                    + resources.getInt("maximum_atlas_pages") + " live_regions="
                    + resources.getInt("maximum_live_regions") + " text_textures="
                    + resources.getInt("maximum_text_textures") + " font_entries="
                    + resources.getInt("maximum_font_entries"));
        }
    }

    private static void updatePeaks(JSONObject peaks, JSONObject buffers, JSONObject resources)
            throws Exception {
        String[][] fields = {{"lines", "line_count"}, {"rects", "rect_count"},
                {"sprites", "sprite_count"}, {"text", "text_count"},
                {"text_bytes", "text_bytes_used"}, {"order", "order_count"},
                {"clips", "clip_count"}, {"sprite_runs", "sprite_run_count"},
                {"atlas_pages", "maximum_atlas_pages"},
                {"live_regions", "maximum_live_regions"},
                {"text_textures", "maximum_text_textures"},
                {"font_entries", "maximum_font_entries"}};
        for (String[] field : fields) {
            JSONObject source = field[1].startsWith("maximum_") ? resources : buffers;
            peaks.put(field[0], Math.max(peaks.getInt(field[0]), source.getInt(field[1])));
        }
    }

    private static JSONObject cleanupReceipt(MainActivity activity, String projectRoot,
            String original, boolean enabled, int[] originalGuest) {
        JSONObject receipt = new JSONObject();
        try {
            if (enabled) requireStateWrite(activity.acceptanceSetRuntimeI32(projectRoot,
                    "seam_it028_enabled", 0), "cleanup disable");
            if (originalGuest != null) restoreGuest(activity, projectRoot, originalGuest);
            if (original == null) throw new IllegalStateException("original source unavailable");
            if (!original.equals(activity.acceptanceReadSource(projectRoot))) {
                activity.acceptanceReplaceSource(projectRoot, original);
                requireCompile(activity.acceptanceCompile(projectRoot), "cleanup source");
            }
            JSONObject runtime = activity.acceptanceRuntimeState(projectRoot);
            if (runtime.optBoolean("pending_candidate", false)) {
                String activation = activity.runIt031Frame(projectRoot);
                if (!"passed".equals(activation)) {
                    throw new IllegalStateException("cleanup activation failed: " + activation);
                }
                receipt.put("activation", "native_frame");
                runtime = activity.acceptanceRuntimeState(projectRoot);
            }
            if (runtime.optBoolean("pending_candidate", false)) {
                throw new IllegalStateException("cleanup left pending runtime candidate");
            }
            return receipt.put("status", "Restored").put("source", "packaged")
                    .put("runtime_generation", runtime.getLong("generation"))
                    .put("pending_candidate", false).put("guest_state", "restored");
        } catch (Exception cleanupError) {
            try { return receipt.put("status", "failed").put("error", cleanupError.toString()); }
            catch (Exception ignored) { return receipt; }
        }
    }

    private static void restoreGuest(MainActivity activity, String projectRoot, int[] values) {
        String[] paths = {"seam_it028_tick_marker", "seam_it028_render_marker",
                "seam_it028_state_counter"};
        for (int index = 0; index < paths.length; index += 1) {
            requireStateWrite(activity.acceptanceSetRuntimeI32(projectRoot,
                    paths[index], values[index]), "restore " + paths[index]);
        }
    }

    private static void requireCompile(String value, String phase) {
        if (value == null || !value.startsWith("CompileReady") || !value.contains("status=0")) {
            throw new IllegalStateException(phase + " compile failed: " + value);
        }
    }

    private static void requireStateWrite(String value, String phase) {
        if (value == null || !value.startsWith("StateSet:")) {
            throw new IllegalStateException(phase + " state write failed: " + value);
        }
    }
}
