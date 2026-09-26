package com.stasislang.workshop;

/** Small policy for distinguishing a laid-out preview from an actual drawable GLES surface. */
final class WorkshopRenderSurfaceReadiness {
    private WorkshopRenderSurfaceReadiness() {}

    static boolean isReady(int layoutWidth, int layoutHeight,
            int drawableWidth, int drawableHeight) {
        return hasUsableBounds(layoutWidth, layoutHeight)
                && hasUsableBounds(drawableWidth, drawableHeight);
    }

    static boolean hasUsableBounds(int width, int height) {
        return width > 1 && height > 1;
    }

    static boolean hasTimedOut(long startedAtMillis, long nowMillis, long timeoutMillis) {
        return timeoutMillis > 0L && nowMillis >= startedAtMillis
                && nowMillis - startedAtMillis >= timeoutMillis;
    }
}
