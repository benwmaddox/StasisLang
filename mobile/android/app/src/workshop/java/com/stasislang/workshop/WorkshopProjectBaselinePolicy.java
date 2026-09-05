package com.stasislang.workshop;

final class WorkshopProjectBaselinePolicy {
    enum Action {
        KEEP,
        UPDATE_MARKER,
        REBUILD
    }

    private WorkshopProjectBaselinePolicy() {}

    static Action requiredAction(boolean imported, boolean readyExists, boolean readyMatches) {
        if (readyMatches) return Action.KEEP;
        if (imported && readyExists) return Action.UPDATE_MARKER;
        return Action.REBUILD;
    }
}
