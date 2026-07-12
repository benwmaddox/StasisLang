package com.stasislang.workshop;

import java.util.Collection;

final class WorkshopAiVerificationRunner {
    private WorkshopAiVerificationRunner() {}

    static WorkshopAiVerificationResult verify(WorkshopAiVerificationPolicy.Decision policy,
            boolean compileReady, boolean generatedTestsPassed, int successfulWrites,
            Collection<String> changedTestFiles, boolean independentChecksPassed) {
        long started = System.nanoTime();
        int total = 3;
        int passed = 0;
        String failure = "";
        if (compileReady) passed += 1; else failure = "provisional project did not compile";
        if (generatedTestsPassed) passed += 1; else if (failure.isEmpty()) failure = "generated tests failed";
        if (successfulWrites > 0) passed += 1; else if (failure.isEmpty()) failure = "no successful writes";
        if (!failure.isEmpty()) {
            return result(WorkshopAiVerificationResult.Status.FAILED, policy, passed, total, failure, started);
        }

        if (policy.requiresBehaviorTest) {
            total += 1;
            if (changedTestFiles != null && !changedTestFiles.isEmpty()) {
                passed += 1;
            } else {
                return result(WorkshopAiVerificationResult.Status.INCONCLUSIVE, policy, passed, total,
                        "behavior changed without a new or updated test", started);
            }
        }
        if (policy.requiresIndependentReview) {
            total += 1;
            if (independentChecksPassed) {
                passed += 1;
            } else {
                return result(WorkshopAiVerificationResult.Status.INCONCLUSIVE, policy, passed, total,
                        "independent verification is required", started);
            }
        }
        return result(WorkshopAiVerificationResult.Status.VERIFIED, policy, passed, total,
                "required local checks passed", started);
    }

    private static WorkshopAiVerificationResult result(WorkshopAiVerificationResult.Status status,
            WorkshopAiVerificationPolicy.Decision policy, int passed, int total, String evidence,
            long startedNanos) {
        return new WorkshopAiVerificationResult(status, policy.risk, passed, total, evidence,
                (System.nanoTime() - startedNanos) / 1_000_000L);
    }
}
