package com.stasislang.workshop;

import org.junit.Test;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

public final class WorkshopAiInitialContextPolicyTest {
    @Test
    public void permitsCandidateInsideBothBounds() {
        assertTrue(WorkshopAiInitialContextPolicy.canAppend(100, 200, 3));
    }

    @Test
    public void rejectsCharacterOverflow() {
        assertFalse(WorkshopAiInitialContextPolicy.canAppend(
                WorkshopAiInitialContextPolicy.MAX_SYMBOL_INDEX_CHARS - 2, 2, 1));
    }

    @Test
    public void rejectsEntryOverflow() {
        assertFalse(WorkshopAiInitialContextPolicy.canAppend(
                0, 1, WorkshopAiInitialContextPolicy.MAX_SYMBOLS));
    }
}
