package com.stasislang.workshop;

import static org.junit.Assert.*;

import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.concurrent.atomic.AtomicInteger;

import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

public final class RetiredAiStateMigrationTest {
    @Rule public final TemporaryFolder temporary = new TemporaryFolder();

    @Test public void populatedUpgradePurgesOnlyRetiredStateAndIsIdempotent() throws Exception {
        File files = temporary.newFolder("files");
        File preferences = temporary.newFolder("shared_prefs");
        write(preferences, "ai_settings.xml", "encrypted_v1_openai_api_key=old-secret");
        write(preferences, "ai_settings.xml.bak", "openai_api_key=legacy-secret");
        File github = write(preferences, "github_sync_settings.xml", "encrypted_v1_github_token=retain");
        File manual = write(preferences, "workshop_ui.xml", "editor_open=true");
        String[] retired = {"codex/auth.json", "codex/sessions/turn.jsonl",
                "workshop_ai_queue/project.json", "workshop_ai_sessions/checkpoint.json",
                "workshop_ai_transactions/transaction.json", "ai_trace.jsonl", "ai_usage.jsonl"};
        for (String path : retired) write(files, path, "private source and prompt");
        String[] retained = {"workshop_projects/game/src/main.stasis", "workshop_projects/game/assets/ai.png",
                "workshop_project_baselines/game/src/main.stasis", "stasis_preferences/manual.json",
                "crash.json", "ai_trace.jsonl.user-copy"};
        for (String path : retained) write(files, path, "retained:" + path);
        AtomicInteger calls = new AtomicInteger();
        RetiredAiStateMigration.PreferenceRemover remover = name -> {
            assertEquals("ai_settings", name);
            calls.incrementAndGet();
            Files.deleteIfExists(new File(preferences, name + ".xml").toPath());
            Files.deleteIfExists(new File(preferences, name + ".xml.bak").toPath());
        };
        RetiredAiStateMigration.migrate(files, remover);
        RetiredAiStateMigration.migrate(files, remover);
        assertEquals(1, calls.get());
        for (String path : retired) assertFalse(path, new File(files, path).exists());
        for (String path : retained) assertEquals("retained:" + path, read(new File(files, path)));
        assertFalse(new File(preferences, "ai_settings.xml").exists());
        assertFalse(new File(preferences, "ai_settings.xml.bak").exists());
        assertEquals("encrypted_v1_github_token=retain", read(github));
        assertEquals("editor_open=true", read(manual));
    }

    @Test public void failedPreferenceDeletionLeavesMigrationRetryable() throws Exception {
        File files = temporary.newFolder("retry");
        File auth = write(files, "codex/auth.json", "credential");
        try {
            RetiredAiStateMigration.migrate(files, name -> { throw new IOException("denied"); });
            fail("expected deletion failure");
        } catch (IOException expected) {
            assertEquals("denied", expected.getMessage());
        }
        assertTrue(auth.exists());
        assertFalse(new File(files, "retired_ai_cleanup_v1").exists());
        RetiredAiStateMigration.migrate(files, name -> {});
        assertFalse(auth.exists());
        assertTrue(new File(files, "retired_ai_cleanup_v1").exists());
    }

    @Test public void freshInstallCompletesWithoutRetiredFiles() throws Exception {
        File files = temporary.newFolder("fresh");
        RetiredAiStateMigration.migrate(files, name -> assertEquals("ai_settings", name));
        assertTrue(new File(files, "retired_ai_cleanup_v1").isFile());
    }

    private static File write(File root, String path, String value) throws IOException {
        Path file = new File(root, path).toPath();
        Files.createDirectories(file.getParent());
        Files.write(file, value.getBytes(StandardCharsets.UTF_8));
        return file.toFile();
    }

    private static String read(File file) throws IOException {
        return new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8);
    }
}
