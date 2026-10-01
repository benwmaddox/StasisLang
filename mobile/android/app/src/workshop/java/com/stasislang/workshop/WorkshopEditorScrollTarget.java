package com.stasislang.workshop;

/** Computes the outer Workshop editor scroll needed to reveal a descendant rectangle. */
final class WorkshopEditorScrollTarget {
    private WorkshopEditorScrollTarget() {}

    static boolean shouldScroll(boolean requireSourceFocus, boolean sourceIsFocused) {
        return !requireSourceFocus || sourceIsFocused;
    }

    static int forVisibleRect(int rectTop, int rectBottom, int currentScrollY,
            int viewportHeight, int contentHeight) {
        int viewportTop = Math.max(0, currentScrollY);
        int viewportBottom = viewportTop + Math.max(0, viewportHeight);
        if (rectTop >= viewportTop && rectBottom <= viewportBottom) {
            return viewportTop;
        }

        int target = rectTop < viewportTop ? rectTop : rectBottom - viewportHeight;
        int maximum = Math.max(0, contentHeight - viewportHeight);
        return Math.max(0, Math.min(maximum, target));
    }
}
