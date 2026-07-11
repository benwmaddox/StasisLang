package com.stasislang.workshop;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class WorkshopNetworkQueuePolicyTest {
    @Test
    public void pendingWorkWaitsWhileOffline() {
        assertTrue(WorkshopNetworkQueuePolicy.shouldWaitForNetwork(true, false));
    }

    @Test
    public void usableNetworkAllowsPendingWorkToStart() {
        assertFalse(WorkshopNetworkQueuePolicy.shouldWaitForNetwork(true, true));
    }

    @Test
    public void emptyQueueDoesNotReportNetworkWaiting() {
        assertFalse(WorkshopNetworkQueuePolicy.shouldWaitForNetwork(false, false));
    }
}
