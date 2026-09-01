package com.stasislang.workshop;

import java.util.ArrayList;

/** Deterministic shelf packing used by the embedded GLES renderer. */
final class WorkshopSpriteAtlas {
    static final int PADDING = 1;
    static final int DEFAULT_PAGE_SIZE = 2048;
    static final int MIN_PAGE_SIZE = 256;

    static final class Region {
        final int page;
        final int x;
        final int y;
        final int width;
        final int height;

        Region(int page, int x, int y, int width, int height) {
            this.page = page;
            this.x = x;
            this.y = y;
            this.width = width;
            this.height = height;
        }
    }

    private static final class ShelfPage {
        int cursorX;
        int cursorY;
        int rowHeight;
    }

    private final int pageSize;
    private final ArrayList<ShelfPage> pages = new ArrayList<>();

    WorkshopSpriteAtlas(int maximumTextureSize) {
        pageSize = Math.max(MIN_PAGE_SIZE,
                Math.min(DEFAULT_PAGE_SIZE, Math.max(MIN_PAGE_SIZE, maximumTextureSize)));
    }

    int pageSize() { return pageSize; }
    int pageCount() { return pages.size(); }

    static int chooseSolidTexture(int activeTexture, int followingTexture) {
        return activeTexture != 0 ? activeTexture : followingTexture;
    }

    static float atlasCoordinate(float regionStart, float regionEnd,
            float logicalSource, float logicalExtent) {
        return regionStart + logicalSource / logicalExtent * (regionEnd - regionStart);
    }

    /** Pure acceptance model of the renderer's order-preserving page selection. */
    static int countMixedRuns(boolean[] solids, int[] spriteTextures, int count, int capacity) {
        if (solids == null || spriteTextures == null || count <= 0 || capacity <= 0) return 0;
        count = Math.min(count, Math.min(solids.length, spriteTextures.length));
        int runs = 0;
        int active = 0;
        int quads = 0;
        for (int index = 0; index < count; index += 1) {
            int texture = spriteTextures[index];
            if (solids[index]) {
                texture = active;
                if (texture == 0) {
                    int end = Math.min(count, index + 33);
                    for (int next = index + 1; next < end; next += 1) {
                        if (!solids[next]) {
                            texture = spriteTextures[next];
                            break;
                        }
                    }
                }
            }
            if (texture == 0) texture = -1; // default atlas domain
            if (quads == 0 || texture != active || quads == capacity) {
                runs += 1;
                active = texture;
                quads = 0;
            }
            quads += 1;
        }
        return runs;
    }

    Region allocate(int width, int height) {
        if (width <= 0 || height <= 0) return null;
        int paddedWidth = width + PADDING * 2;
        int paddedHeight = height + PADDING * 2;
        if (paddedWidth > pageSize || paddedHeight > pageSize) return null;
        for (int page = 0; page < pages.size(); page += 1) {
            Region region = allocateOnPage(pages.get(page), page, width, height,
                    paddedWidth, paddedHeight);
            if (region != null) return region;
        }
        ShelfPage page = new ShelfPage();
        // Every page reserves a small host-private header for the white solid texel
        // and the missing-asset checker. Sprite packing can never overwrite it.
        page.cursorY = 4;
        pages.add(page);
        return allocateOnPage(page, pages.size() - 1, width, height,
                paddedWidth, paddedHeight);
    }

    private Region allocateOnPage(ShelfPage page, int pageIndex, int width, int height,
            int paddedWidth, int paddedHeight) {
        if (page.cursorX + paddedWidth > pageSize) {
            page.cursorX = 0;
            page.cursorY += page.rowHeight;
            page.rowHeight = 0;
        }
        if (page.cursorY + paddedHeight > pageSize) return null;
        Region result = new Region(pageIndex, page.cursorX + PADDING,
                page.cursorY + PADDING, width, height);
        page.cursorX += paddedWidth;
        page.rowHeight = Math.max(page.rowHeight, paddedHeight);
        return result;
    }
}
