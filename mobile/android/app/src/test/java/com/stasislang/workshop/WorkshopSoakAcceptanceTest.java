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
}
