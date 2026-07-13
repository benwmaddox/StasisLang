package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import java.io.File;
import java.nio.file.Files;

import org.junit.Test;

public final class WorkshopAiTemporaryVerificationTest {
    @Test
    public void temporaryTestIsRemovedAfterSuccess() throws Exception {
        File project = Files.createTempDirectory("workshop-verification").toFile();
        WorkshopAiTemporaryVerification.Result result = WorkshopAiTemporaryVerification.run(
                project, "test `temporary`(): bool { return true; }",
                testFile -> {
                    assertTrue(testFile.isFile());
                    return new WorkshopAiTemporaryVerification.Result(true, "passed");
                });
        assertTrue(result.passed);
        assertEquals(0, new File(project, "tests").listFiles().length);
    }

    @Test
    public void temporaryTestIsRemovedAfterExecutorFailure() throws Exception {
        File project = Files.createTempDirectory("workshop-verification-failure").toFile();
        try {
            WorkshopAiTemporaryVerification.run(project,
                    "test `temporary`(): bool { return false; }",
                    testFile -> { throw new IllegalStateException("expected failure"); });
        } catch (IllegalStateException expected) {
            assertEquals("expected failure", expected.getMessage());
        }
        File[] remaining = new File(project, "tests").listFiles();
        assertFalse(remaining == null);
        assertEquals(0, remaining.length);
    }

    @Test
    public void verifierMustAddAtLeastOnePassingTest() {
        assertTrue(WorkshopAiTemporaryVerification.acceptedRun(4, 5, 0));
        assertFalse(WorkshopAiTemporaryVerification.acceptedRun(4, 4, 0));
        assertFalse(WorkshopAiTemporaryVerification.acceptedRun(4, 5, 1));
    }
}
