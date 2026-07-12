package com.stasislang.workshop;

final class WorkshopAiCompletionStatus {
    private WorkshopAiCompletionStatus() {}

    static String afterEdits(String reloadPhase) {
        return "no change".equals(reloadPhase) ? "applied" : reloadPhase;
    }

    static boolean canFinalizeTestedWrites(boolean wroteTest, int successfulWrites,
            boolean compileReady, boolean runnableTestsPassed) {
        return wroteTest && successfulWrites > 0 && compileReady && runnableTestsPassed;
    }
}
