package com.stasislang.workshop;

import java.util.LinkedHashMap;
import java.util.Map;
import java.util.TreeSet;

import org.json.JSONObject;
import org.junit.Test;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

public final class WorkshopAiSymbolDiscoveryTest {
    @Test
    public void resolvesImportsRelativeToTheirSourceFile() {
        assertEquals("src/shared/math.stasis", WorkshopAiSymbolDiscovery.resolveImport(
                "src/systems/movement.stasis", "../shared/math.stasis"));
        assertEquals("vendor/stasis/src/stdlib/storage.stasis",
                WorkshopAiSymbolDiscovery.resolveImport(
                        "src/systems/movement.stasis",
                        "/vendor/stasis/src/stdlib/storage.stasis"));
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

    @Test
    public void preservesMissingDirectImportsAsRepairableContext() throws Exception {
        Map<String, String> sources = new LinkedHashMap<>();
        sources.put("src/main.stasis",
                "import \"missing.stasis\";\nfunction main(): void {}\n");

        TreeSet<String> scope = WorkshopAiSymbolDiscovery.defaultScope(sources);
        assertEquals(2, scope.size());
        assertTrue(scope.contains("src/main.stasis"));
        assertTrue(scope.contains("src/missing.stasis"));
        JSONObject imports = WorkshopAiSymbolDiscovery.importsForFiles(sources, scope);
        assertEquals(
                "[\"src/missing.stasis\"]",
                imports.getJSONArray("src/main.stasis").toString());
        assertEquals(0, imports.getJSONArray("src/missing.stasis").length());
    }
}
