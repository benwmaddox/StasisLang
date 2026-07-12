package com.stasislang.workshop;

final class WorkshopAiVerificationResult {
    enum Status {
        VERIFIED,
        INCONCLUSIVE,
        FAILED
    }

    final Status status;
    final WorkshopAiVerificationPolicy.Risk risk;
    final int passedChecks;
    final int totalChecks;
    final String evidence;
    final long elapsedMs;

    WorkshopAiVerificationResult(Status status, WorkshopAiVerificationPolicy.Risk risk,
            int passedChecks, int totalChecks, String evidence, long elapsedMs) {
        this.status = status;
        this.risk = risk;
        this.passedChecks = passedChecks;
        this.totalChecks = totalChecks;
        this.evidence = evidence == null ? "" : evidence;
        this.elapsedMs = Math.max(0L, elapsedMs);
    }

    boolean canApplyAutomatically() {
        return status == Status.VERIFIED;
    }

    String summary() {
        return status.name().toLowerCase() + " " + passedChecks + "/" + totalChecks
                + (evidence.isEmpty() ? "" : " - " + evidence);
    }
}
