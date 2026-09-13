package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

import org.junit.Test;
import org.json.JSONArray;
import org.json.JSONObject;

public final class WorkshopResourceScopeAcceptanceTest {
    private static final String FRAME =
            "function render(font: Font): void {\n"
                    + "    draw_text(font, \"direct parity\", 64.0, 252.0, 0.95, 0.95, 1.0, 1.0);\n"
                    + "}\n";

    @Test
    public void resourceSnapshotRequiresMatchingProviderAndLifecycleEpochs() throws Exception {
        JSONObject resources = new JSONObject()
                .put("surface_generation", 3).put("renderer_generation", 2)
                .put("lifecycle_surface_generation", 4).put("lifecycle_renderer_generation", 2)
                .put("identities", new JSONArray().put(
                        "sprite:101:/alpha:hash:640x360:density=1065353216:surface=3:renderer=2"));
        assertTrue(WorkshopResourceScopeAcceptance.stableIdentities(resources)
                .contains("hash:640x360:density=1065353216"));
        resources.put("lifecycle_surface_generation", 3);
        assertThrows(IllegalStateException.class,
                () -> WorkshopResourceScopeAcceptance.stableIdentities(resources));
        resources.put("lifecycle_surface_generation", 4).put("lifecycle_renderer_generation", 1);
        assertThrows(IllegalStateException.class,
                () -> WorkshopResourceScopeAcceptance.stableIdentities(resources));
    }

    @Test
    public void spriteRestoreComparesContentWhileCheckingItsOwnEpoch() {
        String stable = "sprite:101:/alpha:hash:640x360:density=1065353216";
        assertEquals(WorkshopResourceScopeAcceptance.stableIdentity(
                stable + ":surface=1:renderer=1", 1, 1),
                WorkshopResourceScopeAcceptance.stableIdentity(
                        stable + ":surface=3:renderer=2", 3, 2));
        assertNotEquals(stable, WorkshopResourceScopeAcceptance.stableIdentity(
                stable.replace("/alpha", "/beta") + ":surface=3:renderer=2", 3, 2));
        assertNotEquals(stable, WorkshopResourceScopeAcceptance.stableIdentity(
                stable.replace("hash", "changed") + ":surface=3:renderer=2", 3, 2));
        assertNotEquals(stable, WorkshopResourceScopeAcceptance.stableIdentity(
                stable.replace("640x360", "320x180") + ":surface=3:renderer=2", 3, 2));
        assertThrows(IllegalStateException.class,
                () -> WorkshopResourceScopeAcceptance.stableIdentity(
                        stable + ":surface=1:renderer=1", 3, 2));
        assertThrows(IllegalStateException.class,
                () -> WorkshopResourceScopeAcceptance.stableIdentity(
                        stable + ":surface=3:renderer=1", 3, 2));
        assertThrows(IllegalStateException.class,
                () -> WorkshopResourceScopeAcceptance.stableIdentity(stable, 3, 2));
        assertEquals("font:201:/alpha:hash:24", WorkshopResourceScopeAcceptance.stableIdentity(
                "font:201:/alpha:hash:24", 3, 2));
    }

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
