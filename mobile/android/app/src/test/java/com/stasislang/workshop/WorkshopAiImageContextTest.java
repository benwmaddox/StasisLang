package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

public final class WorkshopAiImageContextTest {
    @Test
    public void identifiesRoughSketchesSeparatelyFromProjectArt() {
        assertEquals("design_sketch", WorkshopAiImageContext.kind(true));
        assertEquals("project_asset", WorkshopAiImageContext.kind(false));
        assertEquals("design sketch", WorkshopAiImageContext.reviewLabel(true));
    }
}
