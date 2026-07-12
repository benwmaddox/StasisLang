package com.stasislang.workshop;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class WorkshopAiProviderPolicyTest {
    @Test
    public void phoneNativeBuildDefaultsToCodexEvenWithApiFallbackAvailable() {
        assertTrue(WorkshopAiProviderPolicy.defaultToCodex(true));
        assertFalse(WorkshopAiProviderPolicy.defaultToCodex(false));
    }

    @Test
    public void signInAndOneTimeMigrationPromoteCodexWithoutOverridingLaterChoice() {
        assertTrue(WorkshopAiProviderPolicy.promoteCodexAfterSignIn(true, true));
        assertTrue(WorkshopAiProviderPolicy.promoteCodexAfterSignIn(false, false));
        assertFalse(WorkshopAiProviderPolicy.promoteCodexAfterSignIn(false, true));
    }
}
