package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
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

    @Test
    public void publicGraphicsTemplatesDeclareTheirCanonicalStdlibMounts() {
        for (String templateId : new String[] { "pong", "exploration" }) {
            WorkshopTemplateCatalog.Template template = WorkshopTemplateCatalog.require(templateId);
            assertEquals(1, template.directoryMounts.length);
            assertEquals("stasis_stdlib", template.directoryMounts[0].assetDirectory);
            assertEquals("vendor/stasis/src/stdlib",
                    template.directoryMounts[0].projectDirectory);
        }

        WorkshopTemplateCatalog.Template parity = WorkshopTemplateCatalog.require("render-parity");
        assertEquals(1, parity.directoryMounts.length);
        assertEquals("stasis_stdlib", parity.directoryMounts[0].assetDirectory);
        assertEquals(".stasis_cache/toolchain/src/stdlib",
                parity.directoryMounts[0].projectDirectory);
    }
}
