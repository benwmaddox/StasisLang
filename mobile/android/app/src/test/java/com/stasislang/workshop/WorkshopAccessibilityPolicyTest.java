package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class WorkshopAccessibilityPolicyTest {
    @Test
    public void coreTextSurfacesMeetNormalTextContrast() {
        assertTrue(WorkshopAccessibilityPolicy.contrastRatio(
                WorkshopAccessibilityPolicy.PRIMARY_TEXT,
                WorkshopAccessibilityPolicy.PANEL_BACKGROUND) >= 4.5);
        assertTrue(WorkshopAccessibilityPolicy.contrastRatio(
                WorkshopAccessibilityPolicy.SECONDARY_TEXT,
                WorkshopAccessibilityPolicy.PANEL_BACKGROUND) >= 4.5);
        assertTrue(WorkshopAccessibilityPolicy.contrastRatio(
                WorkshopAccessibilityPolicy.DIAGNOSTIC_TEXT,
                WorkshopAccessibilityPolicy.PANEL_BACKGROUND) >= 4.5);
        assertTrue(WorkshopAccessibilityPolicy.contrastRatio(
                WorkshopAccessibilityPolicy.ON_DARK_CONTROL,
                WorkshopAccessibilityPolicy.DARK_CONTROL) >= 4.5);
        assertTrue(WorkshopAccessibilityPolicy.contrastRatio(
                WorkshopAccessibilityPolicy.FOCUS_BORDER,
                WorkshopAccessibilityPolicy.DARK_CONTROL) >= 3.0);
    }

    @Test
    public void paintCursorMovesInBoundedDeterministicSteps() {
        WorkshopAccessibilityPolicy.PaintCursor cursor =
                WorkshopAccessibilityPolicy.initialPaintCursor(16, 20);
        assertEquals(8, cursor.x);
        assertEquals(10, cursor.y);

        cursor = WorkshopAccessibilityPolicy.movePaintCursor(cursor, -1, 0, 16, 20);
        assertEquals(7, cursor.x);
        cursor = WorkshopAccessibilityPolicy.movePaintCursor(cursor, -100, 100, 16, 20);
        assertEquals(0, cursor.x);
        assertEquals(19, cursor.y);
    }
}
