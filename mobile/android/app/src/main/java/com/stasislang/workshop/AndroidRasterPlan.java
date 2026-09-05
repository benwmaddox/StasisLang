package com.stasislang.workshop;

/** Pure sizing contract shared by frame collection and Android resource preparation. */
final class AndroidRasterPlan {
    static final int MAX_DIMENSION = 16_384;
    static final long MAX_PIXELS = 16_000_000L;

    static final class Requirement {
        private double fullWidth;
        private double fullHeight;
        private double pixelsPerSourceX;
        private double pixelsPerSourceY;
        private boolean included;

        void include(float drawWidth, float drawHeight, float sourceWidth, float sourceHeight,
                float scaleX, float scaleY) {
            double displayedWidth = Math.abs((double)drawWidth * scaleX);
            double displayedHeight = Math.abs((double)drawHeight * scaleY);
            if (!Double.isFinite(displayedWidth) || !Double.isFinite(displayedHeight)) return;
            included = true;
            if (sourceWidth > 0.0f && sourceHeight > 0.0f) {
                pixelsPerSourceX = Math.max(pixelsPerSourceX, displayedWidth / sourceWidth);
                pixelsPerSourceY = Math.max(pixelsPerSourceY, displayedHeight / sourceHeight);
            } else {
                fullWidth = Math.max(fullWidth, displayedWidth);
                fullHeight = Math.max(fullHeight, displayedHeight);
            }
        }

    }

    static final class Result {
        final int width;
        final int height;
        final boolean supported;

        Result(int width, int height, boolean supported) {
            this.width = width;
            this.height = height;
            this.supported = supported;
        }

        String identity(float density, int surfaceGeneration, int rendererGeneration) {
            return width + "x" + height + ":density=" + Float.floatToIntBits(density)
                    + ":surface=" + surfaceGeneration + ":renderer=" + rendererGeneration;
        }
    }

    static Result exact(int sourceWidth, int sourceHeight, Requirement requirement,
            float density, int maximumTextureSize) {
        return exact(sourceWidth, sourceHeight, requirement, density,
                maximumTextureSize, maximumTextureSize);
    }

    static Result exact(int sourceWidth, int sourceHeight, Requirement requirement,
            float density, int maximumWidth, int maximumHeight) {
        if (sourceWidth <= 0 || sourceHeight <= 0 || !Float.isFinite(density)
                || density <= 0.0f) return new Result(1, 1, false);
        Requirement requested = requirement == null ? new Requirement() : requirement;
        double scale = requested.included
                ? Math.max(1.0 / sourceWidth, 1.0 / sourceHeight) : 1.0;
        scale = Math.max(scale, requested.fullWidth / sourceWidth);
        scale = Math.max(scale, requested.fullHeight / sourceHeight);
        scale = Math.max(scale, requested.pixelsPerSourceX);
        scale = Math.max(scale, requested.pixelsPerSourceY);
        scale *= density;
        int width = ceilProduct(sourceWidth, scale);
        int height = ceilProduct(sourceHeight, scale);
        int widthCap = Math.min(MAX_DIMENSION, Math.max(0, maximumWidth));
        int heightCap = Math.min(MAX_DIMENSION, Math.max(0, maximumHeight));
        boolean supported = widthCap > 0 && heightCap > 0
                && width <= widthCap && height <= heightCap
                && (long)width * height <= MAX_PIXELS;
        return new Result(Math.max(1, Math.min(width, widthCap)),
                Math.max(1, Math.min(height, heightCap)), supported);
    }

    private static int ceilProduct(int value, double scale) {
        double result = Math.ceil(value * scale);
        if (!Double.isFinite(result) || result > Integer.MAX_VALUE) return Integer.MAX_VALUE;
        return Math.max(1, (int)result);
    }

    private AndroidRasterPlan() {}
}
