package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;
import org.junit.Test;

public final class WorkshopAiObservationCompactorTest {
    @Test
    public void successfulWriteRetainsIdentityButNotFullSource() throws Exception {
        WorkshopAiObservationCompactor.SourceMetadata metadata =
                WorkshopAiObservationCompactor.describe("function tick(): void {}");
        assertEquals(24, metadata.characters);
        assertTrue(metadata.sha256.matches("[0-9a-f]{64}"));
    }
}
