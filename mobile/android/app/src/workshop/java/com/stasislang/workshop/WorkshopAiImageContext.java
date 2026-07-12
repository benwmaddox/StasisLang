package com.stasislang.workshop;

final class WorkshopAiImageContext {
    static final String PROJECT_ASSET = "project_asset";
    static final String DESIGN_SKETCH = "design_sketch";

    private WorkshopAiImageContext() { }

    static String kind(boolean designSketch) {
        return designSketch ? DESIGN_SKETCH : PROJECT_ASSET;
    }

    static String reviewLabel(boolean designSketch) {
        return designSketch ? "design sketch" : "project image";
    }
}
