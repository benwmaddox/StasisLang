package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class WorkshopAiWorkingNotesTest {
    @Test
    public void requiresConciseNonemptyNotesWithinTwoThousandCharacters() {
        assertFalse(WorkshopAiWorkingNotes.isValid(""));
        assertFalse(WorkshopAiWorkingNotes.isValid("   \n"));
        assertTrue(WorkshopAiWorkingNotes.isValid("Intent: inspect render. Next: update both heights."));
        assertTrue(WorkshopAiWorkingNotes.isValid("x".repeat(WorkshopAiWorkingNotes.MAX_CHARS)));
        assertFalse(WorkshopAiWorkingNotes.isValid("x".repeat(WorkshopAiWorkingNotes.MAX_CHARS + 1)));
    }

    @Test
    public void displayCopyIsWhitespaceCollapsedAndBounded() {
        assertEquals("Intent: inspect render Next: write", WorkshopAiWorkingNotes.compactForDisplay(
                "  Intent: inspect render\nNext: write  "));
        assertEquals(320, WorkshopAiWorkingNotes.compactForDisplay("x".repeat(500)).length());
    }
}
