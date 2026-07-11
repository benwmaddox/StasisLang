package com.stasislang.workshop;

final class WorkshopNetworkQueuePolicy {
    private WorkshopNetworkQueuePolicy() {}

    static boolean shouldWaitForNetwork(boolean hasPendingWork, boolean hasUsableNetwork) {
        return hasPendingWork && !hasUsableNetwork;
    }
}
