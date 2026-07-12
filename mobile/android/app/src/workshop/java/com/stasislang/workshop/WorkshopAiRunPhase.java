package com.stasislang.workshop;

enum WorkshopAiRunPhase {
    QUEUED("queued"),
    PREPARING("preparing"),
    EDITING("editing"),
    COMPILING("compiling"),
    GENERATED_TESTS("generated tests"),
    VERIFYING("verifying"),
    REPAIRING("repairing"),
    APPLYING("applying"),
    VERIFIED("verified"),
    RESTORED("restored"),
    CANCELLED("cancelled"),
    FAILED("failed");

    private final String wireValue;

    WorkshopAiRunPhase(String wireValue) {
        this.wireValue = wireValue;
    }

    String wireValue() {
        return wireValue;
    }

    static WorkshopAiRunPhase fromWireValue(String value) {
        for (WorkshopAiRunPhase phase : values()) {
            if (phase.wireValue.equals(value)) return phase;
        }
        return QUEUED;
    }

    static boolean isWireValue(String value) {
        for (WorkshopAiRunPhase phase : values()) {
            if (phase.wireValue.equals(value)) return true;
        }
        return false;
    }

    boolean terminal() {
        return this == VERIFIED || this == RESTORED || this == CANCELLED || this == FAILED;
    }
}
