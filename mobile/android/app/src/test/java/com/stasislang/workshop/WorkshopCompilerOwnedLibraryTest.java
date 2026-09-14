package com.stasislang.workshop;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertFalse;

import org.junit.Test;

public final class WorkshopCompilerOwnedLibraryTest {
    @Test
    public void refreshesOnlyCompilerOwnedGraphicsSources() {
        String[] files = WorkshopCompilerOwnedLibrary.refreshedFiles();

        assertArrayEquals(new String[] {
                "graphics.stasis",
                "asset_tasks.stasis",
                "internal/gfx_cmd.stasis"
        }, files);

        files[0] = "storage.stasis";
        assertFalse("storage.stasis".equals(
                WorkshopCompilerOwnedLibrary.refreshedFiles()[0]));
    }
}
