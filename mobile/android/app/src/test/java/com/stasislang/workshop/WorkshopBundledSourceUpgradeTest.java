package com.stasislang.workshop;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class WorkshopBundledSourceUpgradeTest {
    @Test
    public void replacesOnlyAnUnmodifiedBundledSource() {
        assertTrue(WorkshopBundledSourceUpgrade.shouldReplace(
                "previous packaged source", "previous packaged source"));
        assertFalse(WorkshopBundledSourceUpgrade.shouldReplace(
                "user edit", "previous packaged source"));
        assertFalse(WorkshopBundledSourceUpgrade.shouldReplace(
                "new project without a baseline", null));
    }
}
