package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import org.json.JSONObject;
import org.junit.Test;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.LinkedHashMap;

public final class AndroidAiSessionCheckpointStoreTest {
    private static final String ITEM_A = "11111111-1111-1111-1111-111111111111";
    private static final String ITEM_B = "22222222-2222-2222-2222-222222222222";

    @Test
    public void roundTripsBoundedStateAndRejectsTamperedProjectHash() throws Exception {
        File filesDir = Files.createTempDirectory("workshop-ai-session-store").toFile();
        AndroidAiSessionCheckpointStore.Checkpoint checkpoint = checkpoint("alpha", ITEM_A, "one");
        AndroidAiSessionCheckpointStore.save(filesDir, checkpoint);

        AndroidAiSessionCheckpointStore.Checkpoint loaded =
                AndroidAiSessionCheckpointStore.load(filesDir, "alpha", ITEM_A);
        assertNotNull(loaded);
        assertEquals(WorkshopAiResumePolicy.RESPONSE_READY, loaded.stage);
        assertEquals("response", loaded.payload.getString("pending_response_json"));
        assertEquals(WorkshopAiProjectTransaction.fingerprint(checkpoint.projectSnapshot),
                WorkshopAiProjectTransaction.fingerprint(loaded.projectSnapshot));

        File stored = new File(filesDir,
                "workshop_ai_sessions/alpha-" + ITEM_A + ".json");
        String json = new String(Files.readAllBytes(stored.toPath()), StandardCharsets.UTF_8);
        int hashStart = json.indexOf("\"project_fingerprint\":\"")
                + "\"project_fingerprint\":\"".length();
        String tampered = json.substring(0, hashStart) + "0".repeat(64)
                + json.substring(hashStart + 64);
        Files.write(stored.toPath(), tampered.getBytes(StandardCharsets.UTF_8));
        expectFailure(() -> AndroidAiSessionCheckpointStore.load(filesDir, "alpha", ITEM_A),
                "project hash");
    }

    @Test
    public void projectEraseDoesNotDeletePrefixNamedProjectAndClearAllRemovesRemainder() throws Exception {
        File filesDir = Files.createTempDirectory("workshop-ai-session-isolation").toFile();
        AndroidAiSessionCheckpointStore.save(filesDir, checkpoint("a", ITEM_A, "one"));
        AndroidAiSessionCheckpointStore.save(filesDir, checkpoint("a-b", ITEM_B, "two"));

        AndroidAiSessionCheckpointStore.clearProject(filesDir, "a");
        assertNull(AndroidAiSessionCheckpointStore.load(filesDir, "a", ITEM_A));
        assertNotNull(AndroidAiSessionCheckpointStore.load(filesDir, "a-b", ITEM_B));
        AndroidAiSessionCheckpointStore.clearAll(filesDir);
        assertNull(AndroidAiSessionCheckpointStore.load(filesDir, "a-b", ITEM_B));
    }

    @Test
    public void rejectsCheckpointLargerThanBound() throws Exception {
        File filesDir = Files.createTempDirectory("workshop-ai-session-bound").toFile();
        LinkedHashMap<String, String> files = new LinkedHashMap<>();
        files.put("src/main.stasis", "x".repeat(8 * 1024 * 1024));
        AndroidAiSessionCheckpointStore.Checkpoint checkpoint =
                new AndroidAiSessionCheckpointStore.Checkpoint("alpha", ITEM_A,
                        WorkshopAiResumePolicy.READY, "openai_api", "gpt-test",
                        "a".repeat(64), new JSONObject().put("initial_request_json", "{}"),
                        new WorkshopAiProjectTransaction.Snapshot(files));

        expectFailure(() -> AndroidAiSessionCheckpointStore.save(filesDir, checkpoint),
                "size limit");
    }

    private static AndroidAiSessionCheckpointStore.Checkpoint checkpoint(
            String projectId, String itemId, String source) throws Exception {
        LinkedHashMap<String, String> files = new LinkedHashMap<>();
        files.put("src/main.stasis", source);
        return new AndroidAiSessionCheckpointStore.Checkpoint(projectId, itemId,
                WorkshopAiResumePolicy.RESPONSE_READY, "openai_api", "gpt-test",
                "a".repeat(64), new JSONObject()
                        .put("initial_request_json", "{}")
                        .put("current_request_json", "{}")
                        .put("pending_response_json", "response"),
                new WorkshopAiProjectTransaction.Snapshot(files));
    }

    private static void expectFailure(ThrowingAction action, String expected) throws Exception {
        try {
            action.run();
        } catch (IllegalArgumentException error) {
            assertTrue(error.getMessage().contains(expected));
            return;
        }
        throw new AssertionError("expected failure containing " + expected);
    }

    private interface ThrowingAction {
        void run() throws Exception;
    }
}
