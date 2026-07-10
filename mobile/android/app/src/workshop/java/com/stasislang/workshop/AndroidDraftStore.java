package com.stasislang.workshop;

import android.content.Context;

import org.json.JSONObject;

import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.nio.file.Files;
import java.nio.file.StandardCopyOption;

final class AndroidDraftStore {
    private static final String ROOT = "workshop_drafts";
    private static final int MAX_DRAFT_BYTES = 2 * 1024 * 1024;

    private AndroidDraftStore() {}

    static void save(Context context, String projectId, String path, String kind, String name,
            String owner, String baseSource, String draftSource) throws Exception {
        byte[] draftBytes = draftSource.getBytes(StandardCharsets.UTF_8);
        if (draftBytes.length > MAX_DRAFT_BYTES) throw new IllegalArgumentException("draft exceeds size limit");
        File file = draftFile(context, projectId);
        File parent = file.getParentFile();
        if (!parent.isDirectory() && !parent.mkdirs()) throw new IllegalStateException("draft directory create failed");
        JSONObject json = new JSONObject()
                .put("format_version", 1)
                .put("timestamp_ms", System.currentTimeMillis())
                .put("path", path)
                .put("kind", kind)
                .put("name", name)
                .put("owner", owner)
                .put("base_sha256", sha256(baseSource))
                .put("draft_source", draftSource);
        writeSyncedAtomic(file, json.toString());
    }

    static Entry load(Context context, String projectId) throws Exception {
        File file = draftFile(context, projectId);
        if (!file.isFile()) return null;
        if (file.length() > MAX_DRAFT_BYTES * 2L + 64L * 1024L) {
            throw new IllegalArgumentException("draft record exceeds size limit");
        }
        JSONObject json = new JSONObject(readText(file));
        if (json.optInt("format_version", 0) != 1) throw new IllegalArgumentException("draft format is unsupported");
        return new Entry(json.getString("path"), json.getString("kind"), json.getString("name"),
                json.optString("owner", ""), json.getString("base_sha256"), json.getString("draft_source"));
    }

    static void clear(Context context, String projectId) throws Exception {
        File file = draftFile(context, projectId);
        if (!file.delete() && file.exists()) throw new IllegalStateException("draft delete failed");
    }

    static void clearIfMatches(Context context, String projectId, String path, String kind,
            String name, String owner) throws Exception {
        Entry entry = load(context, projectId);
        if (entry != null && entry.path.equals(path) && entry.kind.equals(kind)
                && entry.name.equals(name) && entry.owner.equals(owner)) {
            clear(context, projectId);
        }
    }

    static boolean matchesBase(Entry entry, String source) throws Exception {
        return entry.baseSha256.equals(sha256(source));
    }

    private static File draftFile(Context context, String projectId) throws Exception {
        if (projectId == null || !projectId.matches("[A-Za-z0-9][A-Za-z0-9-]{0,79}")) {
            throw new IllegalArgumentException("draft project id is invalid");
        }
        File root = new File(context.getFilesDir(), ROOT);
        File file = new File(root, projectId + ".json");
        if (!file.getCanonicalPath().startsWith(root.getCanonicalPath() + File.separator)) {
            throw new IllegalArgumentException("draft path escaped root");
        }
        return file;
    }

    private static String sha256(String source) throws Exception {
        byte[] digest = MessageDigest.getInstance("SHA-256").digest(source.getBytes(StandardCharsets.UTF_8));
        StringBuilder hex = new StringBuilder(digest.length * 2);
        String digits = "0123456789abcdef";
        for (byte value : digest) {
            int unsigned = value & 0xff;
            hex.append(digits.charAt(unsigned >>> 4)).append(digits.charAt(unsigned & 0x0f));
        }
        return hex.toString();
    }

    private static void writeSyncedAtomic(File file, String source) throws Exception {
        File temporary = new File(file.getParentFile(), file.getName() + ".tmp");
        FileOutputStream output = new FileOutputStream(temporary);
        try {
            output.write(source.getBytes(StandardCharsets.UTF_8));
            output.getFD().sync();
        } finally {
            output.close();
        }
        try {
            Files.move(temporary.toPath(), file.toPath(), StandardCopyOption.ATOMIC_MOVE,
                    StandardCopyOption.REPLACE_EXISTING);
        } catch (Exception error) {
            temporary.delete();
            throw new IllegalStateException("draft publish failed", error);
        }
    }

    private static String readText(File file) throws Exception {
        FileInputStream input = new FileInputStream(file);
        try {
            byte[] bytes = new byte[(int)file.length()];
            int offset = 0;
            while (offset < bytes.length) {
                int read = input.read(bytes, offset, bytes.length - offset);
                if (read < 0) break;
                offset += read;
            }
            return new String(bytes, 0, offset, StandardCharsets.UTF_8);
        } finally {
            input.close();
        }
    }

    static final class Entry {
        final String path;
        final String kind;
        final String name;
        final String owner;
        final String baseSha256;
        final String draftSource;

        Entry(String path, String kind, String name, String owner, String baseSha256, String draftSource) {
            this.path = path;
            this.kind = kind;
            this.name = name;
            this.owner = owner;
            this.baseSha256 = baseSha256;
            this.draftSource = draftSource;
        }
    }
}
