package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class WorkshopExplorationLessonPolicyTest {
    @Test
    public void lessonProgressIsDurableMonotonicAndOrderedForPresentation() {
        int progress = 0;
        assertEquals("tap a destination", WorkshopExplorationLessonPolicy.prompt(progress));

        progress = WorkshopExplorationLessonPolicy.record(
                progress, WorkshopExplorationLessonPolicy.OPENED_EDITOR);
        assertEquals("tap a destination", WorkshopExplorationLessonPolicy.prompt(progress));

        progress = WorkshopExplorationLessonPolicy.observeGame(progress, 1, 0);
        assertEquals("collect a keepsake", WorkshopExplorationLessonPolicy.prompt(progress));
        progress = WorkshopExplorationLessonPolicy.observeGame(progress, 1, 1);
        assertEquals("edit MOVE_SPEED, then Apply", WorkshopExplorationLessonPolicy.prompt(progress));

        progress = WorkshopExplorationLessonPolicy.record(
                progress, WorkshopExplorationLessonPolicy.APPLIED_EDIT);
        assertEquals("Run Tests", WorkshopExplorationLessonPolicy.prompt(progress));
        assertFalse(WorkshopExplorationLessonPolicy.isComplete(progress));
        progress = WorkshopExplorationLessonPolicy.record(
                progress, WorkshopExplorationLessonPolicy.PASSED_TESTS);
        assertEquals("tutorial complete", WorkshopExplorationLessonPolicy.prompt(progress));
        assertTrue(WorkshopExplorationLessonPolicy.isComplete(progress));
    }

    @Test
    public void unrelatedBitsCannotCorruptProgress() {
        assertEquals(0, WorkshopExplorationLessonPolicy.record(0, 1 << 20));
    }

    @Test
    public void legacyTutorialStageSuppliesMissingTapProgress() {
        assertEquals(0, WorkshopExplorationLessonPolicy.effectiveTapCount(false, 0, 0));
        assertEquals(1, WorkshopExplorationLessonPolicy.effectiveTapCount(false, 0, 1));
        assertEquals(0, WorkshopExplorationLessonPolicy.effectiveTapCount(true, 0, 3));
        assertEquals(4, WorkshopExplorationLessonPolicy.effectiveTapCount(true, 4, 0));
    }
}
