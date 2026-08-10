package com.stasislang.workshop;

import android.opengl.GLES20;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import java.nio.IntBuffer;
import java.nio.FloatBuffer;

import org.junit.Test;

public final class StasisPreviewRendererSchemaTest {
    @Test
    public void twoLineContextRestoreLabelNeedsNoTextureOrFontAsset() {
        assertTrue(StasisPreviewRenderer.isValidRestoreLabel());
    }

    @Test
    public void replacedCaptureIncludesEmptyTypedSpriteLanes() {
        StasisPreviewRenderer renderer = new StasisPreviewRenderer(
                new StasisPreviewRenderer.TextureProvider() {
                    @Override public void onResourceGenerationChanged(
                            int surfaceGeneration, int rendererGeneration,
                            boolean discardGpuHandles, String transitionReason) {}
                    @Override public int textureFor(int handle) { return 0; }
                }, ignored -> {});
        StasisPreviewRenderer.LogicalFrameSnapshot[] replaced = {null};
        renderer.requestCapture((bitmap, error, snapshot) -> replaced[0] = snapshot);
        renderer.requestCapture((bitmap, error, snapshot) -> {});

        assertEquals(0, replaced[0].sprites.length);
        assertEquals(0, replaced[0].spriteValues.length);
    }

    @Test
    public void productionSchemaMatchesNativeContract() {
        assertEquals(12_320, StasisPreviewRenderer.I_TEXT_BASE);
        assertEquals(80_004, StasisPreviewRenderer.F_SPRITE_BASE);
        assertEquals(79_996, StasisPreviewRenderer.F_RECT_REVERSE_BASE);
        assertEquals(96_388, StasisPreviewRenderer.F_TEXT_BASE);
        assertEquals(18_464, StasisPreviewRenderer.I_ORDER_BASE);
        assertEquals(34_608, StasisPreviewRenderer.FRAME_I32_CAPACITY);
        assertEquals(108_676, StasisPreviewRenderer.FRAME_F32_CAPACITY);
        assertEquals(65_536, StasisPreviewRenderer.TEXT_U8_CAPACITY);
    }

    @Test
    public void resourceFailureDiagnosticIncludesLifecycleContext() {
        String diagnostic = StasisPreviewRenderer.formatResourceFailure(
                "sprite", 7, "sprites/ball.svg", 24, 16, 72, 48,
                3, 4, "surface_changed", "upload_failed");
        assertTrue(diagnostic.contains("stage=sprite handle=7 path=sprites/ball.svg"));
        assertTrue(diagnostic.contains("logical=24x16 raster=72x48 backend=gles"));
        assertTrue(diagnostic.contains("surface_generation=3 renderer_generation=4"));
        assertTrue(diagnostic.contains("reason=surface_changed failure=upload_failed"));
    }

    @Test
    public void failedTextureCreationCannotBecomeCachedRestoreSuccess() {
        assertFalse(StasisPreviewRenderer.textureCreationSucceeded(0, GLES20.GL_NO_ERROR));
        assertFalse(StasisPreviewRenderer.textureCreationSucceeded(23, GLES20.GL_OUT_OF_MEMORY));
        assertTrue(StasisPreviewRenderer.textureCreationSucceeded(23, GLES20.GL_NO_ERROR));
    }

    @Test
    public void displayViewportPreservesLogicalCanvasAcrossDensityAndOrientation() {
        StasisPreviewRenderer.DisplayViewport phone =
                StasisPreviewRenderer.fitViewport(360, 720, 1080, 2400);
        assertEquals(0, phone.x);
        assertEquals(120, phone.y);
        assertEquals(1080, phone.width);
        assertEquals(2160, phone.height);
        assertEquals(3.0f, phone.contentScale, 0.001f);
        assertEquals(3.0f, phone.rasterScale, 0.001f);

        StasisPreviewRenderer.DisplayViewport fractional =
                StasisPreviewRenderer.fitViewport(800, 600, 1200, 900);
        assertEquals(1.5f, fractional.contentScale, 0.001f);

        StasisPreviewRenderer.DisplayViewport landscape =
                StasisPreviewRenderer.fitViewport(360, 720, 2400, 1080);
        assertEquals(930, landscape.x);
        assertEquals(0, landscape.y);
        assertEquals(540, landscape.width);
        assertEquals(1080, landscape.height);
        assertEquals(1.5f, landscape.rasterScale, 0.001f);
    }

    @Test
    public void validationRequiresProductionMagicAndVersion() {
        IntBuffer frame = IntBuffer.allocate(StasisPreviewRenderer.FRAME_I32_CAPACITY);
        assertFalse(StasisPreviewRenderer.isValidFrame(frame));
        frame.put(0, StasisPreviewRenderer.RENDER_MAGIC);
        frame.put(1, StasisPreviewRenderer.RENDER_VERSION);
        assertTrue(StasisPreviewRenderer.isValidFrame(frame));
        assertFalse(StasisPreviewRenderer.shouldPresent(frame));
        frame.put(StasisPreviewRenderer.I_FLAGS, StasisPreviewRenderer.FLAG_PRESENT);
        assertTrue(StasisPreviewRenderer.shouldPresent(frame));
        frame.put(1, StasisPreviewRenderer.RENDER_V2_VERSION);
        assertTrue(StasisPreviewRenderer.isValidFrame(frame));
        frame.put(1, StasisPreviewRenderer.RENDER_V3_VERSION);
        assertTrue(StasisPreviewRenderer.isValidFrame(frame));
        frame.put(1, 1);
        assertFalse(StasisPreviewRenderer.isValidFrame(frame));
        assertFalse(StasisPreviewRenderer.shouldPresent(frame));
        assertFalse(StasisPreviewRenderer.isValidFrame(IntBuffer.allocate(10)));
    }

    @Test
    public void orderedSchemaRepresentsOppositePerGameLayerOrders() {
        int line = StasisPreviewRenderer.ORDER_LINE * StasisPreviewRenderer.ORDER_KIND_SCALE;
        int sprite = StasisPreviewRenderer.ORDER_SPRITE * StasisPreviewRenderer.ORDER_KIND_SCALE;
        int text = StasisPreviewRenderer.ORDER_TEXT * StasisPreviewRenderer.ORDER_KIND_SCALE;
        int rectangle = StasisPreviewRenderer.ORDER_RECT
                * StasisPreviewRenderer.ORDER_KIND_SCALE;

        int[] backgroundFirst = {sprite, rectangle, line, text};
        int[] overlayFirst = {line, text, rectangle, sprite};

        assertEquals(StasisPreviewRenderer.ORDER_SPRITE,
                StasisPreviewRenderer.orderKind(backgroundFirst[0]));
        assertEquals(StasisPreviewRenderer.ORDER_LINE,
                StasisPreviewRenderer.orderKind(overlayFirst[0]));
        assertEquals(StasisPreviewRenderer.ORDER_TEXT,
                StasisPreviewRenderer.orderKind(backgroundFirst[3]));
        assertEquals(StasisPreviewRenderer.ORDER_RECT,
                StasisPreviewRenderer.orderKind(backgroundFirst[1]));
        assertEquals(0, StasisPreviewRenderer.orderIndex(backgroundFirst[0]));
        assertEquals(-1, StasisPreviewRenderer.orderIndex(-1));
    }

    @Test
    public void countsAndActiveSpansAreBounded() {
        assertEquals(0, StasisPreviewRenderer.clampCount(-1, 8));
        assertEquals(5, StasisPreviewRenderer.clampCount(5, 8));
        assertEquals(8, StasisPreviewRenderer.clampCount(20, 8));
        assertEquals(StasisPreviewRenderer.MAX_SPRITES * StasisPreviewRenderer.SPRITE_I32_STRIDE,
                StasisPreviewRenderer.activeSpriteI32Count(Integer.MAX_VALUE));
        assertEquals(StasisPreviewRenderer.MAX_SPRITES * StasisPreviewRenderer.SPRITE_F32_STRIDE,
                StasisPreviewRenderer.activeSpriteF32Count(Integer.MAX_VALUE));
        assertEquals(StasisPreviewRenderer.MAX_TEXT * StasisPreviewRenderer.TEXT_I32_STRIDE,
                StasisPreviewRenderer.activeTextI32Count(Integer.MAX_VALUE));
        assertEquals(StasisPreviewRenderer.MAX_LINES * StasisPreviewRenderer.LINE_F32_STRIDE,
                StasisPreviewRenderer.activeLineF32Count(Integer.MAX_VALUE));
        assertEquals(8, StasisPreviewRenderer.activeRectF32Count(
                StasisPreviewRenderer.RENDER_VERSION,
                StasisPreviewRenderer.MAX_GEOMETRY - 1, Integer.MAX_VALUE));
        assertEquals(0, StasisPreviewRenderer.activeRectF32Count(
                StasisPreviewRenderer.RENDER_V3_VERSION, 0, 1));
        assertEquals(StasisPreviewRenderer.MAX_TEXT * StasisPreviewRenderer.TEXT_F32_STRIDE,
                StasisPreviewRenderer.activeTextF32Count(Integer.MAX_VALUE));
        assertEquals(StasisPreviewRenderer.TEXT_U8_CAPACITY,
                StasisPreviewRenderer.activeTextU8Count(Integer.MAX_VALUE));
    }

    @Test
    public void textSpanRequiresPayloadAndTrailingTerminator() {
        assertTrue(StasisPreviewRenderer.isValidTextSpan(0, 3, 4));
        assertTrue(StasisPreviewRenderer.isValidTextSpan(4, 0, 5));
        assertFalse(StasisPreviewRenderer.isValidTextSpan(0, 4, 4));
        assertFalse(StasisPreviewRenderer.isValidTextSpan(-1, 1, 4));
        assertFalse(StasisPreviewRenderer.isValidTextSpan(0, -1, 4));
        assertFalse(StasisPreviewRenderer.isValidTextSpan(4, 0, 4));
    }

    @Test
    public void packedTextTextureRejectsOutOfRangeDimensions() {
        long packed = StasisPreviewRenderer.packTexture(17, 640, 72);
        assertEquals(17, (int)packed);
        assertEquals(640, (int)((packed >>> 32) & 0xffffL));
        assertEquals(72, (int)((packed >>> 48) & 0xffffL));
        assertEquals(0L, StasisPreviewRenderer.packTexture(17, 65_536, 1));
        assertEquals(0L, StasisPreviewRenderer.packTexture(0, 1, 1));
    }

    @Test
    public void oddFractionalViewportMatchesNativeInputRounding() {
        StasisPreviewRenderer.DisplayViewport viewport =
                StasisPreviewRenderer.fitViewport(360, 720, 2400, 1081);
        assertEquals(929, viewport.x);
        assertEquals(0, viewport.y);
        assertEquals(541, viewport.width);
        assertEquals(1081, viewport.height);
        assertEquals(1081.0f / 720.0f, viewport.contentScale, 0.0001f);

        StasisPreviewRenderer.DisplayViewport vertical =
                StasisPreviewRenderer.fitViewport(360, 720, 1080, 2401);
        assertEquals(120, vertical.y);
        assertEquals(2160, vertical.height);
        assertEquals(121, 2401 - vertical.y - vertical.height);

        StasisPreviewRenderer.DisplayViewport narrow =
                StasisPreviewRenderer.fitViewport(800, 200, 1, 100);
        assertEquals(1, narrow.width);
        assertEquals(1, narrow.height);
        assertEquals(49, narrow.y);
    }

    @Test
    public void logicalSnapshotCopiesOnlyActiveProductionSpans() {
        StasisPreviewRenderer renderer = new StasisPreviewRenderer(
                new StasisPreviewRenderer.TextureProvider() {
                    @Override public void onResourceGenerationChanged(
                            int surfaceGeneration, int rendererGeneration,
                            boolean discardGpuHandles, String transitionReason) {}
                    @Override public int textureFor(int handle) { return 0; }
                }, ignored -> {});
        renderer.frameI32Bytes().asIntBuffer().put(0, StasisPreviewRenderer.RENDER_MAGIC);
        renderer.frameI32Bytes().asIntBuffer().put(1, StasisPreviewRenderer.RENDER_VERSION);
        renderer.frameI32Bytes().asIntBuffer().put(StasisPreviewRenderer.I_LINE_COUNT, 1);
        renderer.frameI32Bytes().asIntBuffer().put(StasisPreviewRenderer.I_RECT_COUNT, 1);
        renderer.frameI32Bytes().asIntBuffer().put(StasisPreviewRenderer.I_SPRITE_COUNT, 1);
        renderer.frameI32Bytes().asIntBuffer().put(StasisPreviewRenderer.I_TEXT_COUNT, 1);
        renderer.frameI32Bytes().asIntBuffer().put(StasisPreviewRenderer.I_TEXT_BYTES_USED, 4);
        renderer.frameI32Bytes().asIntBuffer().put(StasisPreviewRenderer.I_LOGICAL_W, 360);
        renderer.frameI32Bytes().asIntBuffer().put(StasisPreviewRenderer.I_DRAWABLE_H, 2400);
        renderer.frameI32Bytes().asIntBuffer().put(StasisPreviewRenderer.I_DENSITY_GENERATION, 7);
        renderer.frameI32Bytes().asIntBuffer().put(StasisPreviewRenderer.I_ORDER_COUNT, 1);
        renderer.frameI32Bytes().asIntBuffer().put(StasisPreviewRenderer.I_ORDER_BASE,
                StasisPreviewRenderer.ORDER_SPRITE * StasisPreviewRenderer.ORDER_KIND_SCALE);
        renderer.frameF32Bytes().asFloatBuffer().put(StasisPreviewRenderer.F_LINE_BASE, 12.5f);
        renderer.frameF32Bytes().asFloatBuffer().put(
                StasisPreviewRenderer.F_RECT_REVERSE_BASE, 33.5f);
        renderer.frameI32Bytes().asIntBuffer().put(StasisPreviewRenderer.I_SPRITE_BASE, 77);
        renderer.frameF32Bytes().asFloatBuffer().put(StasisPreviewRenderer.F_SPRITE_BASE, 19.25f);
        renderer.frameI32Bytes().asIntBuffer().put(StasisPreviewRenderer.I_TEXT_BASE, 5);
        renderer.frameF32Bytes().asFloatBuffer().put(StasisPreviewRenderer.F_TEXT_BASE, 42.0f);
        renderer.frameU8Bytes().put(0, (byte)'A');
        renderer.frameU8Bytes().put(1, (byte)'B');
        renderer.frameU8Bytes().put(2, (byte)'C');
        renderer.frameU8Bytes().put(3, (byte)0);

        StasisPreviewRenderer.LogicalFrameSnapshot snapshot = renderer.captureLogicalFrame();

        assertEquals(StasisPreviewRenderer.I_RECT_COUNT + 2, snapshot.header.length);
        assertEquals(360, snapshot.header[StasisPreviewRenderer.I_LOGICAL_W]);
        assertEquals(2400, snapshot.header[StasisPreviewRenderer.I_DRAWABLE_H]);
        assertEquals(7, snapshot.header[StasisPreviewRenderer.I_DENSITY_GENERATION]);
        assertEquals(StasisPreviewRenderer.LINE_F32_STRIDE, snapshot.lines.length);
        assertEquals(12.5f, snapshot.lines[0], 0.0f);
        assertEquals(StasisPreviewRenderer.GEOMETRY_F32_STRIDE, snapshot.rectangles.length);
        assertEquals(33.5f, snapshot.rectangles[0], 0.0f);
        assertEquals(StasisPreviewRenderer.SPRITE_I32_STRIDE, snapshot.sprites.length);
        assertEquals(77, snapshot.sprites[0]);
        assertEquals(StasisPreviewRenderer.SPRITE_F32_STRIDE, snapshot.spriteValues.length);
        assertEquals(19.25f, snapshot.spriteValues[0], 0.0f);
        assertEquals(StasisPreviewRenderer.TEXT_I32_STRIDE, snapshot.textMetadata.length);
        assertEquals(5, snapshot.textMetadata[0]);
        assertEquals(StasisPreviewRenderer.TEXT_F32_STRIDE, snapshot.textValues.length);
        assertEquals(42.0f, snapshot.textValues[0], 0.0f);
        assertEquals(4, snapshot.textBytes.length);
        assertEquals('C', snapshot.textBytes[2]);
        assertEquals(1, snapshot.order.length);
        assertEquals(StasisPreviewRenderer.ORDER_SPRITE,
                StasisPreviewRenderer.orderKind(snapshot.order[0]));
    }

    @Test
    public void contiguousHorizontalLinesCoalesceWithoutCrossingColorOrGeometryChanges() {
        FloatBuffer lines = FloatBuffer.allocate(
                StasisPreviewRenderer.F_LINE_BASE + 4 * StasisPreviewRenderer.LINE_F32_STRIDE);
        putLine(lines, 0, 10, 20, 30, 20, 1, 0, 0, 1);
        putLine(lines, 1, 10, 21, 30, 21, 1, 0, 0, 1);
        putLine(lines, 2, 10, 22, 30, 22, 0, 1, 0, 1);
        putLine(lines, 3, 10, 23, 31, 23, 1, 0, 0, 1);

        assertEquals(2, StasisPreviewRenderer.horizontalRunLength(lines, 0, 4));
        assertEquals(1, StasisPreviewRenderer.horizontalRunLength(lines, 2, 4));
        assertEquals(1, StasisPreviewRenderer.horizontalRunLength(lines, 3, 4));
    }

    private static void putLine(FloatBuffer lines, int index, float x1, float y1,
            float x2, float y2, float r, float g, float b, float a) {
        int base = StasisPreviewRenderer.F_LINE_BASE
                + index * StasisPreviewRenderer.LINE_F32_STRIDE;
        lines.put(base, x1);
        lines.put(base + 1, y1);
        lines.put(base + 2, x2);
        lines.put(base + 3, y2);
        lines.put(base + 4, r);
        lines.put(base + 5, g);
        lines.put(base + 6, b);
        lines.put(base + 7, a);
    }
}
