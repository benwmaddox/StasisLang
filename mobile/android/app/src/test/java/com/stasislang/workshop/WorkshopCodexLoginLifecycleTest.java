package com.stasislang.workshop;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class WorkshopCodexLoginLifecycleTest {
    @Test
    public void repeatedBrowserSwitchesKeepOneResumableLogin() {
        WorkshopCodexLoginLifecycle lifecycle = new WorkshopCodexLoginLifecycle();
        lifecycle.onLoginStarted();
        lifecycle.onResume();
        assertTrue(lifecycle.shouldPresentDialog(false, true));
        assertTrue(lifecycle.schedulePoll());
        assertFalse(lifecycle.schedulePoll());

        lifecycle.onPause();
        assertFalse(lifecycle.schedulePoll());
        lifecycle.onResume();
        assertTrue(lifecycle.shouldPresentDialog(false, true));
        assertTrue(lifecycle.schedulePoll());

        lifecycle.onPause();
        lifecycle.onResume();
        assertTrue(lifecycle.shouldPresentDialog(false, true));
        assertTrue(lifecycle.onSignedIn());
        assertFalse(lifecycle.onSignedIn());
    }

    @Test
    public void statusRequestsAndPollsDoNotOverlap() {
        WorkshopCodexLoginLifecycle lifecycle = new WorkshopCodexLoginLifecycle();
        lifecycle.onLoginStarted();
        lifecycle.onResume();
        assertTrue(lifecycle.beginStatusRequest());
        assertFalse(lifecycle.beginStatusRequest());
        assertFalse(lifecycle.schedulePoll());
        lifecycle.finishStatusRequest();
        assertTrue(lifecycle.schedulePoll());
    }
}
