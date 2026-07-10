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

final class AndroidCrashStore {
    private static final String FILE_NAME = "android_crash_redacted.json";
    private static final int MAX_FRAMES = 30;
    private static final int MAX_RECORD_BYTES = 64 * 1024;
    private static boolean installed;

    private AndroidCrashStore() {}

    static synchronized void install(final Context context) {
        if (installed) return;
        installed = true;
        final Context appContext = context.getApplicationContext();
        final Thread.UncaughtExceptionHandler prior = Thread.getDefaultUncaughtExceptionHandler();
        Thread.setDefaultUncaughtExceptionHandler(new Thread.UncaughtExceptionHandler() {
            @Override public void uncaughtException(Thread thread, Throwable error) {
                try {
                    record(appContext, thread, error);
                } catch (Exception ignored) {
                    // Crash recording cannot replace the platform crash path.
                }
                if (prior != null) {
                    prior.uncaughtException(thread, error);
                } else {
                    android.os.Process.killProcess(android.os.Process.myPid());
                }
            }
        });
    }

    static JSONObject safeSummary(Context context) {
        try {
            File file = recordFile(context);
            if (!file.isFile() || file.length() > MAX_RECORD_BYTES) return new JSONObject().put("present", false);
            JSONObject stored = new JSONObject(readText(file));
            JSONArray frames = stored.optJSONArray("frames");
            JSONArray safeFrames = new JSONArray();
            if (frames != null) {
                for (int index = 0; index < frames.length() && safeFrames.length() < MAX_FRAMES; index++) {
                    JSONObject frame = frames.optJSONObject(index);
                    if (frame == null) continue;
                    safeFrames.put(new JSONObject()
                            .put("class", safeName(frame.optString("class", "unknown")))
                            .put("method", safeName(frame.optString("method", "unknown"))));
                }
            }
            return new JSONObject()
                    .put("present", true)
                    .put("timestamp_ms", stored.optLong("timestamp_ms", 0L))
                    .put("exception_type", safeName(stored.optString("exception_type", "unknown")))
                    .put("thread", safeName(stored.optString("thread", "unknown")))
                    .put("frames", safeFrames);
        } catch (Exception ignored) {
            return new JSONObject();
        }
    }

    static void clear(Context context) throws Exception {
        File file = recordFile(context);
        if (!file.delete() && file.exists()) throw new IllegalStateException("crash record delete failed");
    }

    private static void record(Context context, Thread thread, Throwable error) throws Exception {
        JSONArray frames = new JSONArray();
        StackTraceElement[] stack = error == null ? new StackTraceElement[0] : error.getStackTrace();
        for (int index = 0; index < stack.length && index < MAX_FRAMES; index++) {
            frames.put(new JSONObject()
                    .put("class", safeName(stack[index].getClassName()))
                    .put("method", safeName(stack[index].getMethodName())));
        }
        JSONObject record = new JSONObject()
                .put("format", "stasis-android-redacted-crash-v1")
                .put("timestamp_ms", System.currentTimeMillis())
                .put("exception_type", safeName(error == null ? "unknown" : error.getClass().getName()))
                .put("thread", safeName(thread == null ? "unknown" : thread.getName()))
                .put("frames", frames)
                .put("message_excluded", true)
                .put("paths_and_source_excluded", true);
        byte[] bytes = record.toString().getBytes(StandardCharsets.UTF_8);
        if (bytes.length > MAX_RECORD_BYTES) throw new IllegalStateException("crash record exceeds bound");
        File target = recordFile(context);
        File temporary = new File(target.getParentFile(), target.getName() + ".tmp");
        FileOutputStream output = new FileOutputStream(temporary);
        try {
            output.write(bytes);
            output.getFD().sync();
        } finally {
            output.close();
        }
        Files.move(temporary.toPath(), target.toPath(), StandardCopyOption.REPLACE_EXISTING);
    }

    private static File recordFile(Context context) throws Exception {
        File file = new File(context.getFilesDir(), FILE_NAME);
        String root = context.getFilesDir().getCanonicalPath();
        if (!file.getCanonicalPath().startsWith(root + File.separator)) {
            throw new IllegalStateException("crash record path escaped app storage");
        }
        return file;
    }

    private static String safeName(String value) {
        if (value == null) return "unknown";
        String safe = value.replaceAll("[^A-Za-z0-9_.$<>-]", "_");
        if (safe.isEmpty()) return "unknown";
        return safe.length() > 160 ? safe.substring(0, 160) : safe;
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
}
