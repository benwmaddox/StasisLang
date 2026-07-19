package com.stasislang.workshop;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import java.util.Arrays;

import org.junit.Test;

public final class WorkshopTemplateCatalogTest {
    @Test
    public void desktopHostFilesBelongOnlyToExplorationTemplate() {
        WorkshopTemplateCatalog.Template exploration = WorkshopTemplateCatalog.require("exploration");
        WorkshopTemplateCatalog.Template pong = WorkshopTemplateCatalog.require("pong");

        for (String hostFile : new String[] {
                "src/host.stasis",
                "src/host_aot.stasis",
                "src/host_game.stasis",
                "src/host_runtime.stasis"
        }) {
            assertTrue(Arrays.asList(exploration.sourceFiles).contains(hostFile));
            assertFalse(Arrays.asList(pong.sourceFiles).contains(hostFile));
        }
        assertTrue(Arrays.asList(exploration.auxiliaryFiles).contains("stasis.json"));
    }
}
