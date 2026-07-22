package com.stasislang.workshop;

import org.junit.Test;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

public final class WorkshopAiSymbolDiscoveryTest {
    @Test
    public void resolvesImportsRelativeToTheirSourceFile() {
        assertEquals("src/shared/math.stasis", WorkshopAiSymbolDiscovery.resolveImport(
                "src/systems/movement.stasis", "../shared/math.stasis"));
    }

    @Test
    public void matchesCompactFiltersWithoutSource() {
        assertTrue(WorkshopAiSymbolDiscovery.matches("update_paddle", "update_paddle(): void",
                "function", "Root", "paddle", "function", "Root"));
        assertFalse(WorkshopAiSymbolDiscovery.matches("render", "render(): i32",
                "function", "Main", "paddle", "", ""));
    }

    @Test
    public void boundsRequestedPageSize() {
        assertEquals(1, WorkshopAiSymbolDiscovery.boundedLimit(0));
        assertEquals(32, WorkshopAiSymbolDiscovery.boundedLimit(32));
        assertEquals(200, WorkshopAiSymbolDiscovery.boundedLimit(500));
    }
}
