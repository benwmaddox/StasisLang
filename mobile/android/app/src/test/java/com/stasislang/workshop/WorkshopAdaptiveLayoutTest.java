package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class WorkshopAdaptiveLayoutTest {
    @Test
    public void compactWindowsUseFullWidthPanelsAndStackedActions() {
        WorkshopAdaptiveLayout.Profile profile = WorkshopAdaptiveLayout.profile(360, 640, 1.0f);

        assertEquals(WorkshopAdaptiveLayout.SizeClass.COMPACT, profile.sizeClass);
        assertTrue(profile.fullWidthEditor);
        assertTrue(profile.stackActions);
        assertEquals(288, profile.paintCanvasHeightDp);
    }

    @Test
    public void largeTextStacksActionsBeforeControlsBecomeCompressed() {
        WorkshopAdaptiveLayout.Profile profile = WorkshopAdaptiveLayout.profile(700, 900, 1.4f);

        assertEquals(WorkshopAdaptiveLayout.SizeClass.MEDIUM, profile.sizeClass);
        assertFalse(profile.fullWidthEditor);
        assertEquals(490, profile.editorWidthDp);
        assertTrue(profile.stackActions);
    }

    @Test
    public void mediumWindowsReserveVisiblePreviewSpace() {
        WorkshopAdaptiveLayout.Profile profile = WorkshopAdaptiveLayout.profile(600, 900, 1.0f);

        assertEquals(420, profile.editorWidthDp);
        assertTrue(600 - profile.editorWidthDp >= 180);
    }

    @Test
    public void accessibilityTextScaleStacksCoreActions() {
        WorkshopAdaptiveLayout.Profile profile = WorkshopAdaptiveLayout.profile(840, 600, 2.0f);

        assertEquals(WorkshopAdaptiveLayout.SizeClass.EXPANDED, profile.sizeClass);
        assertTrue(profile.stackActions);
    }

    @Test
    public void expandedWindowsKeepTheRunningPreviewVisible() {
        WorkshopAdaptiveLayout.Profile profile = WorkshopAdaptiveLayout.profile(1200, 800, 1.0f);

        assertEquals(WorkshopAdaptiveLayout.SizeClass.EXPANDED, profile.sizeClass);
        assertFalse(profile.fullWidthEditor);
        assertEquals(660, profile.editorWidthDp);
        assertFalse(profile.stackActions);
        assertEquals(360, profile.paintCanvasHeightDp);
    }
}
