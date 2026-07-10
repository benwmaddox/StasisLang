package com.stasislang.workshop;

import android.content.Context;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.StandardCopyOption;
import java.security.MessageDigest;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.UUID;

final class AndroidAiQueue {
    static final String PENDING = "pending";
    static final String IN_PROGRESS = "in_progress";
    static final String COMPLETED = "completed";
    static final String FAILED = "failed";
    static final String CANCELLED = "cancelled";

    private static final String ROOT = "workshop_ai_queue";
    private static final int FORMAT_VERSION = 1;
    private static final int MAX_ITEMS = 100;
    private static final int MAX_FILE_BYTES = 2 * 1024 * 1024;
    private static final int MAX_PREVIEW_BYTES = 12 * 1024 * 1024;

    private AndroidAiQueue() {}

    static synchronized Entry enqueue(Context context, String projectId, String source, String prompt,
            JSONArray imageAttachments, JSONObject logicalSnapshot, boolean imageGeneration,
            byte[] previewPng, int previewWidth, int previewHeight) throws Exception {
        requireProjectId(projectId);
        if (!AiQueuePolicy.validSource(source)) {
            throw new IllegalArgumentException("AI queue source must be text or voice");
        }
        String cleanPrompt = prompt == null ? "" : prompt.trim();
        if (cleanPrompt.isEmpty() || cleanPrompt.getBytes(StandardCharsets.UTF_8).length > 64 * 1024) {
            throw new IllegalArgumentException("AI queue prompt is empty or too large");
        }
        JSONObject document = loadDocument(context, projectId);
        JSONArray items = document.getJSONArray("items");
        while (items.length() >= MAX_ITEMS && removeOldestTerminal(items)) {
            // Never prune pending or active work; one terminal record makes room for this submission.
        }
        if (items.length() >= MAX_ITEMS) {
            throw new IllegalStateException("AI queue has reached its 100 non-terminal item limit");
        }
        String id = UUID.randomUUID().toString();
        if (previewPng != null && (previewPng.length == 0 || previewPng.length > MAX_PREVIEW_BYTES
                || previewWidth <= 0 || previewHeight <= 0)) {
            throw new IllegalArgumentException("AI queue preview snapshot is invalid or too large");
        }
        JSONArray projectImages = projectImagesOnly(imageAttachments);
        long totalImageBytes = previewPng == null ? 0L : previewPng.length;
        for (int index = 0; index < projectImages.length(); index += 1) {
            totalImageBytes += projectImages.getJSONObject(index).optLong("bytes", 0L);
        }
        if (projectImages.length() + (previewPng == null ? 0 : 1) > 4
                || totalImageBytes > MAX_PREVIEW_BYTES) {
            throw new IllegalArgumentException("AI queue attachments exceed the four-image or 12 MiB limit");
        }
        String previewFile = previewPng == null ? "" : projectId + "-" + id + ".png";
        Entry entry = new Entry(id, projectId, source, cleanPrompt,
                System.currentTimeMillis(), PENDING,
                projectImages,
                logicalSnapshot == null ? null : new JSONObject(logicalSnapshot.toString()), imageGeneration,
                "", previewFile, previewWidth, previewHeight,
                previewPng == null ? 0 : previewPng.length, previewPng == null ? "" : sha256(previewPng));
        File savedPreview = null;
        try {
            if (previewPng != null) {
                savedPreview = previewFile(context, entry);
                writeSyncedAtomic(savedPreview, previewPng);
            }
            items.put(entry.toJson());
            writeDocument(context, projectId, document);
        } catch (Exception error) {
            if (savedPreview != null) savedPreview.delete();
            throw error;
        }
        return entry;
    }

    private static JSONArray projectImagesOnly(JSONArray attachments) throws Exception {
        JSONArray result = new JSONArray();
        if (attachments == null) return result;
        for (int index = 0; index < attachments.length(); index += 1) {
            JSONObject attachment = attachments.optJSONObject(index);
            if (attachment != null && !attachment.optString("project_path", "").isEmpty()) {
                result.put(new JSONObject(attachment.toString()));
            }
        }
        return result;
    }

    private static boolean removeOldestTerminal(JSONArray items) throws Exception {
        for (int index = 0; index < items.length(); index += 1) {
            String state = items.getJSONObject(index).optString("state", "");
            if (AiQueuePolicy.terminal(state)) {
                items.remove(index);
                return true;
            }
        }
        return false;
    }

    static synchronized byte[] loadPreview(Context context, Entry entry) throws Exception {
        if (entry.previewFile.isEmpty()) return null;
        File file = previewFile(context, entry);
        if (!file.isFile() || file.length() != entry.previewBytes || file.length() > MAX_PREVIEW_BYTES) {
            throw new IllegalStateException("queued preview snapshot is missing or changed");
        }
        byte[] bytes = readBytes(file);
        if (!sha256(bytes).equals(entry.previewSha256)) {
            throw new IllegalStateException("queued preview snapshot failed its content hash");
        }
        return bytes;
    }

    static synchronized List<Entry> list(Context context, String projectId) throws Exception {
        JSONArray items = loadDocument(context, projectId).getJSONArray("items");
        ArrayList<Entry> result = new ArrayList<>();
        for (int index = 0; index < items.length(); index += 1) {
            result.add(Entry.fromJson(items.getJSONObject(index), projectId));
        }
        return result;
    }

    static synchronized Entry claimNext(Context context, String projectId) throws Exception {
        JSONObject document = loadDocument(context, projectId);
        JSONArray items = document.getJSONArray("items");
        for (int index = 0; index < items.length(); index += 1) {
            Entry entry = Entry.fromJson(items.getJSONObject(index), projectId);
            if (!PENDING.equals(entry.state)) continue;
            Entry claimed = entry.withState(IN_PROGRESS, "");
            items.put(index, claimed.toJson());
            writeDocument(context, projectId, document);
            return claimed;
        }
        return null;
    }

    static synchronized boolean cancelPending(Context context, String projectId, String itemId) throws Exception {
        return transition(context, projectId, itemId, PENDING, CANCELLED, "Cancelled before execution");
    }

    static synchronized boolean finish(Context context, String projectId, String itemId,
            String terminalState, String detail) throws Exception {
        if (!COMPLETED.equals(terminalState) && !FAILED.equals(terminalState) && !CANCELLED.equals(terminalState)) {
            throw new IllegalArgumentException("AI queue terminal state is invalid");
        }
        return transition(context, projectId, itemId, IN_PROGRESS, terminalState, detail);
    }

    static synchronized int recoverInterrupted(Context context, String projectId) throws Exception {
        JSONObject document = loadDocument(context, projectId);
        JSONArray items = document.getJSONArray("items");
        int recovered = 0;
        for (int index = 0; index < items.length(); index += 1) {
            Entry entry = Entry.fromJson(items.getJSONObject(index), projectId);
            String recoveredState = AiQueuePolicy.recoveredState(entry.state);
            if (entry.state.equals(recoveredState)) continue;
            items.put(index, entry.withState(recoveredState,
                    "Interrupted before the compile/test boundary completed; submit again to retry").toJson());
            recovered += 1;
        }
        if (recovered > 0) {
            writeDocument(context, projectId, document);
            for (int index = 0; index < items.length(); index += 1) {
                Entry entry = Entry.fromJson(items.getJSONObject(index), projectId);
                if (FAILED.equals(entry.state)) deletePreview(context, entry);
            }
        }
        return recovered;
    }

    static synchronized void clearAll(Context context) throws Exception {
        File root = new File(context.getFilesDir(), ROOT);
        File[] files = root.listFiles();
        if (files == null) return;
        for (File file : files) {
            if (!file.isFile() || (!file.getName().endsWith(".json") && !file.getName().endsWith(".tmp")
                    && !file.getName().endsWith(".png"))) {
                throw new IllegalStateException("AI queue directory contains an unexpected entry");
            }
            if (!file.delete() && file.exists()) throw new IllegalStateException("AI queue erase failed");
        }
        if (!root.delete() && root.exists()) throw new IllegalStateException("AI queue directory erase failed");
    }

    static synchronized void clearProject(Context context, String projectId) throws Exception {
        requireProjectId(projectId);
        File file = queueFile(context, projectId);
        File temporary = new File(file.getParentFile(), file.getName() + ".tmp");
        if (!file.delete() && file.exists()) throw new IllegalStateException("project AI queue erase failed");
        if (!temporary.delete() && temporary.exists()) {
            throw new IllegalStateException("project AI queue temporary erase failed");
        }
        File[] files = file.getParentFile().listFiles();
        if (files != null) for (File candidate : files) {
            if (candidate.isFile() && candidate.getName().startsWith(projectId + "-")
                    && (candidate.getName().endsWith(".png") || candidate.getName().endsWith(".png.tmp"))
                    && !candidate.delete() && candidate.exists()) {
                throw new IllegalStateException("project AI preview queue erase failed");
            }
        }
    }

    private static boolean transition(Context context, String projectId, String itemId,
            String expectedState, String nextState, String detail) throws Exception {
        if (!AiQueuePolicy.canTransition(expectedState, nextState)) {
            throw new IllegalArgumentException("AI queue state transition is invalid");
        }
        JSONObject document = loadDocument(context, projectId);
        JSONArray items = document.getJSONArray("items");
        for (int index = 0; index < items.length(); index += 1) {
            Entry entry = Entry.fromJson(items.getJSONObject(index), projectId);
            if (!entry.id.equals(itemId)) continue;
            if (!expectedState.equals(entry.state)) return false;
            items.put(index, entry.withState(nextState, detail).toJson());
            writeDocument(context, projectId, document);
            if (CANCELLED.equals(nextState) || COMPLETED.equals(nextState) || FAILED.equals(nextState)) {
                deletePreview(context, entry);
            }
            return true;
        }
        return false;
    }

    private static JSONObject loadDocument(Context context, String projectId) throws Exception {
        requireProjectId(projectId);
        File file = queueFile(context, projectId);
        if (!file.isFile()) {
            JSONObject empty = emptyDocument(projectId);
            pruneOrphanPreviews(context, projectId, empty.getJSONArray("items"));
            return empty;
        }
        if (file.length() > MAX_FILE_BYTES) throw new IllegalArgumentException("AI queue record exceeds size limit");
        JSONObject document = new JSONObject(readText(file));
        if (document.optInt("format_version", 0) != FORMAT_VERSION
                || !projectId.equals(document.optString("project_id", ""))) {
            throw new IllegalArgumentException("AI queue format or project identity is invalid");
        }
        JSONArray items = document.optJSONArray("items");
        if (items == null || items.length() > MAX_ITEMS) throw new IllegalArgumentException("AI queue item list is invalid");
        pruneOrphanPreviews(context, projectId, items);
        return document;
    }

    private static void pruneOrphanPreviews(Context context, String projectId, JSONArray items) throws Exception {
        HashSet<String> retained = new HashSet<>();
        for (int index = 0; index < items.length(); index += 1) {
            Entry entry = Entry.fromJson(items.getJSONObject(index), projectId);
            if (!entry.previewFile.isEmpty() && (PENDING.equals(entry.state) || IN_PROGRESS.equals(entry.state))) {
                retained.add(entry.previewFile);
            }
        }
        File root = new File(context.getFilesDir(), ROOT);
        File[] files = root.listFiles();
        if (files == null) return;
        for (File candidate : files) {
            String name = candidate.getName();
            if (!candidate.isFile() || !name.startsWith(projectId + "-")
                    || (!name.endsWith(".png") && !name.endsWith(".png.tmp"))) continue;
            if (!retained.contains(name) && !candidate.delete() && candidate.exists()) {
                throw new IllegalStateException("orphaned AI queue preview cleanup failed");
            }
        }
    }

    private static JSONObject emptyDocument(String projectId) throws Exception {
        return new JSONObject().put("format_version", FORMAT_VERSION).put("project_id", projectId)
                .put("items", new JSONArray());
    }

    private static void writeDocument(Context context, String projectId, JSONObject document) throws Exception {
        File file = queueFile(context, projectId);
        File parent = file.getParentFile();
        if (!parent.isDirectory() && !parent.mkdirs()) throw new IllegalStateException("AI queue directory create failed");
        byte[] bytes = document.toString().getBytes(StandardCharsets.UTF_8);
        if (bytes.length > MAX_FILE_BYTES) throw new IllegalStateException("AI queue record exceeds size limit");
        File temporary = new File(parent, file.getName() + ".tmp");
        FileOutputStream output = new FileOutputStream(temporary);
        try {
            output.write(bytes);
            output.getFD().sync();
        } finally {
            output.close();
        }
        try {
            Files.move(temporary.toPath(), file.toPath(), StandardCopyOption.ATOMIC_MOVE,
                    StandardCopyOption.REPLACE_EXISTING);
        } catch (Exception error) {
            temporary.delete();
            throw new IllegalStateException("AI queue publish failed", error);
        }
    }

    private static void writeSyncedAtomic(File file, byte[] bytes) throws Exception {
        File parent = file.getParentFile();
        if (!parent.isDirectory() && !parent.mkdirs()) throw new IllegalStateException("AI queue directory create failed");
        File temporary = new File(parent, file.getName() + ".tmp");
        FileOutputStream output = new FileOutputStream(temporary);
        try {
            output.write(bytes);
            output.getFD().sync();
        } finally {
            output.close();
        }
        try {
            Files.move(temporary.toPath(), file.toPath(), StandardCopyOption.ATOMIC_MOVE,
                    StandardCopyOption.REPLACE_EXISTING);
        } catch (Exception error) {
            temporary.delete();
            throw new IllegalStateException("AI queue media publish failed", error);
        }
    }

    private static File previewFile(Context context, Entry entry) throws Exception {
        String expected = entry.projectId + "-" + entry.id + ".png";
        if (!expected.equals(entry.previewFile) || !entry.id.matches("[0-9a-f-]{36}")) {
            throw new IllegalArgumentException("AI queue preview identity is invalid");
        }
        File root = new File(context.getFilesDir(), ROOT);
        File file = new File(root, entry.previewFile);
        if (!file.getCanonicalPath().startsWith(root.getCanonicalPath() + File.separator)) {
            throw new IllegalArgumentException("AI queue preview path escaped root");
        }
        return file;
    }

    private static void deletePreview(Context context, Entry entry) throws Exception {
        if (entry.previewFile.isEmpty()) return;
        File file = previewFile(context, entry);
        if (!file.delete() && file.exists()) throw new IllegalStateException("AI queue preview deletion failed");
    }

    private static byte[] readBytes(File file) throws Exception {
        FileInputStream input = new FileInputStream(file);
        try {
            byte[] bytes = new byte[(int)file.length()];
            int offset = 0;
            while (offset < bytes.length) {
                int count = input.read(bytes, offset, bytes.length - offset);
                if (count < 0) break;
                offset += count;
            }
            if (offset != bytes.length) throw new IllegalStateException("AI queue preview read was incomplete");
            return bytes;
        } finally {
            input.close();
        }
    }

    private static String sha256(byte[] bytes) throws Exception {
        byte[] digest = MessageDigest.getInstance("SHA-256").digest(bytes);
        StringBuilder hex = new StringBuilder(digest.length * 2);
        String digits = "0123456789abcdef";
        for (byte value : digest) {
            int unsigned = value & 0xff;
            hex.append(digits.charAt(unsigned >>> 4)).append(digits.charAt(unsigned & 0x0f));
        }
        return hex.toString();
    }

    private static File queueFile(Context context, String projectId) throws Exception {
        File root = new File(context.getFilesDir(), ROOT);
        File file = new File(root, projectId + ".json");
        if (!file.getCanonicalPath().startsWith(root.getCanonicalPath() + File.separator)) {
            throw new IllegalArgumentException("AI queue path escaped root");
        }
        return file;
    }

    private static void requireProjectId(String projectId) {
        if (!AiQueuePolicy.validProjectId(projectId)) {
            throw new IllegalArgumentException("AI queue project id is invalid");
        }
    }

    private static String readText(File file) throws Exception {
        FileInputStream input = new FileInputStream(file);
        try {
            byte[] bytes = new byte[(int)file.length()];
            int offset = 0;
            while (offset < bytes.length) {
                int count = input.read(bytes, offset, bytes.length - offset);
                if (count < 0) break;
                offset += count;
            }
            return new String(bytes, 0, offset, StandardCharsets.UTF_8);
        } finally {
            input.close();
        }
    }

    static final class Entry {
        final String id;
        final String projectId;
        final String source;
        final String prompt;
        final long createdAtMs;
        final String state;
        final JSONArray imageAttachments;
        final JSONObject logicalSnapshot;
        final boolean imageGeneration;
        final String detail;
        final String previewFile;
        final int previewWidth;
        final int previewHeight;
        final int previewBytes;
        final String previewSha256;

        Entry(String id, String projectId, String source, String prompt, long createdAtMs, String state,
                JSONArray imageAttachments, JSONObject logicalSnapshot, boolean imageGeneration, String detail,
                String previewFile, int previewWidth, int previewHeight, int previewBytes, String previewSha256) {
            this.id = id;
            this.projectId = projectId;
            this.source = source;
            this.prompt = prompt;
            this.createdAtMs = createdAtMs;
            this.state = state;
            this.imageAttachments = imageAttachments;
            this.logicalSnapshot = logicalSnapshot;
            this.imageGeneration = imageGeneration;
            this.detail = detail;
            this.previewFile = previewFile;
            this.previewWidth = previewWidth;
            this.previewHeight = previewHeight;
            this.previewBytes = previewBytes;
            this.previewSha256 = previewSha256;
        }

        Entry withState(String nextState, String nextDetail) {
            return new Entry(id, projectId, source, prompt, createdAtMs, nextState, imageAttachments,
                    logicalSnapshot, imageGeneration, nextDetail == null ? "" : nextDetail,
                    previewFile, previewWidth, previewHeight, previewBytes, previewSha256);
        }

        JSONObject toJson() throws Exception {
            JSONObject json = new JSONObject().put("id", id).put("project_id", projectId)
                    .put("source", source).put("prompt", prompt).put("created_at_ms", createdAtMs)
                    .put("state", state).put("image_attachments", new JSONArray(imageAttachments.toString()))
                    .put("image_generation", imageGeneration).put("detail", detail)
                    .put("preview_file", previewFile).put("preview_width", previewWidth)
                    .put("preview_height", previewHeight).put("preview_bytes", previewBytes)
                    .put("preview_sha256", previewSha256);
            if (logicalSnapshot != null) json.put("logical_snapshot", new JSONObject(logicalSnapshot.toString()));
            return json;
        }

        static Entry fromJson(JSONObject json, String expectedProjectId) throws Exception {
            String state = json.getString("state");
            if (!AiQueuePolicy.validState(state)) {
                throw new IllegalArgumentException("AI queue item state is invalid");
            }
            String projectId = json.getString("project_id");
            if (!expectedProjectId.equals(projectId)) throw new IllegalArgumentException("AI queue item crossed projects");
            String id = json.getString("id");
            String source = json.getString("source");
            String prompt = json.getString("prompt");
            if (!id.matches("[0-9a-f-]{36}") || !AiQueuePolicy.validSource(source)
                    || prompt.trim().isEmpty() || prompt.getBytes(StandardCharsets.UTF_8).length > 64 * 1024) {
                throw new IllegalArgumentException("AI queue item identity or request is invalid");
            }
            String previewFile = json.optString("preview_file", "");
            int previewWidth = json.optInt("preview_width", 0);
            int previewHeight = json.optInt("preview_height", 0);
            int previewBytes = json.optInt("preview_bytes", 0);
            String previewSha256 = json.optString("preview_sha256", "");
            if (!previewFile.isEmpty() && (!previewFile.equals(projectId + "-" + id + ".png")
                    || previewWidth <= 0 || previewHeight <= 0 || previewBytes <= 0
                    || previewBytes > MAX_PREVIEW_BYTES || !previewSha256.matches("[0-9a-f]{64}"))) {
                throw new IllegalArgumentException("AI queue preview metadata is invalid");
            }
            return new Entry(id, projectId, source, prompt,
                    json.getLong("created_at_ms"), state, json.optJSONArray("image_attachments") == null
                            ? new JSONArray() : new JSONArray(json.getJSONArray("image_attachments").toString()),
                    json.optJSONObject("logical_snapshot") == null ? null
                            : new JSONObject(json.getJSONObject("logical_snapshot").toString()),
                    json.optBoolean("image_generation", false), json.optString("detail", ""),
                    previewFile, previewWidth, previewHeight, previewBytes, previewSha256);
        }
    }
}
