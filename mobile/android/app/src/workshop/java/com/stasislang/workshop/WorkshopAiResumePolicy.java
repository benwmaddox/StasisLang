package com.stasislang.workshop;

final class WorkshopAiResumePolicy {
    static final String READY = "ready";
    static final String RESPONSE_READY = "response_ready";
    static final String PROVIDER_IN_FLIGHT = "provider_in_flight";
    static final String CANCEL_REQUESTED = "cancel_requested";

    private WorkshopAiResumePolicy() {}

    static boolean validStage(String stage) {
        return READY.equals(stage) || RESPONSE_READY.equals(stage)
                || PROVIDER_IN_FLIGHT.equals(stage) || CANCEL_REQUESTED.equals(stage);
    }

    static Decision decide(String stage, boolean projectMatches, boolean attachmentsMatch,
            boolean providerMatches, boolean cancelled) {
        if (cancelled || CANCEL_REQUESTED.equals(stage)) {
            return Decision.fresh("The interrupted session was cancelled");
        }
        if (!attachmentsMatch) return Decision.fresh("Queued attachments changed or disappeared");
        if (!providerMatches) return Decision.fresh("The selected AI provider or model changed");
        if (PROVIDER_IN_FLIGHT.equals(stage)) {
            return Decision.fresh("A paid provider call may have completed; it will not be replayed");
        }
        if (!projectMatches) return Decision.fresh("Project files changed after the safe checkpoint");
        if (!READY.equals(stage) && !RESPONSE_READY.equals(stage)) {
            return Decision.fresh("The session checkpoint stage is invalid");
        }
        return new Decision(true, "Safe continuation is available");
    }

    static final class Decision {
        final boolean resumable;
        final String detail;

        Decision(boolean resumable, String detail) {
            this.resumable = resumable;
            this.detail = detail;
        }

        static Decision fresh(String detail) {
            return new Decision(false, detail + "; use Fresh Retry to start a new budget-checked run");
        }
    }
}
