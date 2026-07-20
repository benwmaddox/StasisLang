package com.stasislang.workshop;

final class WorkshopAiQueueRunPolicy {
    enum Decision {
        IDLE,
        WAIT_FOR_NETWORK,
        CLAIM_NEXT
    }

    private WorkshopAiQueueRunPolicy() {}

    static Decision decide(boolean recoveryPaused, boolean aiRunActive,
            boolean longWorkActive, boolean activeItemClaimed, boolean audioRecordingActive,
            boolean hasPendingItem, boolean hasUsableNetwork) {
        if (recoveryPaused || aiRunActive || longWorkActive || activeItemClaimed
                || audioRecordingActive || !hasPendingItem) {
            return Decision.IDLE;
        }
        return hasUsableNetwork ? Decision.CLAIM_NEXT : Decision.WAIT_FOR_NETWORK;
    }
}
