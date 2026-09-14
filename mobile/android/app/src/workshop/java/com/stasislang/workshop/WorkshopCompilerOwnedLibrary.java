package com.stasislang.workshop;

final class WorkshopCompilerOwnedLibrary {
    private static final String[] REFRESHED_FILES = new String[] {
            "graphics.stasis",
            "asset_tasks.stasis",
            "internal/gfx_cmd.stasis"
    };

    private WorkshopCompilerOwnedLibrary() {}

    static String[] refreshedFiles() {
        return REFRESHED_FILES.clone();
    }
}
