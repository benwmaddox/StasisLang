package com.stasislang.workshop;

final class WorkshopAccessibilityPolicy {
    static final int PANEL_BACKGROUND = 0xfff7f8fb;
    static final int PRIMARY_TEXT = 0xff161b22;
    static final int SECONDARY_TEXT = 0xff495464;
    static final int DIAGNOSTIC_TEXT = 0xff7d372d;
    static final int DARK_CONTROL = 0xff232d3c;
    static final int DARK_CONTROL_BORDER = 0xff536073;
    static final int FOCUS_BORDER = 0xffffffff;
    static final int ON_DARK_CONTROL = 0xffffffff;

    static final class PaintCursor {
        final int x;
        final int y;

        PaintCursor(int x, int y) {
            this.x = x;
            this.y = y;
        }
    }

    private WorkshopAccessibilityPolicy() {}

    static PaintCursor initialPaintCursor(int width, int height) {
        return new PaintCursor(Math.max(0, width / 2), Math.max(0, height / 2));
    }

    static PaintCursor movePaintCursor(PaintCursor cursor, int deltaX, int deltaY,
            int width, int height) {
        int maxX = Math.max(0, width - 1);
        int maxY = Math.max(0, height - 1);
        return new PaintCursor(clamp(cursor.x + deltaX, 0, maxX),
                clamp(cursor.y + deltaY, 0, maxY));
    }

    static double contrastRatio(int first, int second) {
        double firstLuminance = relativeLuminance(first);
        double secondLuminance = relativeLuminance(second);
        double lighter = Math.max(firstLuminance, secondLuminance);
        double darker = Math.min(firstLuminance, secondLuminance);
        return (lighter + 0.05) / (darker + 0.05);
    }

    private static double relativeLuminance(int color) {
        return 0.2126 * linearChannel((color >> 16) & 0xff)
                + 0.7152 * linearChannel((color >> 8) & 0xff)
                + 0.0722 * linearChannel(color & 0xff);
    }

    private static double linearChannel(int channel) {
        double value = channel / 255.0;
        return value <= 0.04045 ? value / 12.92 : Math.pow((value + 0.055) / 1.055, 2.4);
    }

    private static int clamp(int value, int minimum, int maximum) {
        return Math.max(minimum, Math.min(maximum, value));
    }
}
