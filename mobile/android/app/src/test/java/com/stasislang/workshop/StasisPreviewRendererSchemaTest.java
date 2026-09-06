package com.stasislang.workshop;

import android.opengl.GLES20;

import static com.stasislang.workshop.StasisPreviewRenderer.*;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import java.nio.IntBuffer;
import java.nio.FloatBuffer;
import java.nio.ByteBuffer;
import java.util.ArrayList;
import java.util.List;

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
        assertEquals(0, replaced[0].spriteRuns.length);
        assertEquals(0, replaced[0].clips.length);
    }

    @Test
    public void productionSchemaMatchesNativeContract() {
        assertEquals(12_320, StasisPreviewRenderer.I_TEXT_BASE);
        assertEquals(80_004, StasisPreviewRenderer.F_SPRITE_BASE);
        assertEquals(79_996, StasisPreviewRenderer.F_RECT_REVERSE_BASE);
        assertEquals(133_252, StasisPreviewRenderer.F_TEXT_BASE);
        assertEquals(18_464, StasisPreviewRenderer.I_SPRITE_RUN_BASE);
        assertEquals(51_232, StasisPreviewRenderer.I_ORDER_BASE);
        assertEquals(145_540, StasisPreviewRenderer.F_CLIP_BASE);
        assertEquals(256, StasisPreviewRenderer.MAX_CLIPS);
        assertEquals(4, StasisPreviewRenderer.CLIP_STRIDE_F32);
        assertEquals(67_888, StasisPreviewRenderer.FRAME_I32_CAPACITY);
        assertEquals(146_564, StasisPreviewRenderer.FRAME_F32_CAPACITY);
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
    public void spriteReleaseQueueIsOrderedBoundedAndAppliedThroughProvider() {
        List<Integer> released = new ArrayList<>();
        StasisPreviewRenderer.TextureProvider provider =
                new StasisPreviewRenderer.TextureProvider() {
                    @Override public void onResourceGenerationChanged(
                            int surfaceGeneration, int rendererGeneration,
                            boolean discardGpuHandles, String transitionReason) {}
                    @Override public int textureFor(int handle) { return 0; }
                    @Override public void releaseSprite(int handle) { released.add(handle); }
                };
        StasisPreviewRenderer renderer = new StasisPreviewRenderer(provider, ignored -> {});

        assertTrue(renderer.enqueuePendingSpriteReleases(
                "{\"status\":\"ok\",\"handles\":[-7,11,-13]}"));
        assertTrue(renderer.hasPendingSpriteReleases());
        renderer.applyPendingSpriteReleases();
        assertEquals(java.util.Arrays.asList(-7, 11, -13), released);
        assertFalse(renderer.hasPendingSpriteReleases());

        String tooLarge = releaseBatchJson(StasisPreviewRenderer.MAX_PENDING_SPRITE_RELEASES + 1);
        assertFalse(renderer.enqueuePendingSpriteReleases(tooLarge));
        assertFalse(renderer.hasPendingSpriteReleases());

        String full = releaseBatchJson(StasisPreviewRenderer.MAX_PENDING_SPRITE_RELEASES);
        assertTrue(renderer.enqueuePendingSpriteReleases(full));
        assertFalse(renderer.enqueuePendingSpriteReleases("{\"handles\":[99]}"));
        assertTrue(renderer.hasPendingSpriteReleases());
        renderer.applyPendingSpriteReleases();
        assertFalse(renderer.hasPendingSpriteReleases());
        assertEquals(3 + StasisPreviewRenderer.MAX_PENDING_SPRITE_RELEASES, released.size());
        assertEquals(-1, released.get(3).intValue());
        assertEquals(-StasisPreviewRenderer.MAX_PENDING_SPRITE_RELEASES,
                released.get(released.size() - 1).intValue());
    }

    @Test
    public void queuedReleaseIsCanceledBeforeGlApplicationWhenHandleReacquires() {
        List<Integer> released = new ArrayList<>();
        StasisPreviewRenderer.TextureProvider provider =
                new StasisPreviewRenderer.TextureProvider() {
                    @Override public void onResourceGenerationChanged(
                            int surfaceGeneration, int rendererGeneration,
                            boolean discardGpuHandles, String transitionReason) {}
                    @Override public int textureFor(int handle) { return 0; }
                    @Override public void releaseSprite(int handle) { released.add(handle); }
                };
        StasisPreviewRenderer renderer = new StasisPreviewRenderer(provider, ignored -> {});

        assertTrue(renderer.enqueuePendingSpriteReleases(
                "{\"handles\":[-17,23]}"));
        assertTrue(renderer.cancelPendingSpriteReleases(
                "{\"handles\":[-17]}"));
        assertTrue(renderer.hasPendingSpriteReleases());
        renderer.applyPendingSpriteReleases();
        assertEquals(java.util.Arrays.asList(23), released);
        assertFalse(renderer.hasPendingSpriteReleases());
        assertFalse(renderer.cancelPendingSpriteReleases(
                "{\"handles\":[-17]}"));
        assertTrue(renderer.enqueuePendingSpriteReleases("{\"handles\":[-17]}"));
        renderer.applyPendingSpriteReleases();
        assertEquals(java.util.Arrays.asList(23, -17), released);
    }

    @Test
    public void spriteReleasesWaitForPresentationButCanCleanUpWithoutAFrame() {
        List<Integer> released = new ArrayList<>();
        StasisPreviewRenderer.TextureProvider provider =
                new StasisPreviewRenderer.TextureProvider() {
                    @Override public void onResourceGenerationChanged(
                            int surfaceGeneration, int rendererGeneration,
                            boolean discardGpuHandles, String transitionReason) {}
                    @Override public int textureFor(int handle) { return 0; }
                    @Override public void releaseSprite(int handle) { released.add(handle); }
                };
        StasisPreviewRenderer renderer = new StasisPreviewRenderer(provider, ignored -> {});

        assertTrue(renderer.enqueuePendingSpriteReleases("{\"handles\":[-31]}"));
        renderer.finishPendingSpriteReleases(true, false);
        assertTrue(renderer.hasPendingSpriteReleases());
        assertTrue(released.isEmpty());
        renderer.finishPendingSpriteReleases(true, true);
        assertFalse(renderer.hasPendingSpriteReleases());
        assertEquals(java.util.Arrays.asList(-31), released);

        assertTrue(renderer.enqueuePendingSpriteReleases("{\"handles\":[47]}"));
        renderer.finishPendingSpriteReleases(false, false);
        assertFalse(renderer.hasPendingSpriteReleases());
        assertEquals(java.util.Arrays.asList(-31, 47), released);
    }

    private static String releaseBatchJson(int count) {
        StringBuilder json = new StringBuilder("{\"handles\":[");
        for (int index = 0; index < count; index += 1) {
            if (index != 0) json.append(',');
            json.append(-(index + 1));
        }
        return json.append("]}").toString();
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
        FloatBuffer floats = FloatBuffer.allocate(StasisPreviewRenderer.FRAME_F32_CAPACITY);
        assertFalse(StasisPreviewRenderer.isValidFrame(frame, floats));
        frame.put(0, StasisPreviewRenderer.RENDER_MAGIC);
        frame.put(1, StasisPreviewRenderer.RENDER_VERSION);
        assertTrue(StasisPreviewRenderer.isValidFrame(frame, floats));
        assertFalse(StasisPreviewRenderer.shouldPresent(frame, floats));
        frame.put(StasisPreviewRenderer.I_FLAGS, StasisPreviewRenderer.FLAG_PRESENT);
        assertTrue(StasisPreviewRenderer.shouldPresent(frame, floats));
        for (int version = 2; version < RENDER_VERSION; version += 1) {
            frame.put(1, version);
            assertFalse(StasisPreviewRenderer.isValidFrame(frame, floats));
            assertFalse(StasisPreviewRenderer.shouldPresent(frame, floats));
        }
        frame.put(1, 1);
        assertFalse(StasisPreviewRenderer.isValidFrame(frame, floats));
        assertFalse(StasisPreviewRenderer.shouldPresent(frame, floats));
        assertFalse(StasisPreviewRenderer.isValidFrame(IntBuffer.allocate(10), floats));
        assertFalse(StasisPreviewRenderer.isValidFrame(frame, FloatBuffer.allocate(10)));
        assertFalse(StasisPreviewRenderer.isValidFrame(frame, null));
    }

    @Test
    public void malformedCountsSpansAndOrderRejectAndRecover() {
        IntBuffer frame = validSpriteFrame();
        FloatBuffer floats = validSpriteFloats();
        frame.put(StasisPreviewRenderer.I_FLAGS, StasisPreviewRenderer.FLAG_PRESENT);
        int[] counts = {I_LINE_COUNT, I_SPRITE_COUNT, I_TEXT_COUNT, I_TEXT_BYTES_USED,
                I_RECT_COUNT, I_ORDER_COUNT, I_CLIP_COUNT, I_SPRITE_RUN_COUNT};
        int[] maxima = {MAX_LINES, MAX_SPRITES, MAX_TEXT, TEXT_U8_CAPACITY,
                MAX_GEOMETRY, MAX_ORDER, MAX_CLIPS, MAX_SPRITE_RUNS};
        for (int i = 0; i < counts.length; i++) {
            rejectAndRecover(frame, floats, counts[i], -1);
            rejectAndRecover(frame, floats, counts[i], maxima[i] + 1);
            rejectAndRecover(frame, floats, counts[i], Integer.MAX_VALUE);
        }
        rejectAndRecover(frame, floats, I_MAGIC, 0);
        rejectAndRecover(frame, floats, I_VERSION, 99);
        frame.put(I_LINE_COUNT, 1);
        rejectAndRecover(frame, floats, I_RECT_COUNT, MAX_GEOMETRY);
        frame.put(I_TEXT_COUNT, 1);
        frame.put(I_TEXT_BYTES_USED, 2);
        int text = StasisPreviewRenderer.I_TEXT_BASE;
        frame.put(text + 1, 1);
        rejectAndRecover(frame, floats, text + 2, 1);
        rejectAndRecover(frame, floats, text + 2, Integer.MAX_VALUE);
        rejectAndRecover(frame, floats, text + 2, -1);
        rejectAndRecover(frame, floats, text + 1, 2);
        rejectAndRecover(frame, floats, text + 1, Integer.MIN_VALUE);
        frame.put(text + 1, -9);
        assertTrue(StasisPreviewRenderer.shouldPresent(frame, floats));
        rejectAndRecover(frame, floats, text + 2, 1);
        frame.put(I_ORDER_COUNT, 1);
        int order = StasisPreviewRenderer.I_ORDER_BASE;
        frame.put(order, StasisPreviewRenderer.ORDER_LINE * ORDER_KIND_SCALE);
        for (int entry : new int[] {-1, 0, 7 * ORDER_KIND_SCALE, ORDER_KIND_SCALE + 1,
                2 * ORDER_KIND_SCALE + 1, 3 * ORDER_KIND_SCALE + 1, 4 * ORDER_KIND_SCALE,
                5 * ORDER_KIND_SCALE, 6 * ORDER_KIND_SCALE, 6 * ORDER_KIND_SCALE + 1}) {
            rejectAndRecover(frame, floats, order, entry);
        }
        frame.put(I_CLIP_COUNT, 1);
        frame.put(I_ORDER_COUNT, 2);
        frame.put(order, 5 * ORDER_KIND_SCALE);
        frame.put(order + 1, 6 * ORDER_KIND_SCALE);
        assertTrue(StasisPreviewRenderer.shouldPresent(frame, floats));
        rejectAndRecover(frame, floats, I_ORDER_COUNT, 1);
        rejectAndRecover(frame, floats, order, 6 * ORDER_KIND_SCALE);
        frame.put(I_ORDER_COUNT, 514);
        for (int i = 0; i < 257; i++) {
            frame.put(order + i, 5 * ORDER_KIND_SCALE);
            frame.put(order + 257 + i, 6 * ORDER_KIND_SCALE);
        }
        assertFalse(StasisPreviewRenderer.shouldPresent(frame, floats));
        frame.put(I_ORDER_COUNT, 0);
        assertTrue(StasisPreviewRenderer.shouldPresent(frame, floats));
        int run = I_SPRITE_RUN_BASE;
        frame.put(I_SPRITE_RUN_COUNT, 1);
        frame.put(run + 1, 1);
        frame.put(run + 2, -1);
        rejectAndRecover(frame, floats, run, -1);
        rejectAndRecover(frame, floats, run, Integer.MAX_VALUE);
        rejectAndRecover(frame, floats, run + 1, 0);
        rejectAndRecover(frame, floats, run + 1, Integer.MAX_VALUE);
        rejectAndRecover(frame, floats, run + 2, -2);
        rejectAndRecover(frame, floats, run + 2, 1);
        for (int field = 3; field < SPRITE_RUN_I32_STRIDE; field++) {
            rejectAndRecover(frame, floats, run + field, 1);
        }
        rejectAndRecover(frame, floats, I_SPRITE_BASE, 0);
        rejectAndRecover(frame, floats, I_SPRITE_BASE + 2, 1);
        assertFalse(isValidFrame(null, floats));
        frame.limit(FRAME_I32_CAPACITY - 1);
        assertFalse(isValidFrame(frame, floats));
        frame.clear();
        floats.limit(FRAME_F32_CAPACITY - 1);
        assertFalse(isValidFrame(frame, floats));
        floats.clear();
        assertTrue(shouldPresent(frame, floats));
    }

    @Test
    public void rejectedDrawDoesNotPrepareResourcesOrConsumeCapture() {
        int[] timings = {0};
        StasisPreviewRenderer renderer = new StasisPreviewRenderer(
                new StasisPreviewRenderer.TextureProvider() {
                    @Override public void onResourceGenerationChanged(int surface, int generation,
                            boolean discard, String reason) { throw new AssertionError(); }
                    @Override public int textureFor(int handle) { throw new AssertionError(); }
                    @Override public void beginRestoreAttempt() { throw new AssertionError(); }
                }, ignored -> timings[0]++);
        int[] captures = {0};
        renderer.requestCapture((bitmap, error, snapshot) -> captures[0]++);
        IntBuffer frame = renderer.frameI32Bytes().asIntBuffer();
        frame.put(I_MAGIC, RENDER_MAGIC);
        frame.put(I_VERSION, RENDER_VERSION);
        frame.put(I_FLAGS, FLAG_CLEAR | FLAG_PRESENT);
        // Each IT-009 malformed category must exit before calling GLES, whose
        // host-test methods throw, even when clear and present are requested.
        int[][] malformed = {{I_MAGIC, 0}, {I_VERSION, 99},
                {I_LINE_COUNT, -1}, {I_LINE_COUNT, MAX_LINES + 1},
                {I_TEXT_COUNT, 1}, {I_ORDER_COUNT, 1}};
        for (int[] mutation : malformed) {
            int saved = frame.get(mutation[0]);
            frame.put(mutation[0], mutation[1]);
            renderer.onDrawFrame(null);
            frame.put(mutation[0], saved);
            assertTrue(shouldPresent(frame, renderer.frameF32Bytes().asFloatBuffer()));
        }
        assertEquals(malformed.length, timings[0]);
        assertEquals(0, captures[0]);
        assertEquals(0, renderer.rendererGeneration());
        assertTrue(renderer.awaitPresentedFrameToken(-1, 0));
        assertFalse(renderer.awaitPresentedFrameToken(0, 0));
        frame.put(I_LINE_COUNT, 0);
        assertTrue(shouldPresent(frame, renderer.frameF32Bytes().asFloatBuffer()));
        // Replacing the request proves rejection retained the pending callback.
        renderer.requestCapture((bitmap, error, snapshot) -> {});
        assertEquals(1, captures[0]);
    }

    private static void rejectAndRecover(IntBuffer frame, FloatBuffer floats,
            int index, int badValue) {
        assertTrue(StasisPreviewRenderer.shouldPresent(frame, floats));
        int saved = frame.get(index);
        frame.put(index, badValue);
        assertFalse("must reject slot " + index + " value " + badValue,
                StasisPreviewRenderer.shouldPresent(frame, floats));
        frame.put(index, saved);
        assertTrue(StasisPreviewRenderer.shouldPresent(frame, floats));
    }

    @Test
    public void spriteGeometryValidationMatchesNativeV7Contract() {
        IntBuffer frame = validSpriteFrame();
        FloatBuffer floats = validSpriteFloats();
        assertTrue(StasisPreviewRenderer.isValidFrame(frame, floats));

        int base = StasisPreviewRenderer.F_SPRITE_BASE;
        int[] finiteFields = {0, 1, 8, 9, 12};
        for (int field : finiteFields) {
            float saved = floats.get(base + field);
            floats.put(base + field, Float.NaN);
            assertFalse(StasisPreviewRenderer.isValidFrame(frame, floats));
            floats.put(base + field, Float.POSITIVE_INFINITY);
            assertFalse(StasisPreviewRenderer.isValidFrame(frame, floats));
            floats.put(base + field, saved);
        }

        floats.put(base + 2, 0.0f);
        assertFalse(StasisPreviewRenderer.isValidFrame(frame, floats));
        floats.put(base + 2, 16.0f);
        floats.put(base + 3, -1.0f);
        assertFalse(StasisPreviewRenderer.isValidFrame(frame, floats));
        floats.put(base + 3, 12.0f);

        floats.put(base + 10, 0.0f);
        assertFalse(StasisPreviewRenderer.isValidFrame(frame, floats));
        floats.put(base + 10, 1.0f);
        floats.put(base + 11, Float.NEGATIVE_INFINITY);
        assertFalse(StasisPreviewRenderer.isValidFrame(frame, floats));
    }

    @Test
    public void sourceCropMustBeDefaultOrACompletePositiveRectangle() {
        IntBuffer frame = validSpriteFrame();
        FloatBuffer floats = validSpriteFloats();
        int base = StasisPreviewRenderer.F_SPRITE_BASE;

        assertTrue(StasisPreviewRenderer.isValidFrame(frame, floats));
        floats.put(base + 4, 3.0f);
        floats.put(base + 5, 4.0f);
        assertTrue(StasisPreviewRenderer.isValidFrame(frame, floats));

        floats.put(base + 6, 5.0f);
        assertFalse(StasisPreviewRenderer.isValidFrame(frame, floats));
        floats.put(base + 7, 6.0f);
        assertTrue(StasisPreviewRenderer.isValidFrame(frame, floats));
        floats.put(base + 6, 0.0f);
        assertFalse(StasisPreviewRenderer.isValidFrame(frame, floats));
        floats.put(base + 6, -1.0f);
        assertFalse(StasisPreviewRenderer.isValidFrame(frame, floats));
        floats.put(base + 6, 5.0f);
        floats.put(base + 4, -1.0f);
        assertFalse(StasisPreviewRenderer.isValidFrame(frame, floats));
    }

    @Test
    public void rejectedResourceCropAppendsNoQuad() throws Exception {
        StasisPreviewRenderer renderer = new StasisPreviewRenderer(
                new StasisPreviewRenderer.TextureProvider() {
                    @Override public void onResourceGenerationChanged(
                            int surfaceGeneration, int rendererGeneration,
                            boolean discardGpuHandles, String transitionReason) {}
                    @Override public int textureFor(int handle) { return 1; }
                }, ignored -> {});
        IntBuffer frame = renderer.frameI32Bytes().asIntBuffer();
        FloatBuffer floats = renderer.frameF32Bytes().asFloatBuffer();
        putValidSprite(frame, floats);
        int base = StasisPreviewRenderer.F_SPRITE_BASE;
        floats.put(base + 4, 9.0f);
        floats.put(base + 5, 0.0f);
        floats.put(base + 6, 2.0f);
        floats.put(base + 7, 2.0f);

        java.lang.reflect.Field widths = StasisPreviewRenderer.class
                .getDeclaredField("frameSpriteWidths");
        java.lang.reflect.Field heights = StasisPreviewRenderer.class
                .getDeclaredField("frameSpriteHeights");
        widths.setAccessible(true);
        heights.setAccessible(true);
        ((int[])widths.get(renderer))[0] = 10;
        ((int[])heights.get(renderer))[0] = 10;

        assertFalse(renderer.appendSprite(StasisPreviewRenderer.I_SPRITE_BASE));
    }

    private static IntBuffer validSpriteFrame() {
        IntBuffer frame = IntBuffer.allocate(StasisPreviewRenderer.FRAME_I32_CAPACITY);
        FloatBuffer ignored = FloatBuffer.allocate(StasisPreviewRenderer.FRAME_F32_CAPACITY);
        putValidSprite(frame, ignored);
        return frame;
    }

    private static FloatBuffer validSpriteFloats() {
        IntBuffer ignored = IntBuffer.allocate(StasisPreviewRenderer.FRAME_I32_CAPACITY);
        FloatBuffer floats = FloatBuffer.allocate(StasisPreviewRenderer.FRAME_F32_CAPACITY);
        putValidSprite(ignored, floats);
        return floats;
    }

    private static void putValidSprite(IntBuffer frame, FloatBuffer floats) {
        frame.put(StasisPreviewRenderer.I_MAGIC, StasisPreviewRenderer.RENDER_MAGIC);
        frame.put(StasisPreviewRenderer.I_VERSION, StasisPreviewRenderer.RENDER_VERSION);
        frame.put(StasisPreviewRenderer.I_SPRITE_COUNT, 1);
        frame.put(StasisPreviewRenderer.I_SPRITE_BASE, 17);
        int base = StasisPreviewRenderer.F_SPRITE_BASE;
        floats.put(base, 10.0f);
        floats.put(base + 1, 20.0f);
        floats.put(base + 2, 16.0f);
        floats.put(base + 3, 12.0f);
        floats.put(base + 8, 8.0f);
        floats.put(base + 9, 6.0f);
        floats.put(base + 10, 1.0f);
        floats.put(base + 11, 1.0f);
        floats.put(base + 12, 15.0f);
    }

    @Test
    public void orderedSchemaRepresentsOppositePerGameLayerOrders() {
        int line = StasisPreviewRenderer.ORDER_LINE * StasisPreviewRenderer.ORDER_KIND_SCALE;
        int sprite = StasisPreviewRenderer.ORDER_SPRITE * StasisPreviewRenderer.ORDER_KIND_SCALE;
        int text = StasisPreviewRenderer.ORDER_TEXT * StasisPreviewRenderer.ORDER_KIND_SCALE;
        int rectangle = StasisPreviewRenderer.ORDER_RECT
                * StasisPreviewRenderer.ORDER_KIND_SCALE;
        int clipPush = StasisPreviewRenderer.ORDER_CLIP_PUSH
                * StasisPreviewRenderer.ORDER_KIND_SCALE;
        int clipPop = StasisPreviewRenderer.ORDER_CLIP_POP
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
        assertEquals(StasisPreviewRenderer.ORDER_CLIP_PUSH,
                StasisPreviewRenderer.orderKind(clipPush + 2));
        assertEquals(StasisPreviewRenderer.ORDER_CLIP_POP,
                StasisPreviewRenderer.orderKind(clipPop));
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
                StasisPreviewRenderer.MAX_GEOMETRY - 1, Integer.MAX_VALUE));
        assertEquals(8, StasisPreviewRenderer.activeRectF32Count(0, 1));
        assertEquals(3, StasisPreviewRenderer.clampedClipCount(3));
        assertEquals(StasisPreviewRenderer.MAX_CLIPS, StasisPreviewRenderer.clampedClipCount(
                Integer.MAX_VALUE));
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
        renderer.frameI32Bytes().asIntBuffer().put(StasisPreviewRenderer.I_SPRITE_RUN_COUNT, 1);
        renderer.frameI32Bytes().asIntBuffer().put(StasisPreviewRenderer.I_TEXT_COUNT, 1);
        renderer.frameI32Bytes().asIntBuffer().put(StasisPreviewRenderer.I_TEXT_BYTES_USED, 4);
        renderer.frameI32Bytes().asIntBuffer().put(StasisPreviewRenderer.I_CLIP_COUNT, 1);
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
        renderer.frameI32Bytes().asIntBuffer().put(StasisPreviewRenderer.I_SPRITE_RUN_BASE, 0);
        renderer.frameI32Bytes().asIntBuffer().put(StasisPreviewRenderer.I_SPRITE_RUN_BASE + 1, 1);
        renderer.frameI32Bytes().asIntBuffer().put(StasisPreviewRenderer.I_SPRITE_RUN_BASE + 2, -1);
        renderer.frameF32Bytes().asFloatBuffer().put(StasisPreviewRenderer.F_SPRITE_BASE, 19.25f);
        renderer.frameF32Bytes().asFloatBuffer().put(
                StasisPreviewRenderer.F_SPRITE_BASE + 4, 0.25f);
        renderer.frameI32Bytes().asIntBuffer().put(StasisPreviewRenderer.I_TEXT_BASE, 5);
        renderer.frameF32Bytes().asFloatBuffer().put(StasisPreviewRenderer.F_TEXT_BASE, 42.0f);
        renderer.frameF32Bytes().asFloatBuffer().put(StasisPreviewRenderer.F_CLIP_BASE, 8.0f);
        renderer.frameF32Bytes().asFloatBuffer().put(StasisPreviewRenderer.F_CLIP_BASE + 1, 9.0f);
        renderer.frameF32Bytes().asFloatBuffer().put(StasisPreviewRenderer.F_CLIP_BASE + 2, 70.0f);
        renderer.frameF32Bytes().asFloatBuffer().put(StasisPreviewRenderer.F_CLIP_BASE + 3, 40.0f);
        renderer.frameU8Bytes().put(0, (byte)'A');
        renderer.frameU8Bytes().put(1, (byte)'B');
        renderer.frameU8Bytes().put(2, (byte)'C');
        renderer.frameU8Bytes().put(3, (byte)0);

        StasisPreviewRenderer.LogicalFrameSnapshot snapshot = renderer.captureLogicalFrame();

        assertEquals(StasisPreviewRenderer.I_SPRITE_RUN_COUNT + 2, snapshot.header.length);
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
        assertEquals(0.25f, snapshot.spriteValues[4], 0.0f);
        assertEquals(StasisPreviewRenderer.SPRITE_RUN_I32_STRIDE, snapshot.spriteRuns.length);
        assertEquals(1, snapshot.spriteRuns[1]);
        assertEquals(StasisPreviewRenderer.TEXT_I32_STRIDE, snapshot.textMetadata.length);
        assertEquals(5, snapshot.textMetadata[0]);
        assertEquals(StasisPreviewRenderer.TEXT_F32_STRIDE, snapshot.textValues.length);
        assertEquals(42.0f, snapshot.textValues[0], 0.0f);
        assertEquals(4, snapshot.textBytes.length);
        assertEquals('C', snapshot.textBytes[2]);
        assertEquals(1, snapshot.order.length);
        assertEquals(StasisPreviewRenderer.ORDER_SPRITE,
                StasisPreviewRenderer.orderKind(snapshot.order[0]));
        assertEquals(4, snapshot.clips.length);
        assertEquals(8.0f, snapshot.clips[0], 0.0f);
        assertEquals(40.0f, snapshot.clips[3], 0.0f);
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

    @Test
    public void performanceSamplesExcludeWarmupAndUseNearestRankPercentiles() {
        StasisPreviewRenderer.FramePerformanceSamples samples =
                new StasisPreviewRenderer.FramePerformanceSamples(2, 4);
        assertNull(samples.add(99_000, 90_000, 9_000, 9, 1, 2, 3, 4, 5));
        assertNull(samples.add(98_000, 80_000, 8_000, 8, 1, 2, 3, 4, 5));
        assertNull(samples.add(10_000, 4_000, 5_000, 3, 1, 2, 3, 4, 5));
        assertNull(samples.add(20_000, 8_000, 10_000, 4, 1, 2, 3, 4, 5));
        assertNull(samples.add(30_000, 12_000, 15_000, 5, 1, 2, 3, 4, 5));

        String report = samples.add(40_000, 16_000, 20_000, 6, 1, 2, 3, 4, 5);

        assertTrue(report.contains("warmup=2 samples=4"));
        assertTrue(report.contains("total_p50_us=20 total_p95_us=40"));
        assertTrue(report.contains("resource_p50_us=8 resource_p95_us=16"));
        assertTrue(report.contains("draw_p50_us=10 draw_p95_us=20"));
        assertTrue(report.contains("draw_calls_min=3 draw_calls_max=6"));
        assertTrue(report.endsWith("lines=1 rects=2 sprites=3 text=4 order=5"));
        assertNull(samples.add(1, 1, 1, 1, 0, 0, 0, 0, 0));
    }

    @Test
    public void frameResourcesResolveOnceBeforeOrderedSubmission() {
        IntBuffer frame = IntBuffer.allocate(StasisPreviewRenderer.FRAME_I32_CAPACITY);
        ByteBuffer textBytes = ByteBuffer.allocate(StasisPreviewRenderer.TEXT_U8_CAPACITY);
        frame.put(StasisPreviewRenderer.I_SPRITE_COUNT, 2);
        frame.put(StasisPreviewRenderer.I_SPRITE_BASE, 7);
        frame.put(StasisPreviewRenderer.I_SPRITE_BASE + StasisPreviewRenderer.SPRITE_I32_STRIDE, 0);
        frame.put(StasisPreviewRenderer.I_TEXT_COUNT, 2);
        frame.put(StasisPreviewRenderer.I_TEXT_BYTES_USED, 4);
        frame.put(StasisPreviewRenderer.I_TEXT_BASE, 11);
        frame.put(StasisPreviewRenderer.I_TEXT_BASE + 1, 0);
        frame.put(StasisPreviewRenderer.I_TEXT_BASE + 2, 3);
        frame.put(StasisPreviewRenderer.I_TEXT_BASE + StasisPreviewRenderer.TEXT_I32_STRIDE, 11);
        frame.put(StasisPreviewRenderer.I_TEXT_BASE + StasisPreviewRenderer.TEXT_I32_STRIDE + 1, -9);
        int[] calls = new int[4];
        StasisPreviewRenderer.TextureProvider provider = new StasisPreviewRenderer.TextureProvider() {
            @Override public void onResourceGenerationChanged(int surfaceGeneration,
                    int rendererGeneration, boolean discardGpuHandles, String transitionReason) {}
            @Override public int textureFor(int handle) {
                calls[0] += 1;
                return handle == 0 ? 0 : 70;
            }
            @Override public int fallbackTexture() { return 99; }
            @Override public int filterFor(int handle) {
                calls[1] += 1;
                return 100 + handle;
            }
            @Override public long textTextureFor(
                    int font, ByteBuffer utf8, int offset, int length) {
                calls[2] += 1;
                return StasisPreviewRenderer.packTexture(12, 30, 10);
            }
            @Override public long cachedTextTextureFor(int runHandle) {
                calls[3] += 1;
                return StasisPreviewRenderer.packTexture(13, 31, 11);
            }
        };
        int[] textures = new int[StasisPreviewRenderer.MAX_SPRITES];
        int[] filters = new int[StasisPreviewRenderer.MAX_SPRITES];
        long[] textTextures = new long[StasisPreviewRenderer.MAX_TEXT];

        StasisPreviewRenderer.resolveFrameResources(
                provider, frame, textBytes, textures, filters, textTextures);

        assertEquals(2, calls[0]);
        assertEquals(2, calls[1]);
        assertEquals(1, calls[2]);
        assertEquals(1, calls[3]);
        assertEquals(70, textures[0]);
        assertEquals(99, textures[1]);
        assertEquals(107, filters[0]);
        assertEquals(100, filters[1]);
        assertEquals(12, (int)textTextures[0]);
        assertEquals(13, (int)textTextures[1]);
    }

    @Test
    public void frameResourcesAggregateEveryUseBeforeResolvingAHandle() {
        IntBuffer frame = IntBuffer.allocate(StasisPreviewRenderer.FRAME_I32_CAPACITY);
        FloatBuffer values = FloatBuffer.allocate(StasisPreviewRenderer.FRAME_F32_CAPACITY);
        frame.put(StasisPreviewRenderer.I_SPRITE_COUNT, 2);
        frame.put(StasisPreviewRenderer.I_SPRITE_BASE, 7);
        frame.put(StasisPreviewRenderer.I_SPRITE_BASE + StasisPreviewRenderer.SPRITE_I32_STRIDE, 7);
        int first = StasisPreviewRenderer.F_SPRITE_BASE;
        int second = first + StasisPreviewRenderer.SPRITE_F32_STRIDE;
        values.put(first + 2, 20.0f); values.put(first + 3, 10.0f);
        values.put(first + 10, 1.0f); values.put(first + 11, 1.0f);
        values.put(second + 2, 30.0f); values.put(second + 3, 10.0f);
        values.put(second + 6, 5.0f); values.put(second + 7, 5.0f);
        values.put(second + 10, -2.0f); values.put(second + 11, 3.0f);
        final int[] calls = {0};
        final AndroidRasterPlan.Requirement[] captured = {null};
        StasisPreviewRenderer.TextureProvider provider = new StasisPreviewRenderer.TextureProvider() {
            @Override public void onResourceGenerationChanged(int surface, int renderer,
                    boolean discard, String reason) {}
            @Override public int textureFor(int handle) { throw new AssertionError(); }
            @Override public int textureFor(int handle, AndroidRasterPlan.Requirement requirement) {
                calls[0] += 1;
                captured[0] = requirement;
                return 8;
            }
        };
        int[] textures = new int[StasisPreviewRenderer.MAX_SPRITES];
        int[] filters = new int[StasisPreviewRenderer.MAX_SPRITES];
        long[] text = new long[StasisPreviewRenderer.MAX_TEXT];

        StasisPreviewRenderer.resolveFrameResources(provider, frame, values,
                ByteBuffer.allocate(StasisPreviewRenderer.TEXT_U8_CAPACITY), textures, filters,
                new int[StasisPreviewRenderer.MAX_SPRITES],
                new int[StasisPreviewRenderer.MAX_SPRITES],
                new float[StasisPreviewRenderer.MAX_SPRITES],
                new float[StasisPreviewRenderer.MAX_SPRITES],
                new float[StasisPreviewRenderer.MAX_SPRITES],
                new float[StasisPreviewRenderer.MAX_SPRITES], text);

        assertEquals(1, calls[0]);
        AndroidRasterPlan.Result plan = AndroidRasterPlan.exact(
                100, 50, captured[0], 2.0f, 8192);
        assertEquals(2400, plan.width);
        assertEquals(1200, plan.height);
        assertEquals(8, textures[0]);
        assertEquals(8, textures[1]);
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
