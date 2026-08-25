package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import org.json.JSONArray;
import org.json.JSONObject;
import org.junit.Test;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.Collections;

public final class AndroidAiQueueTest {
    @Test
    public void durableQueueClaimsOneFifoItemAndKeepsProjectsIsolated() throws Exception {
        File filesDir = Files.createTempDirectory("workshop-ai-queue-fifo").toFile();
        AndroidAiQueue.Entry alphaOne = enqueue(filesDir, "alpha", "one");
        AndroidAiQueue.Entry alphaTwo = enqueue(filesDir, "alpha", "two");
        AndroidAiQueue.Entry betaOne = enqueue(filesDir, "beta", "other project");

        AndroidAiQueue.Entry claimed = AndroidAiQueue.claimNext(filesDir, "alpha");
        assertEquals(alphaOne.id, claimed.id);
        assertNull(AndroidAiQueue.claimNext(filesDir, "alpha"));
        assertEquals(AndroidAiQueue.PENDING,
                AndroidAiQueue.list(filesDir, "beta").get(0).state);

        assertTrue(AndroidAiQueue.finish(filesDir, "alpha", claimed.id,
                AndroidAiQueue.COMPLETED, "done"));
        assertEquals(alphaTwo.id, AndroidAiQueue.claimNext(filesDir, "alpha").id);
        assertEquals(betaOne.id, AndroidAiQueue.claimNext(filesDir, "beta").id);
    }

    @Test
    public void pendingCancellationPersistsWithoutCallingOrReorderingLaterWork() throws Exception {
        File filesDir = Files.createTempDirectory("workshop-ai-queue-cancel").toFile();
        AndroidAiQueue.Entry cancelled = enqueue(filesDir, "alpha", "cancel me");
        AndroidAiQueue.Entry next = enqueue(filesDir, "alpha", "run me");

        assertTrue(AndroidAiQueue.cancelPending(filesDir, "alpha", cancelled.id));
        assertFalse(AndroidAiQueue.cancelPending(filesDir, "alpha", cancelled.id));
        assertEquals(AndroidAiQueue.CANCELLED,
                AndroidAiQueue.list(filesDir, "alpha").get(0).state);
        assertEquals(next.id, AndroidAiQueue.claimNext(filesDir, "alpha").id);
    }

    @Test
    public void interruptedRecoverySeparatesSafeUnsafeAndCancelledWork() throws Exception {
        File filesDir = Files.createTempDirectory("workshop-ai-queue-recovery").toFile();
        AndroidAiQueue.Entry safe = claimOnly(filesDir, "safe", "resume");
        AndroidAiQueue.Entry unsafe = claimOnly(filesDir, "unsafe", "do not replay");
        AndroidAiQueue.Entry cancelled = claimOnly(filesDir, "cancelled", "restore then cancel");

        assertEquals(1, AndroidAiQueue.recoverInterrupted(filesDir, "safe",
                Collections.singleton(safe.id)));
        assertEquals(1, AndroidAiQueue.recoverInterrupted(filesDir, "unsafe",
                Collections.<String>emptySet()));
        assertTrue(AndroidAiQueue.finish(filesDir, "cancelled", cancelled.id,
                AndroidAiQueue.CANCELLED, WorkshopAiRunPhase.CANCELLED,
                "Cancellation completed during process recovery; the original project was restored"));

        assertEquals(AndroidAiQueue.PENDING,
                AndroidAiQueue.list(filesDir, "safe").get(0).state);
        assertEquals(AndroidAiQueue.FAILED,
                AndroidAiQueue.list(filesDir, "unsafe").get(0).state);
        AndroidAiQueue.Entry recoveredCancellation =
                AndroidAiQueue.list(filesDir, "cancelled").get(0);
        assertEquals(AndroidAiQueue.CANCELLED, recoveredCancellation.state);
        assertTrue(recoveredCancellation.detail.contains("project was restored"));
    }

    @Test
    public void queuePersistsTheExactImageGenerationProfile() throws Exception {
        File filesDir = Files.createTempDirectory("workshop-ai-queue-image-profile").toFile();
        AndroidAiQueue.Entry queued = AndroidAiQueue.enqueue(filesDir, "alpha", "text", "make art",
                new JSONArray(), null, WorkshopImageGenerationProfile.FINAL_PORTRAIT_ID,
                null, 0, 0);

        AndroidAiQueue.Entry restored = AndroidAiQueue.list(filesDir, "alpha").get(0);

        assertTrue(queued.imageGeneration);
        assertEquals(WorkshopImageGenerationProfile.FINAL_PORTRAIT_ID,
                restored.imageGenerationProfile);
        assertEquals(queued.requestFingerprint(), restored.requestFingerprint());
    }

    @Test
    public void versionOneImageGenerationFlagMigratesToDraftProfile() throws Exception {
        File filesDir = Files.createTempDirectory("workshop-ai-queue-image-profile-v1").toFile();
        File root = new File(filesDir, "workshop_ai_queue");
        assertTrue(root.mkdirs());
        JSONObject item = new JSONObject()
                .put("id", "00000000-0000-0000-0000-000000000001")
                .put("project_id", "alpha")
                .put("source", "text")
                .put("prompt", "legacy art")
                .put("created_at_ms", 1L)
                .put("state", AndroidAiQueue.PENDING)
                .put("phase", WorkshopAiRunPhase.QUEUED.wireValue())
                .put("image_attachments", new JSONArray())
                .put("image_generation", true)
                .put("detail", "")
                .put("preview_file", "")
                .put("preview_width", 0)
                .put("preview_height", 0)
                .put("preview_bytes", 0)
                .put("preview_sha256", "");
        JSONObject document = new JSONObject()
                .put("format_version", 1)
                .put("project_id", "alpha")
                .put("items", new JSONArray().put(item));
        Files.write(new File(root, "alpha.json").toPath(),
                document.toString().getBytes(StandardCharsets.UTF_8));

        AndroidAiQueue.Entry restored = AndroidAiQueue.list(filesDir, "alpha").get(0);

        assertEquals(WorkshopImageGenerationProfile.DRAFT_SQUARE_ID,
                restored.imageGenerationProfile);
        assertEquals(1, restored.requestFingerprintVersion);
        JSONObject legacyFingerprintRequest = new JSONObject()
                .put("project_id", "alpha")
                .put("source", "text")
                .put("prompt", "legacy art")
                .put("image_attachments", new JSONArray())
                .put("image_generation", true)
                .put("preview_sha256", "");
        assertEquals(sha256(legacyFingerprintRequest.toString()), restored.requestFingerprint());
    }

    @Test
    public void freshRetryPreservesFinalImageProfile() throws Exception {
        File filesDir = Files.createTempDirectory("workshop-ai-queue-image-retry").toFile();
        AndroidAiQueue.Entry queued = AndroidAiQueue.enqueue(filesDir, "alpha", "text", "make art",
                new JSONArray(), null, WorkshopImageGenerationProfile.FINAL_LANDSCAPE_ID,
                null, 0, 0);
        assertEquals(queued.id, AndroidAiQueue.claimNext(filesDir, "alpha").id);
        assertTrue(AndroidAiQueue.finish(filesDir, "alpha", queued.id,
                AndroidAiQueue.COMPLETED, "done"));
        AndroidAiQueue.Entry terminal = AndroidAiQueue.list(filesDir, "alpha").get(0);

        AndroidAiQueue.Entry retried = AndroidAiQueue.retryTerminal(filesDir, terminal);

        assertEquals(WorkshopImageGenerationProfile.FINAL_LANDSCAPE_ID,
                retried.imageGenerationProfile);
    }

    private static AndroidAiQueue.Entry claimOnly(
            File filesDir, String projectId, String prompt) throws Exception {
        AndroidAiQueue.Entry queued = enqueue(filesDir, projectId, prompt);
        assertEquals(queued.id, AndroidAiQueue.claimNext(filesDir, projectId).id);
        return queued;
    }

    private static AndroidAiQueue.Entry enqueue(
            File filesDir, String projectId, String prompt) throws Exception {
        return AndroidAiQueue.enqueue(filesDir, projectId, "text", prompt,
                new JSONArray(), null, WorkshopImageGenerationProfile.OFF_ID, null, 0, 0);
    }

    private static String sha256(String source) throws Exception {
        byte[] digest = java.security.MessageDigest.getInstance("SHA-256")
                .digest(source.getBytes(StandardCharsets.UTF_8));
        StringBuilder result = new StringBuilder();
        for (byte value : digest) result.append(String.format("%02x", value & 0xff));
        return result.toString();
    }
}
