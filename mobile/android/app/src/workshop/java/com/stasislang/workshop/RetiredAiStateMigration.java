package com.stasislang.workshop;

import android.content.Context;

import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.FileVisitResult;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.SimpleFileVisitor;
import java.nio.file.attribute.BasicFileAttributes;

/** Upgrade cleanup for state exclusively owned by the retired AI feature. */
final class RetiredAiStateMigration {
    private static final String COMPLETED = "retired_ai_cleanup_v1";
    private static final String[] RETIRED_FILES = {
            "codex", "workshop_ai_queue", "workshop_ai_sessions", "workshop_ai_transactions",
            "ai_trace.jsonl", "ai_usage.jsonl"
    };

    interface PreferenceRemover {
        void remove(String name) throws IOException;
    }

    private RetiredAiStateMigration() {}

    static void migrate(Context context) throws IOException {
        migrate(context.getFilesDir(), name -> {
            if (!context.deleteSharedPreferences(name)) {
                throw new IOException("Unable to remove retired AI preferences");
            }
        });
    }

    static void migrate(File filesDir, PreferenceRemover preferences) throws IOException {
        Path root = filesDir.toPath();
        Path completed = root.resolve(COMPLETED);
        if (Files.isRegularFile(completed)) return;
        // The credential encryption key is shared with GitHub; delete only AI preferences.
        preferences.remove("ai_settings");
        for (String name : RETIRED_FILES) {
            Path retired = root.resolve(name);
            if (!Files.exists(retired, java.nio.file.LinkOption.NOFOLLOW_LINKS)) continue;
            // walkFileTree does not follow symlinks into projects or other retained storage.
            Files.walkFileTree(retired, new SimpleFileVisitor<Path>() {
                @Override public FileVisitResult visitFile(Path file, BasicFileAttributes attributes)
                        throws IOException {
                    Files.delete(file);
                    return FileVisitResult.CONTINUE;
                }

                @Override public FileVisitResult postVisitDirectory(Path directory, IOException error)
                        throws IOException {
                    if (error != null) throw error;
                    Files.delete(directory);
                    return FileVisitResult.CONTINUE;
                }
            });
        }
        // Mark completion only after all deletions succeed, so interrupted upgrades retry.
        Files.write(completed, "complete\n".getBytes(StandardCharsets.UTF_8));
    }
}
