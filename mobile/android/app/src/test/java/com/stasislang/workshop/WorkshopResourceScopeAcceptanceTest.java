package com.stasislang.workshop;

import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class WorkshopResourceScopeAcceptanceTest {
    private static final String FRAME =
            "function render(font: Font): void {\n"
                    + "    draw_text(font, \"direct parity\", 64.0, 252.0, 0.95, 0.95, 1.0, 1.0);\n"
                    + "}\n";

    @Test
    public void publicDirectTextCustomizationIsDistinctPerProject() {
        String alpha = WorkshopResourceScopeAcceptance.customizeDirectText(
                FRAME, "scope alpha!!");
        String beta = WorkshopResourceScopeAcceptance.customizeDirectText(
                FRAME, "scope beta!!!");

        assertTrue(alpha.contains("draw_text(font, \"scope alpha!!\""));
        assertTrue(beta.contains("draw_text(font, \"scope beta!!!\""));
        assertNotEquals(alpha, beta);
    }

    @Test
    public void publicDirectTextCustomizationRejectsMissingOrAmbiguousMarker() {
        assertThrows(IllegalStateException.class,
                () -> WorkshopResourceScopeAcceptance.customizeDirectText(
                        "function render(): void {}\n", "scope alpha!!"));
        assertThrows(IllegalStateException.class,
                () -> WorkshopResourceScopeAcceptance.customizeDirectText(
                        FRAME + FRAME, "scope beta!!!"));
    }
}
