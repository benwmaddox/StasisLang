package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import org.json.JSONArray;
import org.junit.Test;

import java.io.File;
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
                Collections.singleton(safe.id), Collections.<String>emptySet()));
        assertEquals(1, AndroidAiQueue.recoverInterrupted(filesDir, "unsafe",
                Collections.<String>emptySet(), Collections.<String>emptySet()));
        assertEquals(1, AndroidAiQueue.recoverInterrupted(filesDir, "cancelled",
                Collections.<String>emptySet(), Collections.singleton(cancelled.id)));

        assertEquals(AndroidAiQueue.PENDING,
                AndroidAiQueue.list(filesDir, "safe").get(0).state);
        assertEquals(AndroidAiQueue.FAILED,
                AndroidAiQueue.list(filesDir, "unsafe").get(0).state);
        AndroidAiQueue.Entry recoveredCancellation =
                AndroidAiQueue.list(filesDir, "cancelled").get(0);
        assertEquals(AndroidAiQueue.CANCELLED, recoveredCancellation.state);
        assertTrue(recoveredCancellation.detail.contains("project was restored"));
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
                new JSONArray(), null, false, null, 0, 0);
    }
}
