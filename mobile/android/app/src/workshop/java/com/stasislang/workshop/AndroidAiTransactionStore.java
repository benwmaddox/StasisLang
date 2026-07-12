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

final class AndroidAiTransactionStore {
    private static final String ROOT = "workshop_ai_transactions";
    private static final int FORMAT_VERSION = 1;
    private static final int MAX_FILES = 512;
    private static final int MAX_BYTES = 8 * 1024 * 1024;

    private AndroidAiTransactionStore() {}

    static synchronized void save(Context context, String projectId, String itemId,
            WorkshopAiProjectTransaction.Snapshot snapshot) throws Exception {
        requireIdentity(projectId, itemId);
        if (snapshot == null || snapshot.editableFiles.size() > MAX_FILES) {
            throw new IllegalArgumentException("AI transaction snapshot is missing or too large");
        }
        JSONArray files = new JSONArray();
        for (Map.Entry<String, String> entry : snapshot.editableFiles.entrySet()) {
            files.put(new JSONObject().put("path", entry.getKey()).put("source", entry.getValue()));
        }
        byte[] bytes = new JSONObject().put("format_version", FORMAT_VERSION)
                .put("project_id", projectId).put("item_id", itemId).put("files", files)
                .toString().getBytes(StandardCharsets.UTF_8);
        if (bytes.length > MAX_BYTES) throw new IllegalArgumentException("AI transaction exceeds size limit");
        writeAtomic(file(context, projectId, itemId), bytes);
    }

    static synchronized WorkshopAiProjectTransaction.Snapshot load(Context context,
            String projectId, String itemId) throws Exception {
        requireIdentity(projectId, itemId);
        File file = file(context, projectId, itemId);
        if (!file.isFile()) return null;
        if (file.length() > MAX_BYTES) throw new IllegalArgumentException("AI transaction exceeds size limit");
        JSONObject json = new JSONObject(read(file));
        if (json.optInt("format_version", 0) != FORMAT_VERSION
                || !projectId.equals(json.optString("project_id", ""))
                || !itemId.equals(json.optString("item_id", ""))) {
            throw new IllegalArgumentException("AI transaction identity or format is invalid");
        }
        JSONArray files = json.optJSONArray("files");
        if (files == null || files.length() > MAX_FILES) {
            throw new IllegalArgumentException("AI transaction file list is invalid");
        }
        LinkedHashMap<String, String> sources = new LinkedHashMap<>();
        for (int index = 0; index < files.length(); index += 1) {
            JSONObject entry = files.getJSONObject(index);
            String path = entry.getString("path");
            if ((!path.startsWith("src/") && !path.startsWith("tests/"))
                    || !path.endsWith(".stasis") || path.contains("..")
                    || sources.put(path, entry.getString("source")) != null) {
                throw new IllegalArgumentException("AI transaction contains an invalid path");
            }
        }
        return new WorkshopAiProjectTransaction.Snapshot(sources);
    }

    static synchronized void clear(Context context, String projectId, String itemId) throws Exception {
        requireIdentity(projectId, itemId);
        File file = file(context, projectId, itemId);
        File temporary = new File(file.getParentFile(), file.getName() + ".tmp");
        if (!file.delete() && file.exists()) throw new IllegalStateException("AI transaction delete failed");
        if (!temporary.delete() && temporary.exists()) {
            throw new IllegalStateException("AI transaction temporary delete failed");
        }
    }

    private static File file(Context context, String projectId, String itemId) throws Exception {
        File root = new File(context.getFilesDir(), ROOT);
        File file = new File(root, projectId + "-" + itemId + ".json");
        if (!file.getCanonicalPath().startsWith(root.getCanonicalPath() + File.separator)) {
            throw new IllegalArgumentException("AI transaction path escaped root");
        }
        return file;
    }

    private static void writeAtomic(File file, byte[] bytes) throws Exception {
        File parent = file.getParentFile();
        if (!parent.isDirectory() && !parent.mkdirs()) {
            throw new IllegalStateException("AI transaction directory create failed");
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
            if (offset != bytes.length) throw new IllegalStateException("AI transaction read was incomplete");
            return new String(bytes, StandardCharsets.UTF_8);
        } finally {
            input.close();
        }
    }

    private static void requireIdentity(String projectId, String itemId) {
        if (projectId == null || !projectId.matches("[A-Za-z0-9_-]{1,80}")
                || itemId == null || !itemId.matches("[0-9a-f-]{36}")) {
            throw new IllegalArgumentException("AI transaction identity is invalid");
        }
    }
}
