package com.stasislang.workshop;

import static org.junit.Assert.assertArrayEquals;

import org.junit.Test;

public class WorkshopDiagnosticSeamAcceptanceTest {
    @Test public void casesAreTheSingleOrderedNativeSequence() {
        assertArrayEquals(new String[] {"parse", "extern_resolution", "runtime_entry",
                "render_schema", "missing_resource"},
                WorkshopDiagnosticSeamAcceptance.caseNames());
    }
}
