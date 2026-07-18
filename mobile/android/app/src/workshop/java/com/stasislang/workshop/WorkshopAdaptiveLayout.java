package com.stasislang.workshop;

final class WorkshopAdaptiveLayout {
    private static final int MEDIUM_WIDTH_DP = 600;
    private static final int EXPANDED_WIDTH_DP = 840;
    private static final float LARGE_TEXT_SCALE = 1.3f;

    enum SizeClass { COMPACT, MEDIUM, EXPANDED }

    static final class Profile {
        final SizeClass sizeClass;
        final boolean fullWidthEditor;
        final boolean stackActions;
        final int editorWidthDp;
        final int paintCanvasHeightDp;

        Profile(SizeClass sizeClass, boolean fullWidthEditor, boolean stackActions,
                int editorWidthDp, int paintCanvasHeightDp) {
            this.sizeClass = sizeClass;
            this.fullWidthEditor = fullWidthEditor;
            this.stackActions = stackActions;
            this.editorWidthDp = editorWidthDp;
            this.paintCanvasHeightDp = paintCanvasHeightDp;
        }
    }

    private WorkshopAdaptiveLayout() {}

    static Profile profile(int screenWidthDp, int screenHeightDp, float fontScale) {
        int width = Math.max(1, screenWidthDp);
        int height = Math.max(1, screenHeightDp);
        SizeClass sizeClass = width >= EXPANDED_WIDTH_DP
                ? SizeClass.EXPANDED
                : width >= MEDIUM_WIDTH_DP ? SizeClass.MEDIUM : SizeClass.COMPACT;
        boolean fullWidthEditor = sizeClass == SizeClass.COMPACT;
        boolean stackActions = width < 480 || fontScale >= LARGE_TEXT_SCALE;
        int editorWidthDp = 0;
        if (sizeClass == SizeClass.MEDIUM) {
            editorWidthDp = Math.min(560, Math.max(360, width * 70 / 100));
        } else if (sizeClass == SizeClass.EXPANDED) {
            editorWidthDp = Math.min(720, Math.max(560, width * 55 / 100));
        }
        int paintCanvasHeightDp = Math.max(240, Math.min(480, height * 45 / 100));
        return new Profile(sizeClass, fullWidthEditor, stackActions, editorWidthDp,
                paintCanvasHeightDp);
    }
}
