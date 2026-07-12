package com.stasislang.workshop;

final class WorkshopAiCompletionStatus {
    private WorkshopAiCompletionStatus() {}

    static String afterEdits(String reloadPhase) {
        return "no change".equals(reloadPhase) ? "applied" : reloadPhase;
    }
}
