package com.stasislang.workshop;

final class WorkshopAiProviderPolicy {
    private WorkshopAiProviderPolicy() { }

    static boolean defaultToCodex(boolean phoneNativeCodexReady) {
        return phoneNativeCodexReady;
    }

    static boolean promoteCodexAfterSignIn(boolean activeLogin, boolean migrationComplete) {
        return activeLogin || !migrationComplete;
    }
}
