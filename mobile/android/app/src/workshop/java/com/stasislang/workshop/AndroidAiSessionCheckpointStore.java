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
import java.util.LinkedHashMap;
import java.util.Map;

final class AndroidAiSessionCheckpointStore {
    private static final String ROOT = "workshop_ai_sessions";
    private static final int FORMAT_VERSION = 1;
    private static final int MAX_FILES = 512;
    private static final int MAX_BYTES = 8 * 1024 * 1024;

    private AndroidAiSessionCheckpointStore() {}

    static synchronized void save(Context context, Checkpoint checkpoint) throws Exception {
        save(context.getFilesDir(), checkpoint);
    }

    static synchronized void save(File filesDir, Checkpoint checkpoint) throws Exception {
        checkpoint.validate();
        JSONArray files = new JSONArray();
        for (Map.Entry<String, String> entry : checkpoint.projectSnapshot.editableFiles.entrySet()) {
            files.put(new JSONObject().put("path", entry.getKey()).put("source", entry.getValue()));
        }
        byte[] bytes = new JSONObject().put("format_version", FORMAT_VERSION)
                .put("project_id", checkpoint.projectId).put("item_id", checkpoint.itemId)
                .put("stage", checkpoint.stage).put("provider", checkpoint.provider)
                .put("model", checkpoint.model).put("attachment_fingerprint", checkpoint.attachmentFingerprint)
                .put("project_fingerprint", WorkshopAiProjectTransaction.fingerprint(checkpoint.projectSnapshot))
                .put("payload", new JSONObject(checkpoint.payload.toString())).put("files", files)
                .toString().getBytes(StandardCharsets.UTF_8);
        if (bytes.length > MAX_BYTES) throw new IllegalArgumentException("AI session checkpoint exceeds size limit");
        writeAtomic(file(filesDir, checkpoint.projectId, checkpoint.itemId), bytes);
    }

    static synchronized Checkpoint load(Context context, String projectId, String itemId) throws Exception {
        return load(context.getFilesDir(), projectId, itemId);
    }

    static synchronized Checkpoint load(File filesDir, String projectId, String itemId) throws Exception {
        requireIdentity(projectId, itemId);
        File file = file(filesDir, projectId, itemId);
        if (!file.isFile()) return null;
        if (file.length() > MAX_BYTES) throw new IllegalArgumentException("AI session checkpoint exceeds size limit");
        JSONObject json = new JSONObject(read(file));
        if (json.optInt("format_version", 0) != FORMAT_VERSION
                || !projectId.equals(json.optString("project_id", ""))
                || !itemId.equals(json.optString("item_id", ""))) {
            throw new IllegalArgumentException("AI session checkpoint identity or format is invalid");
        }
        JSONArray files = json.optJSONArray("files");
        if (files == null || files.length() > MAX_FILES) {
            throw new IllegalArgumentException("AI session checkpoint file list is invalid");
        }
        LinkedHashMap<String, String> sources = new LinkedHashMap<>();
        for (int index = 0; index < files.length(); index += 1) {
            JSONObject entry = files.getJSONObject(index);
            String path = entry.getString("path");
            if ((!path.startsWith("src/") && !path.startsWith("tests/"))
                    || !path.endsWith(".stasis") || path.contains("..")
                    || sources.put(path, entry.getString("source")) != null) {
                throw new IllegalArgumentException("AI session checkpoint contains an invalid path");
            }
        }
        Checkpoint checkpoint = new Checkpoint(projectId, itemId, json.getString("stage"),
                json.getString("provider"), json.getString("model"),
                json.getString("attachment_fingerprint"), json.getJSONObject("payload"),
                new WorkshopAiProjectTransaction.Snapshot(sources));
        checkpoint.validate();
        if (!json.getString("project_fingerprint").equals(
                WorkshopAiProjectTransaction.fingerprint(checkpoint.projectSnapshot))) {
            throw new IllegalArgumentException("AI session checkpoint project hash is invalid");
        }
        return checkpoint;
    }

    static synchronized void clear(Context context, String projectId, String itemId) throws Exception {
        clear(context.getFilesDir(), projectId, itemId);
    }

    static synchronized void clear(File filesDir, String projectId, String itemId) throws Exception {
        requireIdentity(projectId, itemId);
        File file = file(filesDir, projectId, itemId);
        File temporary = new File(file.getParentFile(), file.getName() + ".tmp");
        if (!file.delete() && file.exists()) throw new IllegalStateException("AI session checkpoint delete failed");
        if (!temporary.delete() && temporary.exists()) {
            throw new IllegalStateException("AI session checkpoint temporary delete failed");
        }
    }

    static synchronized void clearProject(Context context, String projectId) throws Exception {
        clearProject(context.getFilesDir(), projectId);
    }

    static synchronized void clearProject(File filesDir, String projectId) throws Exception {
        if (!AiQueuePolicy.validProjectId(projectId)) {
            throw new IllegalArgumentException("AI session checkpoint project identity is invalid");
        }
        File root = new File(filesDir, ROOT);
        File[] files = root.listFiles();
        if (files == null) return;
        for (File candidate : files) {
            if (candidate.isFile() && belongsToProject(candidate.getName(), projectId)
                    && !candidate.delete() && candidate.exists()) {
                throw new IllegalStateException("project AI session checkpoint erase failed");
            }
        }
    }

    private static boolean belongsToProject(String name, String projectId) {
        String prefix = projectId + "-";
        String suffix = name.endsWith(".json.tmp") ? ".json.tmp"
                : (name.endsWith(".json") ? ".json" : "");
        if (suffix.isEmpty() || name.length() != prefix.length() + 36 + suffix.length()) return false;
        return name.startsWith(prefix)
                && name.substring(prefix.length(), prefix.length() + 36).matches("[0-9a-f-]{36}");
    }

    static synchronized void clearAll(Context context) throws Exception {
        clearAll(context.getFilesDir());
    }

    static synchronized void clearAll(File filesDir) throws Exception {
        File root = new File(filesDir, ROOT);
        File[] files = root.listFiles();
        if (files == null) return;
        for (File file : files) {
            if (!file.isFile() || (!file.getName().endsWith(".json")
                    && !file.getName().endsWith(".tmp"))) {
                throw new IllegalStateException("AI session checkpoint directory contains an unexpected entry");
            }
            if (!file.delete() && file.exists()) {
                throw new IllegalStateException("AI session checkpoint erase failed");
            }
        }
        if (!root.delete() && root.exists()) {
            throw new IllegalStateException("AI session checkpoint directory erase failed");
        }
    }

    static final class Checkpoint {
        final String projectId;
        final String itemId;
        final String stage;
        final String provider;
        final String model;
        final String attachmentFingerprint;
        final JSONObject payload;
        final WorkshopAiProjectTransaction.Snapshot projectSnapshot;

        Checkpoint(String projectId, String itemId, String stage, String provider, String model,
                String attachmentFingerprint, JSONObject payload,
                WorkshopAiProjectTransaction.Snapshot projectSnapshot) {
            this.projectId = projectId;
            this.itemId = itemId;
            this.stage = stage;
            this.provider = provider;
            this.model = model;
            this.attachmentFingerprint = attachmentFingerprint;
            this.payload = payload;
            this.projectSnapshot = projectSnapshot;
        }

        void validate() {
            requireIdentity(projectId, itemId);
            if (!WorkshopAiResumePolicy.validStage(stage) || provider.isEmpty() || model.isEmpty()
                    || !attachmentFingerprint.matches("[0-9a-f]{64}") || payload == null
                    || projectSnapshot == null || projectSnapshot.editableFiles.size() > MAX_FILES) {
                throw new IllegalArgumentException("AI session checkpoint is invalid");
            }
        }
    }

    private static File file(File filesDir, String projectId, String itemId) throws Exception {
        File root = new File(filesDir, ROOT);
        File file = new File(root, projectId + "-" + itemId + ".json");
        if (!file.getCanonicalPath().startsWith(root.getCanonicalPath() + File.separator)) {
            throw new IllegalArgumentException("AI session checkpoint path escaped root");
        }
        return file;
    }

    private static void writeAtomic(File file, byte[] bytes) throws Exception {
        File parent = file.getParentFile();
        if (!parent.isDirectory() && !parent.mkdirs()) {
            throw new IllegalStateException("AI session checkpoint directory create failed");
        }
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
            throw error;
        }
    }

    private static String read(File file) throws Exception {
        FileInputStream input = new FileInputStream(file);
        try {
            byte[] bytes = new byte[(int)file.length()];
            int offset = 0;
            while (offset < bytes.length) {
                int count = input.read(bytes, offset, bytes.length - offset);
                if (count < 0) break;
                offset += count;
            }
            if (offset != bytes.length) throw new IllegalStateException("AI session checkpoint read was incomplete");
            return new String(bytes, StandardCharsets.UTF_8);
        } finally {
            input.close();
        }
    }

    private static void requireIdentity(String projectId, String itemId) {
        if (!AiQueuePolicy.validProjectId(projectId)
                || itemId == null || !itemId.matches("[0-9a-f-]{36}")) {
            throw new IllegalArgumentException("AI session checkpoint identity is invalid");
        }
    }
}
