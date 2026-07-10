package com.stasislang.workshop;

import android.content.Context;

import org.json.JSONObject;

import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.Comparator;

final class AndroidEditRecoveryStore {
    private static final String ROOT = "workshop_edit_recovery";
    private static final int MAX_ENTRIES = 10;
    private static final int MAX_SOURCE_BYTES = 2 * 1024 * 1024;

    private AndroidEditRecoveryStore() {}

    static Entry record(Context context, String projectId, String path, String symbol,
            String beforeSource, String failedSource, String diagnostic) throws Exception {
        File directory = projectDirectory(context, projectId);
        if (!directory.isDirectory() && !directory.mkdirs()) throw new IllegalStateException("recovery directory create failed");
        long timestamp = System.currentTimeMillis();
        byte[] beforeBytes = beforeSource.getBytes(StandardCharsets.UTF_8);
        byte[] failedBytes = failedSource.getBytes(StandardCharsets.UTF_8);
        if (beforeBytes.length > MAX_SOURCE_BYTES || failedBytes.length > MAX_SOURCE_BYTES) {
            throw new IllegalArgumentException("recovery source exceeds size limit");
        }
        File target = new File(directory, timestamp + ".json");
        for (int suffix = 2; target.exists(); suffix += 1) target = new File(directory, timestamp + "-" + suffix + ".json");
        JSONObject json = new JSONObject()
                .put("timestamp_ms", timestamp)
                .put("path", path)
                .put("symbol", symbol)
                .put("before_source", beforeSource)
                .put("failed_source", failedSource)
                .put("diagnostic", diagnostic);
        writeSyncedAtomic(target, json.toString());
        trim(directory);
        return new Entry(target, timestamp, path, symbol, beforeSource, failedSource, diagnostic);
    }

    static Entry latest(Context context, String projectId) throws Exception {
        Entry[] entries = list(context, projectId);
        return entries.length == 0 ? null : entries[0];
    }

    static Entry[] list(Context context, String projectId) throws Exception {
        File[] files = recoveryFiles(projectDirectory(context, projectId));
        Entry[] entries = new Entry[files.length];
        for (int index = 0; index < files.length; index += 1) entries[index] = read(files[index]);
        return entries;
    }

    static void consume(Entry entry) throws Exception {
        if (!entry.file.delete() && entry.file.exists()) throw new IllegalStateException("recovery entry delete failed");
    }

    static void clearProject(Context context, String projectId) throws Exception {
        File directory = projectDirectory(context, projectId);
        File[] files = directory.listFiles();
        if (files != null) {
            for (File file : files) {
                if (!file.delete() && file.exists()) throw new IllegalStateException("recovery entry delete failed");
            }
        }
        if (!directory.delete() && directory.exists()) throw new IllegalStateException("recovery directory delete failed");
    }

    private static Entry read(File file) throws Exception {
        JSONObject json = new JSONObject(readText(file));
        return new Entry(file, json.getLong("timestamp_ms"), json.getString("path"),
                json.optString("symbol", ""), json.getString("before_source"),
                json.getString("failed_source"), json.optString("diagnostic", ""));
    }

    private static File projectDirectory(Context context, String projectId) throws Exception {
        if (projectId == null || !projectId.matches("[A-Za-z0-9][A-Za-z0-9-]{0,79}")) {
            throw new IllegalArgumentException("recovery project id is invalid");
        }
        File root = new File(context.getFilesDir(), ROOT);
        File directory = new File(root, projectId);
        String rootPath = root.getCanonicalPath();
        if (!directory.getCanonicalPath().startsWith(rootPath + File.separator)) {
            throw new IllegalArgumentException("recovery path escaped root");
        }
        return directory;
    }

    private static File[] recoveryFiles(File directory) {
        File[] files = directory.listFiles((parent, name) -> name.endsWith(".json"));
        if (files == null) return new File[0];
        Arrays.sort(files, new Comparator<File>() {
            @Override public int compare(File left, File right) {
                int modified = Long.compare(right.lastModified(), left.lastModified());
                return modified != 0 ? modified : right.getName().compareTo(left.getName());
            }
        });
        return files;
    }

    private static void trim(File directory) {
        File[] files = recoveryFiles(directory);
        for (int index = MAX_ENTRIES; index < files.length; index += 1) files[index].delete();
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
        if (!temporary.renameTo(file)) {
            temporary.delete();
            throw new IllegalStateException("recovery entry publish failed");
        }
    }

    private static String readText(File file) throws Exception {
        if (file.length() > MAX_SOURCE_BYTES * 4L + 64L * 1024L) {
            throw new IllegalArgumentException("recovery entry exceeds size limit");
        }
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
        final File file;
        final long timestampMs;
        final String path;
        final String symbol;
        final String beforeSource;
        final String failedSource;
        final String diagnostic;

        Entry(File file, long timestampMs, String path, String symbol, String beforeSource,
                String failedSource, String diagnostic) {
            this.file = file;
            this.timestampMs = timestampMs;
            this.path = path;
            this.symbol = symbol;
            this.beforeSource = beforeSource;
            this.failedSource = failedSource;
            this.diagnostic = diagnostic;
        }
    }
}
