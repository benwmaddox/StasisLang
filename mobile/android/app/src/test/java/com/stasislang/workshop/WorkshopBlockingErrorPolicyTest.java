package com.stasislang.workshop;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class WorkshopBlockingErrorPolicyTest {
    @Test
    public void compileAndFrameErrorsAreProminentOnlyWithoutRunningGame() {
        assertTrue(WorkshopBlockingErrorPolicy.shouldShow(false, "CompileError: bad source"));
        assertTrue(WorkshopBlockingErrorPolicy.shouldShow(false, "RunError: invalid frame"));
        assertFalse(WorkshopBlockingErrorPolicy.shouldShow(true, "CompileError: stale edit"));
        assertFalse(WorkshopBlockingErrorPolicy.shouldShow(false, "CompileReady: status=0"));
    }

    @Test
    public void summaryRemovesPrivateRootAndFormatsSourceLocation() {
        String status = "CompileError: /data/data/com.stasis/files/project/src/main.stasis: bad token"
                + "|diagnostic_file=src/main.stasis|diagnostic_line=12|diagnostic_column=7";
        String summary = WorkshopBlockingErrorPolicy.summary(
                status, "/data/user/0/com.stasis/files/project");
        assertFalse(summary.contains("/data/data"));
        assertTrue(summary.contains("src/main.stasis: bad token"));
        assertTrue(summary.contains("src/main.stasis:12:7"));
    }
}
