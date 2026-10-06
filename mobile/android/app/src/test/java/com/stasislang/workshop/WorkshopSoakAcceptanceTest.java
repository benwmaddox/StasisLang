package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;
import org.json.JSONObject;

public final class WorkshopSoakAcceptanceTest {
    private static final String SOURCE = "const IT028_TICK_REVISION: i32 = 1;\n"
            + "const IT028_RENDER_REVISION: i32 = 1;\n";

    @Test
    public void restoredCleanupIsAccepted() throws Exception {
        WorkshopSoakAcceptance.requireCleanup(new JSONObject().put("status", "Restored"));
    }

    @Test(expected = IllegalStateException.class)
    public void failedCleanupCannotProducePassingSummary() throws Exception {
        WorkshopSoakAcceptance.requireCleanup(new JSONObject().put("status", "failed"));
    }

    @Test
    public void scheduleIsBoundedAndDeterministic() {
        assertEquals(300, WorkshopSoakAcceptance.FRAME_COUNT);
        assertEquals(1, WorkshopSoakAcceptance.revisionAt(1));
        assertEquals(2, WorkshopSoakAcceptance.revisionAt(75));
        assertEquals(3, WorkshopSoakAcceptance.revisionAt(150));
        assertEquals(4, WorkshopSoakAcceptance.revisionAt(225));
        assertEquals(1, WorkshopSoakAcceptance.revisionAt(300));
        assertTrue(WorkshopSoakAcceptance.isMarker(100,
                WorkshopSoakAcceptance.SURFACE_FRAMES));
        assertFalse(WorkshopSoakAcceptance.isMarker(101,
                WorkshopSoakAcceptance.SURFACE_FRAMES));
    }

    @Test
    public void revisionsKeepTheSourceShapeAndRestoreExactly() {
        String revision = WorkshopSoakAcceptance.sourceForRevision(SOURCE, 4);
        assertEquals(SOURCE.length(), revision.length());
        assertTrue(revision.contains("IT028_TICK_REVISION: i32 = 4"));
        assertEquals(SOURCE, WorkshopSoakAcceptance.sourceForRevision(SOURCE, 1));
    }

    @Test
    public void presentationCounterIgnoresRedrawsAndRejectsOlderTokens() {
        StasisPreviewRenderer.AcceptancePresentationCounter counter =
                new StasisPreviewRenderer.AcceptancePresentationCounter();
        counter.observe(10);
        counter.observe(10);
        counter.observe(11);
        assertEquals(2, counter.count());
        assertEquals(11, counter.lastToken());
        assertTrue(counter.ordered());
        counter.observe(9);
        assertEquals(2, counter.count());
        assertFalse(counter.ordered());
        counter.reset();
        assertEquals(0, counter.count());
        assertEquals(-1, counter.lastToken());
        assertTrue(counter.ordered());
    }

    @Test
    public void presentationBarrierRejectsStaleRepeatedTokenAcrossProjectChanges() {
        StasisPreviewRenderer.PresentationState presentation =
                new StasisPreviewRenderer.PresentationState();
        presentation.observe(17, 101);
        long beforeProjectChange = presentation.serial();
        assertFalse(presentation.matches(17, 101, beforeProjectChange));

        presentation.observe(17, 101);
        assertTrue(presentation.matches(17, 101, beforeProjectChange));
        long beforeTraceChange = presentation.serial();
        presentation.observe(17, 202);

        assertFalse(presentation.matches(17, 101, beforeTraceChange));
        assertTrue(presentation.matches(17, 202, beforeTraceChange));
        long beforeNormalSubmission = presentation.serial();
        presentation.observe(17, -1);
        assertFalse(presentation.matches(17, 202, beforeNormalSubmission));
        assertTrue(presentation.matches(17, -1, beforeNormalSubmission));
        assertFalse(presentation.matches(17, 202, presentation.serial()));
    }

    @Test
    public void failedCandidatePublicationRetainsCommittedTargetsAndIdentity() {
        StasisPreviewRenderer.AcceptedSnapshotState snapshot =
                new StasisPreviewRenderer.AcceptedSnapshotState();
        StasisPreviewRenderer.PresentationState presentation =
                new StasisPreviewRenderer.PresentationState();
        snapshot.initializeTargets(11, 22, 111, 222);
        snapshot.setTargetStorage(624, 962);

        assertTrue(snapshot.adoptCandidate(true, true, 624, 962, 3, 4, 5,
                100, 7, 1L, presentation));
        assertTrue(presentation.matches(100, 7, 0L));
        int acceptedTexture = snapshot.acceptedTexture();
        int candidateTexture = snapshot.candidateTexture();
        int acceptedFramebuffer = snapshot.acceptedFramebuffer();
        int candidateFramebuffer = snapshot.candidateFramebuffer();
        long committedSerial = presentation.serial();

        assertFalse(snapshot.adoptCandidate(true, false, 624, 962, 3, 4, 6,
                101, 8, 2L, presentation));

        assertTrue(snapshot.isAvailableFor(624, 962, 3, 4));
        assertEquals(acceptedTexture, snapshot.acceptedTexture());
        assertEquals(candidateTexture, snapshot.candidateTexture());
        assertEquals(acceptedFramebuffer, snapshot.acceptedFramebuffer());
        assertEquals(candidateFramebuffer, snapshot.candidateFramebuffer());
        assertEquals(624, snapshot.width());
        assertEquals(962, snapshot.height());
        assertEquals(3, snapshot.surfaceGeneration());
        assertEquals(4, snapshot.rendererGeneration());
        assertEquals(5, snapshot.displayGeneration());
        assertEquals(100, snapshot.frameToken());
        assertEquals(1L, snapshot.presentationSerial());
        assertEquals(committedSerial, presentation.serial());
        assertEquals(100, presentation.token());
        assertTrue(presentation.matches(100, 7, 0L));
        assertFalse(presentation.matches(101, 8, committedSerial));

        assertTrue(snapshot.adoptCandidate(true, true, 624, 962, 3, 4, 7,
                102, 9, 2L, presentation));
        assertEquals(candidateTexture, snapshot.acceptedTexture());
        assertEquals(candidateFramebuffer, snapshot.acceptedFramebuffer());
        assertEquals(102, snapshot.frameToken());
        assertEquals(7, snapshot.displayGeneration());
        assertEquals(2L, presentation.serial());
        assertEquals(102, presentation.token());
        assertTrue(presentation.matches(102, 9, committedSerial));
    }
}
