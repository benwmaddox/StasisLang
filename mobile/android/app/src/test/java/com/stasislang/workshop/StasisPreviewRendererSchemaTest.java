package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import java.nio.IntBuffer;
import java.nio.FloatBuffer;

import org.junit.Test;

public final class StasisPreviewRendererSchemaTest {
    @Test
    public void productionSchemaMatchesNativeContract() {
        assertEquals(28_704, StasisPreviewRenderer.I_TEXT_BASE);
        assertEquals(80_004, StasisPreviewRenderer.F_TEXT_BASE);
        assertEquals(34_848, StasisPreviewRenderer.FRAME_I32_CAPACITY);
        assertEquals(92_292, StasisPreviewRenderer.FRAME_F32_CAPACITY);
        assertEquals(65_536, StasisPreviewRenderer.TEXT_U8_CAPACITY);
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
        frame.put(1, 2);
        assertFalse(StasisPreviewRenderer.isValidFrame(frame));
        assertFalse(StasisPreviewRenderer.shouldPresent(frame));
        assertFalse(StasisPreviewRenderer.isValidFrame(IntBuffer.allocate(10)));
    }

    @Test
    public void countsAndActiveSpansAreBounded() {
        assertEquals(0, StasisPreviewRenderer.clampCount(-1, 8));
        assertEquals(5, StasisPreviewRenderer.clampCount(5, 8));
        assertEquals(8, StasisPreviewRenderer.clampCount(20, 8));
        assertEquals(StasisPreviewRenderer.MAX_SPRITES * StasisPreviewRenderer.SPRITE_I32_STRIDE,
                StasisPreviewRenderer.activeSpriteI32Count(Integer.MAX_VALUE));
        assertEquals(StasisPreviewRenderer.MAX_TEXT * StasisPreviewRenderer.TEXT_I32_STRIDE,
                StasisPreviewRenderer.activeTextI32Count(Integer.MAX_VALUE));
        assertEquals(StasisPreviewRenderer.MAX_LINES * StasisPreviewRenderer.LINE_F32_STRIDE,
                StasisPreviewRenderer.activeLineF32Count(Integer.MAX_VALUE));
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
    public void logicalSnapshotCopiesOnlyActiveProductionSpans() {
        StasisPreviewRenderer renderer = new StasisPreviewRenderer(
                new StasisPreviewRenderer.TextureProvider() {
                    @Override public void onSurfaceCreated() {}
                    @Override public int textureFor(int handle) { return 0; }
                }, ignored -> {});
        renderer.frameI32Bytes().asIntBuffer().put(0, StasisPreviewRenderer.RENDER_MAGIC);
        renderer.frameI32Bytes().asIntBuffer().put(1, StasisPreviewRenderer.RENDER_VERSION);
        renderer.frameI32Bytes().asIntBuffer().put(StasisPreviewRenderer.I_LINE_COUNT, 1);
        renderer.frameI32Bytes().asIntBuffer().put(StasisPreviewRenderer.I_SPRITE_COUNT, 1);
        renderer.frameI32Bytes().asIntBuffer().put(StasisPreviewRenderer.I_TEXT_COUNT, 1);
        renderer.frameI32Bytes().asIntBuffer().put(StasisPreviewRenderer.I_TEXT_BYTES_USED, 4);
        renderer.frameF32Bytes().asFloatBuffer().put(StasisPreviewRenderer.F_LINE_BASE, 12.5f);
        renderer.frameI32Bytes().asIntBuffer().put(StasisPreviewRenderer.I_SPRITE_BASE, 77);
        renderer.frameI32Bytes().asIntBuffer().put(StasisPreviewRenderer.I_TEXT_BASE, 5);
        renderer.frameF32Bytes().asFloatBuffer().put(StasisPreviewRenderer.F_TEXT_BASE, 42.0f);
        renderer.frameU8Bytes().put(0, (byte)'A');
        renderer.frameU8Bytes().put(1, (byte)'B');
        renderer.frameU8Bytes().put(2, (byte)'C');
        renderer.frameU8Bytes().put(3, (byte)0);

        StasisPreviewRenderer.LogicalFrameSnapshot snapshot = renderer.captureLogicalFrame();

        assertEquals(StasisPreviewRenderer.LINE_F32_STRIDE, snapshot.lines.length);
        assertEquals(12.5f, snapshot.lines[0], 0.0f);
        assertEquals(StasisPreviewRenderer.SPRITE_I32_STRIDE, snapshot.sprites.length);
        assertEquals(77, snapshot.sprites[0]);
        assertEquals(StasisPreviewRenderer.TEXT_I32_STRIDE, snapshot.textMetadata.length);
        assertEquals(5, snapshot.textMetadata[0]);
        assertEquals(StasisPreviewRenderer.TEXT_F32_STRIDE, snapshot.textValues.length);
        assertEquals(42.0f, snapshot.textValues[0], 0.0f);
        assertEquals(4, snapshot.textBytes.length);
        assertEquals('C', snapshot.textBytes[2]);
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
