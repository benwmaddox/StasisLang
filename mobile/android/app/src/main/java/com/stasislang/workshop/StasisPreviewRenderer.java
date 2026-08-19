package com.stasislang.workshop;

import android.graphics.Bitmap;
import android.opengl.GLES20;
import android.opengl.GLSurfaceView;
import android.util.Log;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.FloatBuffer;
import java.nio.IntBuffer;
import java.util.Arrays;
import java.util.ArrayDeque;

import org.json.JSONArray;
import org.json.JSONObject;

final class StasisPreviewRenderer implements GLSurfaceView.Renderer {
    private static final String LOG_TAG = "StasisRenderer";
    static final int RENDER_MAGIC = 0x47584631;
    static final int RENDER_V2_VERSION = 2;
    static final int RENDER_V3_VERSION = 3;
    static final int RENDER_V4_VERSION = 4;
    static final int RENDER_VERSION = 5;
    static final int FLAG_CLEAR = 1;
    static final int FLAG_PRESENT = 2;

    static final int I_MAGIC = 0;
    static final int I_VERSION = 1;
    static final int I_FLAGS = 2;
    static final int I_LINE_COUNT = 3;
    static final int I_SPRITE_COUNT = 4;
    static final int I_DROPPED_LINES = 5;
    static final int I_DROPPED_SPRITES = 6;
    static final int I_TEXT_COUNT = 7;
    static final int I_DROPPED_TEXT = 8;
    static final int I_TEXT_BYTES_USED = 9;
    static final int I_LOGICAL_W = 10;
    static final int I_LOGICAL_H = 11;
    static final int I_NATIVE_W = 12;
    static final int I_NATIVE_H = 13;
    static final int I_DRAWABLE_W = 14;
    static final int I_DRAWABLE_H = 15;
    static final int I_SAFE_X = 16;
    static final int I_SAFE_Y = 17;
    static final int I_SAFE_W = 18;
    static final int I_SAFE_H = 19;
    static final int I_DISPLAY_GENERATION = 20;
    static final int I_DENSITY_GENERATION = 21;
    static final int I_ORDER_COUNT = 22;
    static final int I_DROPPED_ORDER = 23;
    static final int I_RECT_COUNT = 24;
    static final int I_DROPPED_RECTS = 25;
    static final int I_FRAME_TOKEN = 26;
    static final int I_SPRITE_BASE = 32;
    static final int F_LINE_BASE = 4;
    static final int MAX_GEOMETRY = 10_000;
    static final int GEOMETRY_F32_STRIDE = 8;
    static final int MAX_LINES = MAX_GEOMETRY;
    static final int LINE_F32_STRIDE = GEOMETRY_F32_STRIDE;
    static final int MAX_SPRITES = 4_096;
    static final int SPRITE_I32_STRIDE = 3;
    static final int LEGACY_SPRITE_F32_STRIDE = 4;
    static final int SPRITE_F32_STRIDE = 8;
    static final int MAX_TEXT = 2_048;
    static final int TEXT_I32_STRIDE = 3;
    static final int TEXT_F32_STRIDE = 6;
    static final int TEXT_U8_CAPACITY = 65_536;
    static final int MAX_ORDER = MAX_LINES + MAX_SPRITES + MAX_TEXT;
    static final int ORDER_KIND_SCALE = 16_384;
    static final int ORDER_LINE = 1;
    static final int ORDER_SPRITE = 2;
    static final int ORDER_TEXT = 3;
    static final int ORDER_RECT = 4;
    static final int I_TEXT_BASE = I_SPRITE_BASE + MAX_SPRITES * SPRITE_I32_STRIDE;
    static final int F_SPRITE_BASE = F_LINE_BASE + MAX_LINES * LINE_F32_STRIDE;
    static final int F_RECT_REVERSE_BASE = F_SPRITE_BASE - GEOMETRY_F32_STRIDE;
    static final int LEGACY_F_TEXT_BASE = F_SPRITE_BASE + MAX_SPRITES * LEGACY_SPRITE_F32_STRIDE;
    static final int F_TEXT_BASE = F_SPRITE_BASE + MAX_SPRITES * SPRITE_F32_STRIDE;
    static final int I_ORDER_BASE = I_TEXT_BASE + MAX_TEXT * TEXT_I32_STRIDE;
    static final int FRAME_I32_CAPACITY = I_ORDER_BASE + MAX_ORDER;
    static final int FRAME_F32_CAPACITY = F_TEXT_BASE + MAX_TEXT * TEXT_F32_STRIDE;

    // The production host header intentionally stops at density generation;
    // the frame token is read through frameToken() so this ABI stays 22 ints.
    private static final int HOST_HEADER_I32S = I_DENSITY_GENERATION + 1;
    private static final int CAPTURE_HEADER_I32S = I_RECT_COUNT + 2;
    private static final int LINE_CHUNK_SIZE = 256;
    private static final int SPRITE_CHUNK_SIZE = 128;
    private static final int VERTICES_PER_QUAD = 6;
    private static final int COLOR_VERTEX_FLOATS = 6;
    private static final int TEXTURE_VERTEX_FLOATS = 8;
    private static final int COLOR_VERTEX_BYTES = COLOR_VERTEX_FLOATS * 4;
    private static final int TEXTURE_VERTEX_BYTES = TEXTURE_VERTEX_FLOATS * 4;
    private static final int PIPELINE_NONE = 0;
    private static final int PIPELINE_COLOR = 1;
    private static final int PIPELINE_TEXTURE = 2;
    private static final int MAX_CAPTURE_PIXELS = 8_000_000;
    private static final long MIN_RESTORE_LABEL_NANOS = 250_000_000L;
    private static final int PERFORMANCE_WARMUP_FRAMES = 60;
    private static final int PERFORMANCE_SAMPLE_FRAMES = 180;
    private static final String[] RESTORE_LABEL = {
            "   01110 11111 01110 01110 11111 01110   ",
            "   10001 00100 10001 10001 00100 10001   ",
            "   10000 00100 10001 10000 00100 10000   ",
            "   01110 00100 11111 01110 00100 01110   ",
            "   00001 00100 10001 00001 00100 00001   ",
            "   10001 00100 10001 10001 00100 10001   ",
            "   01110 00100 10001 01110 11111 01110   ",
            "                                         ",
            "10000 01110 01110 11110 11111 10001 01110",
            "10000 10001 10001 10001 00100 11001 10001",
            "10000 10001 10001 10001 00100 10101 10000",
            "10000 10001 11111 10001 00100 10011 10111",
            "10000 10001 10001 10001 00100 10001 10001",
            "10000 10001 10001 10001 00100 10001 10001",
            "11111 01110 10001 11110 11111 10001 01110"
    };

    interface TextureProvider {
        void onResourceGenerationChanged(int surfaceGeneration, int rendererGeneration,
                boolean discardGpuHandles, String transitionReason);

        default void beginRestoreAttempt() {}

        default String consumeFailure() { return null; }

        default boolean isRestoreComplete() { return true; }

        default void onFrameStart() {}

        default void onDisplayMetricsChanged(float rasterScale, int densityGeneration) {}

        int textureFor(int handle);

        default void releaseSprite(int handle) {}

        default int fallbackTexture() {
            return 0;
        }

        default int filterFor(int handle) {
            return GLES20.GL_LINEAR;
        }

        // Packed as texture:u32, width:u16, height:u16. Zero means unavailable.
        default long textTextureFor(int font, ByteBuffer utf8, int offset, int length) {
            return 0L;
        }

        default long cachedTextTextureFor(int runHandle) {
            return 0L;
        }
    }

    interface TimingListener {
        void onRendered(long durationNanos);
    }

    interface CaptureCallback {
        void onCaptured(Bitmap bitmap, String error, LogicalFrameSnapshot capturedFrame);
    }

    static final class LogicalFrameSnapshot {
        final int[] header;
        final float[] lines;
        final float[] rectangles;
        final int[] sprites;
        final float[] spriteValues;
        final int[] textMetadata;
        final float[] textValues;
        final byte[] textBytes;
        final int[] order;

        LogicalFrameSnapshot(int[] header, float[] lines, float[] rectangles,
                int[] sprites, float[] spriteValues,
                int[] textMetadata, float[] textValues, byte[] textBytes, int[] order) {
            this.header = header;
            this.lines = lines;
            this.rectangles = rectangles;
            this.sprites = sprites;
            this.spriteValues = spriteValues;
            this.textMetadata = textMetadata;
            this.textValues = textValues;
            this.textBytes = textBytes;
            this.order = order;
        }
    }

    static final class DisplayViewport {
        final int x;
        final int y;
        final int width;
        final int height;
        final float contentScale;
        final float rasterScale;

        DisplayViewport(int x, int y, int width, int height,
                float contentScale, float rasterScale) {
            this.x = x;
            this.y = y;
            this.width = width;
            this.height = height;
            this.contentScale = contentScale;
            this.rasterScale = rasterScale;
        }
    }

    static final class FramePerformanceSamples {
        private final int warmupFrames;
        private final long[] totalNanos;
        private final long[] resourceNanos;
        private final long[] drawNanos;
        private int seenFrames;
        private int sampleCount;
        private int minimumDrawCalls = Integer.MAX_VALUE;
        private int maximumDrawCalls;
        private boolean reported;

        FramePerformanceSamples(int warmupFrames, int sampleFrames) {
            if (warmupFrames < 0 || sampleFrames <= 0) {
                throw new IllegalArgumentException("render performance sample bounds are invalid");
            }
            this.warmupFrames = warmupFrames;
            totalNanos = new long[sampleFrames];
            resourceNanos = new long[sampleFrames];
            drawNanos = new long[sampleFrames];
        }

        String add(long total, long resources, long draw, int drawCalls,
                int lines, int rectangles, int sprites, int text, int order) {
            if (reported) return null;
            seenFrames += 1;
            if (seenFrames <= warmupFrames) return null;
            totalNanos[sampleCount] = Math.max(0L, total);
            resourceNanos[sampleCount] = Math.max(0L, resources);
            drawNanos[sampleCount] = Math.max(0L, draw);
            minimumDrawCalls = Math.min(minimumDrawCalls, drawCalls);
            maximumDrawCalls = Math.max(maximumDrawCalls, drawCalls);
            sampleCount += 1;
            if (sampleCount < totalNanos.length) return null;
            reported = true;
            return "RenderPerformance: schema=1 warmup=" + warmupFrames
                    + " samples=" + sampleCount
                    + " total_p50_us=" + percentileMicros(totalNanos, 50)
                    + " total_p95_us=" + percentileMicros(totalNanos, 95)
                    + " resource_p50_us=" + percentileMicros(resourceNanos, 50)
                    + " resource_p95_us=" + percentileMicros(resourceNanos, 95)
                    + " draw_p50_us=" + percentileMicros(drawNanos, 50)
                    + " draw_p95_us=" + percentileMicros(drawNanos, 95)
                    + " draw_calls_min=" + minimumDrawCalls
                    + " draw_calls_max=" + maximumDrawCalls
                    + " lines=" + lines + " rects=" + rectangles
                    + " sprites=" + sprites + " text=" + text + " order=" + order;
        }

        private static long percentileMicros(long[] values, int percentile) {
            long[] ordered = values.clone();
            Arrays.sort(ordered);
            int rank = Math.min(ordered.length - 1,
                    Math.max(0, (ordered.length * percentile + 99) / 100 - 1));
            return ordered[rank] / 1_000L;
        }
    }

    private static final String VERTEX_SHADER =
            "attribute vec2 aPosition;" +
            "attribute vec4 aColor;" +
            "uniform vec2 uResolution;" +
            "varying vec4 vColor;" +
            "void main(){vec2 p=aPosition/uResolution*2.0-1.0;vColor=aColor;" +
            "gl_Position=vec4(p.x,-p.y,0.0,1.0);}";
    private static final String FRAGMENT_SHADER =
            "precision mediump float;varying vec4 vColor;" +
            "void main(){gl_FragColor=vColor;}";
    private static final String TEXTURE_VERTEX_SHADER =
            "attribute vec2 aPosition;attribute vec2 aTexCoord;attribute vec4 aColor;" +
            "uniform vec2 uResolution;varying vec2 vTexCoord;varying vec4 vColor;" +
            "void main(){vec2 p=aPosition/uResolution*2.0-1.0;vTexCoord=aTexCoord;" +
            "vColor=aColor;gl_Position=vec4(p.x,-p.y,0.0,1.0);}";
    private static final String TEXTURE_FRAGMENT_SHADER =
            "precision mediump float;uniform sampler2D uTexture;varying vec2 vTexCoord;" +
            "varying vec4 vColor;void main(){vec4 texel=texture2D(uTexture,vTexCoord);" +
            "gl_FragColor=texel*vec4(vColor.rgb*vColor.a,vColor.a);}";

    private final TextureProvider textures;
    private final TimingListener timing;
    private final RendererResourceLifecycle resourceLifecycle = new RendererResourceLifecycle();
    static final int MAX_PENDING_SPRITE_RELEASES = 256;
    private final ArrayDeque<Integer> pendingSpriteReleases = new ArrayDeque<>();
    private final FramePerformanceSamples performanceSamples = BuildConfig.STASIS_RENDER_ACCEPTANCE
            ? new FramePerformanceSamples(PERFORMANCE_WARMUP_FRAMES, PERFORMANCE_SAMPLE_FRAMES)
            : null;
    private final ByteBuffer frameI32Bytes = directBytes(FRAME_I32_CAPACITY * 4);
    private final ByteBuffer frameF32Bytes = directBytes(FRAME_F32_CAPACITY * 4);
    private final ByteBuffer frameU8Bytes = directBytes(TEXT_U8_CAPACITY);
    private final IntBuffer frameI32 = frameI32Bytes.asIntBuffer();
    private final FloatBuffer frameF32 = frameF32Bytes.asFloatBuffer();
    private final FloatBuffer lineVertices = directBytes(
            LINE_CHUNK_SIZE * 2 * COLOR_VERTEX_FLOATS * 4).asFloatBuffer();
    private final FloatBuffer spriteVertices = directBytes(
            SPRITE_CHUNK_SIZE * VERTICES_PER_QUAD * TEXTURE_VERTEX_FLOATS * 4).asFloatBuffer();
    private final int[] frameSpriteTextures = new int[MAX_SPRITES];
    private final int[] frameSpriteFilters = new int[MAX_SPRITES];
    private final long[] frameTextTextures = new long[MAX_TEXT];
    private int colorProgram;
    private int colorPosition;
    private int colorValue;
    private int colorResolution;
    private int textureProgram;
    private int texturePosition;
    private int textureCoordinate;
    private int textureColor;
    private int textureResolution;
    private int textureSampler;
    private int surfaceWidth = 1;
    private int surfaceHeight = 1;
    private int logicalWidth = 1;
    private int logicalHeight = 1;
    private DisplayViewport displayViewport = new DisplayViewport(0, 0, 1, 1, 1.0f, 1.0f);
    private int displayGeneration = -1;
    private int densityGeneration = -1;
    private CaptureCallback pendingCapture;
    private boolean restorePlaceholderPending;
    private long restorePlaceholderUntilNanos;
    private int renderAcceptanceFrameCount;
    private int lastPresentedFrameToken = -1;
    private int lastAcceptanceGlesEvidenceToken = -1;
    private int acceptanceTrace = -1;
    private int acceptanceTraceToken = -1;
    private int frameDrawCalls;
    private int activePipeline;

    StasisPreviewRenderer(TextureProvider textures, TimingListener timing) {
        this.textures = textures;
        this.timing = timing;
    }

    synchronized boolean enqueuePendingSpriteReleases(String message) {
        if (message == null || message.isEmpty()) return false;
        try {
            JSONObject response = new JSONObject(message);
            JSONArray handles = response.optJSONArray("handles");
            if (handles == null || handles.length() == 0) return false;
            if (handles.length() > MAX_PENDING_SPRITE_RELEASES - pendingSpriteReleases.size()) {
                return false;
            }
            boolean enqueued = false;
            for (int index = 0; index < handles.length(); index += 1) {
                int handle = handles.optInt(index, 0);
                if (handle != 0) {
                    pendingSpriteReleases.addLast(handle);
                    enqueued = true;
                }
            }
            return enqueued;
        } catch (Exception error) {
            return false;
        }
    }

    synchronized boolean hasPendingSpriteReleases() {
        return !pendingSpriteReleases.isEmpty();
    }

    synchronized boolean cancelPendingSpriteReleases(String message) {
        if (message == null || message.isEmpty()) return false;
        try {
            JSONArray handles = new JSONObject(message).optJSONArray("handles");
            if (handles == null) return false;
            boolean canceled = false;
            for (int index = 0; index < handles.length(); index += 1) {
                int handle = handles.optInt(index, 0);
                if (handle == 0) continue;
                while (pendingSpriteReleases.removeFirstOccurrence(handle)) {
                    canceled = true;
                }
            }
            return canceled;
        } catch (Exception error) {
            return false;
        }
    }

    // Package-private so queue behavior can be tested without constructing a GLES surface.
    synchronized void applyPendingSpriteReleases() {
        while (!pendingSpriteReleases.isEmpty()) {
            textures.releaseSprite(pendingSpriteReleases.removeFirst());
        }
    }

    // Callers fill these only while synchronized on this renderer. Native code writes
    // active spans directly, so a frame does not copy the full production capacities.
    ByteBuffer frameI32Bytes() {
        return frameI32Bytes;
    }

    ByteBuffer frameF32Bytes() {
        return frameF32Bytes;
    }

    ByteBuffer frameU8Bytes() {
        return frameU8Bytes;
    }

    synchronized float acceptanceFrameF32(int index) {
        return frameF32.get(index);
    }

    synchronized int frameToken() {
        return frameI32.get(I_FRAME_TOKEN);
    }

    synchronized int rectCount() {
        return frameI32.get(I_RECT_COUNT);
    }

    synchronized void copyFrameHeaderInto(int[] destination) {
        if (destination.length < HOST_HEADER_I32S) {
            throw new IllegalArgumentException("render header destination is too small");
        }
        for (int index = 0; index < HOST_HEADER_I32S; index += 1) {
            destination[index] = frameI32.get(index);
        }
    }

    synchronized void requestCapture(CaptureCallback callback) {
        if (pendingCapture != null) {
            pendingCapture.onCaptured(null, "a newer preview capture replaced this request",
                    new LogicalFrameSnapshot(new int[0], new float[0], new float[0],
                            new int[0], new float[0], new int[0], new float[0],
                            new byte[0], new int[0]));
        }
        pendingCapture = callback;
    }

    synchronized void onHostPaused() {
        resourceLifecycle.onPause();
    }

    synchronized void onHostResumed() {
        resourceLifecycle.onResume();
    }

    @Override
    public synchronized void onSurfaceCreated(javax.microedition.khronos.opengles.GL10 gl,
            javax.microedition.khronos.egl.EGLConfig config) {
        resourceLifecycle.onRendererCreated();
        restorePlaceholderPending = true;
        restorePlaceholderUntilNanos = System.nanoTime() + MIN_RESTORE_LABEL_NANOS;
        drawRestorePlaceholder();
        colorProgram = createProgram(VERTEX_SHADER, FRAGMENT_SHADER);
        colorPosition = GLES20.glGetAttribLocation(colorProgram, "aPosition");
        colorValue = GLES20.glGetAttribLocation(colorProgram, "aColor");
        colorResolution = GLES20.glGetUniformLocation(colorProgram, "uResolution");
        textureProgram = createProgram(TEXTURE_VERTEX_SHADER, TEXTURE_FRAGMENT_SHADER);
        texturePosition = GLES20.glGetAttribLocation(textureProgram, "aPosition");
        textureCoordinate = GLES20.glGetAttribLocation(textureProgram, "aTexCoord");
        textureColor = GLES20.glGetAttribLocation(textureProgram, "aColor");
        textureResolution = GLES20.glGetUniformLocation(textureProgram, "uResolution");
        textureSampler = GLES20.glGetUniformLocation(textureProgram, "uTexture");
        textures.onResourceGenerationChanged(
                resourceLifecycle.surfaceGeneration(),
                resourceLifecycle.rendererGeneration(), true, resourceLifecycle.reason());
        GLES20.glEnable(GLES20.GL_BLEND);
        GLES20.glBlendFunc(GLES20.GL_SRC_ALPHA, GLES20.GL_ONE_MINUS_SRC_ALPHA);
        GLES20.glClearColor(15.0f / 255.0f, 20.0f / 255.0f, 28.0f / 255.0f, 1.0f);
        GLES20.glClear(GLES20.GL_COLOR_BUFFER_BIT);
    }

    @Override
    public synchronized void onSurfaceChanged(javax.microedition.khronos.opengles.GL10 gl,
            int width, int height) {
        resourceLifecycle.onSurfaceChanged();
        surfaceWidth = Math.max(1, width);
        surfaceHeight = Math.max(1, height);
        GLES20.glViewport(0, 0, surfaceWidth, surfaceHeight);
        if (restorePlaceholderPending) drawRestorePlaceholder();
        displayGeneration = -1;
        Log.i(LOG_TAG, "drawable=" + surfaceWidth + "x" + surfaceHeight);
    }

    @Override
    public void onDrawFrame(javax.microedition.khronos.opengles.GL10 gl) {
        long started = System.nanoTime();
        CaptureCallback capture;
        LogicalFrameSnapshot capturedFrame;
        long resourceNanos = 0L;
        long drawNanos = 0L;
        int drawCalls = 0;
        int lineCount = 0;
        int rectCount = 0;
        int spriteCount = 0;
        int textCount = 0;
        int orderCount = 0;
        boolean presented = false;
        synchronized (this) {
            if (restorePlaceholderPending
                    && System.nanoTime() < restorePlaceholderUntilNanos) {
                drawRestorePlaceholder();
                timing.onRendered(System.nanoTime() - started);
                return;
            }
            restorePlaceholderPending = false;
            long resourceStarted = performanceSamples == null ? 0L : System.nanoTime();
            boolean restoring = resourceLifecycle.beginRestore();
            textures.beginRestoreAttempt();
            if (restoring) {
                while (GLES20.glGetError() != GLES20.GL_NO_ERROR) {}
            }
            boolean hasFrame = shouldPresent(frameI32);
            if (hasFrame) prepareFrameResources();
            String resourceFailure = textures.consumeFailure();
            int glError = restoring ? GLES20.glGetError() : GLES20.GL_NO_ERROR;
            boolean restoreComplete = textures.isRestoreComplete();
            boolean restored = resourceFailure == null && glError == GLES20.GL_NO_ERROR
                    && restoreComplete;
            if (restoring) {
                if (resourceFailure == null && glError == GLES20.GL_NO_ERROR
                        && !restoreComplete) {
                    resourceLifecycle.deferRestore();
                } else {
                    resourceLifecycle.finishRestore(restored);
                }
            } else if (!restored) {
                if (resourceFailure != null || glError != GLES20.GL_NO_ERROR) {
                    resourceLifecycle.resourceFailed();
                }
            }
            if (restoring) {
                if (restored) {
                    Log.i(LOG_TAG, "resources restored backend=gles surface_generation="
                            + resourceLifecycle.surfaceGeneration() + " renderer_generation="
                            + resourceLifecycle.rendererGeneration() + " reason="
                            + resourceLifecycle.reason());
                } else if (resourceFailure != null || glError != GLES20.GL_NO_ERROR) {
                    Log.e(LOG_TAG, "resource restore failed backend=gles surface_generation="
                            + resourceLifecycle.surfaceGeneration() + " renderer_generation="
                            + resourceLifecycle.rendererGeneration() + " reason="
                            + resourceLifecycle.reason() + " failure="
                            + (resourceFailure == null ? "gl_error_" + glError : resourceFailure));
                }
            }
            if (!restoreComplete && resourceFailure == null
                    && glError == GLES20.GL_NO_ERROR) drawRestorePlaceholder();
            if (hasFrame && restored && resourceLifecycle.canPresent()) {
                lineCount = clampCount(frameI32.get(I_LINE_COUNT), MAX_LINES);
                rectCount = clampedRectCount(
                        frameI32.get(I_VERSION), lineCount, frameI32.get(I_RECT_COUNT));
                spriteCount = clampCount(frameI32.get(I_SPRITE_COUNT), MAX_SPRITES);
                textCount = clampCount(frameI32.get(I_TEXT_COUNT), MAX_TEXT);
                orderCount = frameI32.get(I_VERSION) >= RENDER_V3_VERSION
                        ? clampCount(frameI32.get(I_ORDER_COUNT), MAX_ORDER) : 0;
                resourceNanos = performanceSamples == null
                        ? 0L : System.nanoTime() - resourceStarted;
                long drawStarted = performanceSamples == null ? 0L : System.nanoTime();
                frameDrawCalls = 0;
                drawFrame();
                drawNanos = performanceSamples == null
                        ? 0L : System.nanoTime() - drawStarted;
                drawCalls = frameDrawCalls;
                presented = true;
                int frameToken = frameI32.get(I_FRAME_TOKEN);
                lastPresentedFrameToken = frameToken;
                notifyAll();
                if (BuildConfig.STASIS_RENDER_ACCEPTANCE) {
                    if (rectCount >= 2 && frameToken != lastAcceptanceGlesEvidenceToken) {
                        lastAcceptanceGlesEvidenceToken = frameToken;
                        int markerBase = F_RECT_REVERSE_BASE - GEOMETRY_F32_STRIDE;
                        Log.i(LOG_TAG, "Stasis Workshop IT-027 GLES: {\"schema\":\"stasis.workshop_touch_roundtrip.v1\","
                                + "\"test_id\":\"IT-027\",\"event\":\"present\","
                                + "\"frame_token\":" + frameToken + ","
                                + "\"trace\":" + (acceptanceTraceToken == frameToken
                                        ? Integer.toUnsignedLong(acceptanceTrace) : -1L) + ","
                                + "\"rect_count\":" + rectCount + ",\"order_count\":" + orderCount + ","
                                + "\"marker\":{\"active\":true,\"x\":" + frameF32.get(markerBase)
                                + ",\"y\":" + frameF32.get(markerBase + 1)
                                + ",\"w\":" + frameF32.get(markerBase + 2)
                                + ",\"h\":" + frameF32.get(markerBase + 3)
                                + ",\"r\":" + frameF32.get(markerBase + 4)
                                + ",\"g\":" + frameF32.get(markerBase + 5)
                                + ",\"b\":" + frameF32.get(markerBase + 6)
                                + ",\"a\":" + frameF32.get(markerBase + 7) + "}}");
                    }
                    renderAcceptanceFrameCount += 1;
                    if (renderAcceptanceFrameCount == 1 || renderAcceptanceFrameCount % 30 == 0) {
                        Log.i(LOG_TAG, "RenderAcceptanceFrame: count=" + renderAcceptanceFrameCount
                                + " frame_token=" + frameToken);
                        Log.i(LOG_TAG, "Stasis Workshop IT-025 GLES: {\"schema\":\"stasis.workshop_seam.v1\","
                                + "\"test_id\":\"IT-025\",\"event\":\"present\",\"count\":"
                                + renderAcceptanceFrameCount + ","
                                + "\"frame_token\":" + frameToken + "}");
                    }
                }
            }
            finishPendingSpriteReleases(hasFrame, presented);
            capture = pendingCapture;
            pendingCapture = null;
            capturedFrame = capture == null ? null : captureLogicalFrame();
        }
        captureIfRequested(capture, capturedFrame);
        long totalNanos = System.nanoTime() - started;
        if (performanceSamples != null && presented) {
            String report = performanceSamples.add(totalNanos, resourceNanos, drawNanos,
                    drawCalls, lineCount, rectCount, spriteCount, textCount, orderCount);
            if (report != null) Log.i(LOG_TAG, report);
        }
        timing.onRendered(totalNanos);
    }

    // Acceptance synchronization waits for the GL thread to consume the exact
    // token written by the preceding JNI call. It is never used by production.
    synchronized boolean awaitPresentedFrameToken(int token, long timeoutMillis) {
        long deadline = System.nanoTime() + timeoutMillis * 1_000_000L;
        while (lastPresentedFrameToken != token) {
            long remaining = deadline - System.nanoTime();
            if (remaining <= 0L) return false;
            try {
                long millis = remaining / 1_000_000L;
                int nanos = (int)(remaining % 1_000_000L);
                wait(Math.max(1L, millis), nanos);
            } catch (InterruptedException interrupted) {
                Thread.currentThread().interrupt();
                return false;
            }
        }
        return true;
    }

    synchronized void setAcceptanceTrace(int token, int trace) {
        acceptanceTraceToken = token;
        acceptanceTrace = trace;
    }

    synchronized int acceptanceTrace() {
        return acceptanceTrace;
    }

    // Releases must wait until the command buffer has consumed its sprite textures. A
    // frame blocked by restore/resource failure is retried later, so retain its queue.
    synchronized void finishPendingSpriteReleases(boolean hasFrame, boolean presented) {
        if (!hasFrame || presented) applyPendingSpriteReleases();
    }

    private void drawRestorePlaceholder() {
        GLES20.glDisable(GLES20.GL_SCISSOR_TEST);
        GLES20.glViewport(0, 0, surfaceWidth, surfaceHeight);
        GLES20.glClearColor(15.0f / 255.0f, 20.0f / 255.0f, 28.0f / 255.0f, 1.0f);
        GLES20.glClear(GLES20.GL_COLOR_BUFFER_BIT);
        int columns = RESTORE_LABEL[0].length();
        int cell = Math.max(2, Math.min(surfaceWidth / (columns + 8), surfaceHeight / 60));
        int labelWidth = columns * cell;
        int labelHeight = RESTORE_LABEL.length * cell;
        int originX = (surfaceWidth - labelWidth) / 2;
        int originY = (surfaceHeight - labelHeight) / 2;
        GLES20.glEnable(GLES20.GL_SCISSOR_TEST);
        GLES20.glClearColor(66.0f / 255.0f, 153.0f / 255.0f, 225.0f / 255.0f, 1.0f);
        for (int row = 0; row < RESTORE_LABEL.length; row += 1) {
            String pixels = RESTORE_LABEL[row];
            for (int column = 0; column < pixels.length(); column += 1) {
                if (pixels.charAt(column) != '1') continue;
                GLES20.glScissor(originX + column * cell,
                        originY + (RESTORE_LABEL.length - row - 1) * cell, cell, cell);
                GLES20.glClear(GLES20.GL_COLOR_BUFFER_BIT);
            }
        }
        GLES20.glDisable(GLES20.GL_SCISSOR_TEST);
    }

    static boolean isValidRestoreLabel() {
        if (RESTORE_LABEL.length != 15 || RESTORE_LABEL[0].isEmpty()) return false;
        int width = RESTORE_LABEL[0].length();
        for (String row : RESTORE_LABEL) {
            if (row.length() != width) return false;
            for (int index = 0; index < width; index += 1) {
                char pixel = row.charAt(index);
                if (pixel != '0' && pixel != '1' && pixel != ' ') return false;
            }
        }
        return true;
    }

    private void drawFrame() {
        activePipeline = PIPELINE_NONE;
        GLES20.glDisable(GLES20.GL_SCISSOR_TEST);
        GLES20.glViewport(0, 0, surfaceWidth, surfaceHeight);
        clearLetterboxBars();
        GLES20.glViewport(displayViewport.x,
                surfaceHeight - displayViewport.y - displayViewport.height,
                displayViewport.width, displayViewport.height);
        int flags = frameI32.get(I_FLAGS);
        if ((flags & FLAG_CLEAR) != 0) {
            GLES20.glEnable(GLES20.GL_SCISSOR_TEST);
            GLES20.glScissor(displayViewport.x,
                    surfaceHeight - displayViewport.y - displayViewport.height,
                    displayViewport.width, displayViewport.height);
            GLES20.glClearColor(frameF32.get(0), frameF32.get(1), frameF32.get(2), frameF32.get(3));
            GLES20.glClear(GLES20.GL_COLOR_BUFFER_BIT);
            GLES20.glDisable(GLES20.GL_SCISSOR_TEST);
        }
        int lineCount = clampCount(frameI32.get(I_LINE_COUNT), MAX_LINES);
        int rectCount = clampedRectCount(
                frameI32.get(I_VERSION), lineCount, frameI32.get(I_RECT_COUNT));
        int spriteCount = clampCount(frameI32.get(I_SPRITE_COUNT), MAX_SPRITES);
        int textCount = clampCount(frameI32.get(I_TEXT_COUNT), MAX_TEXT);
        int orderCount = frameI32.get(I_VERSION) >= RENDER_V3_VERSION
                ? clampCount(frameI32.get(I_ORDER_COUNT), MAX_ORDER) : 0;
        if (orderCount == 0) {
            drawLines(0, lineCount);
            drawRects(0, rectCount);
            drawSprites(0, spriteCount);
            drawText(0, textCount);
            finishPipeline();
            return;
        }
        int position = 0;
        while (position < orderCount) {
            int entry = frameI32.get(I_ORDER_BASE + position);
            int kind = orderKind(entry);
            int index = orderIndex(entry);
            int limit = kind == ORDER_LINE ? lineCount
                    : kind == ORDER_RECT ? rectCount
                    : kind == ORDER_SPRITE ? spriteCount
                    : kind == ORDER_TEXT ? textCount : 0;
            if (entry < 0 || index >= limit) {
                position += 1;
                continue;
            }
            int run = 1;
            while (position + run < orderCount && index + run < limit) {
                int next = frameI32.get(I_ORDER_BASE + position + run);
                if (orderKind(next) != kind || orderIndex(next) != index + run) break;
                run += 1;
            }
            if (kind == ORDER_LINE) drawLines(index, run);
            else if (kind == ORDER_RECT) drawRects(index, run);
            else if (kind == ORDER_SPRITE) drawSprites(index, run);
            else drawText(index, run);
            position += run;
        }
        finishPipeline();
    }

    private void prepareFrameResources() {
        updateDisplayMetrics();
        textures.onFrameStart();
        resolveFrameResources(textures, frameI32, frameU8Bytes,
                frameSpriteTextures, frameSpriteFilters, frameTextTextures);
    }

    static void resolveFrameResources(TextureProvider textures, IntBuffer frameI32,
            ByteBuffer frameU8Bytes, int[] spriteTextures, int[] spriteFilters,
            long[] textTextures) {
        int spriteCount = clampCount(frameI32.get(I_SPRITE_COUNT), MAX_SPRITES);
        for (int index = 0; index < spriteCount; index += 1) {
            int base = I_SPRITE_BASE + index * SPRITE_I32_STRIDE;
            int handle = frameI32.get(base);
            int texture = textures.textureFor(handle);
            spriteTextures[index] = texture == 0 ? textures.fallbackTexture() : texture;
            spriteFilters[index] = textures.filterFor(handle);
        }
        int textCount = clampCount(frameI32.get(I_TEXT_COUNT), MAX_TEXT);
        int bytesUsed = clampCount(frameI32.get(I_TEXT_BYTES_USED), TEXT_U8_CAPACITY);
        for (int index = 0; index < textCount; index += 1) {
            int meta = I_TEXT_BASE + index * TEXT_I32_STRIDE;
            int font = frameI32.get(meta);
            int offset = frameI32.get(meta + 1);
            int length = frameI32.get(meta + 2);
            long texture = 0L;
            if (offset < 0) {
                if (font > 0 && offset != Integer.MIN_VALUE) {
                    texture = textures.cachedTextTextureFor(-offset);
                }
            } else if (font > 0 && isValidTextSpan(offset, length, bytesUsed)) {
                texture = textures.textTextureFor(font, frameU8Bytes, offset, length);
            }
            textTextures[index] = texture;
        }
    }

    private void clearLetterboxBars() {
        int right = displayViewport.x + displayViewport.width;
        int bottom = displayViewport.y + displayViewport.height;
        GLES20.glEnable(GLES20.GL_SCISSOR_TEST);
        GLES20.glClearColor(0.0f, 0.0f, 0.0f, 1.0f);
        clearScissorRect(0, 0, displayViewport.x, surfaceHeight);
        clearScissorRect(right, 0, surfaceWidth - right, surfaceHeight);
        clearScissorRect(0, 0, surfaceWidth, surfaceHeight - bottom);
        clearScissorRect(0, surfaceHeight - displayViewport.y,
                surfaceWidth, displayViewport.y);
        GLES20.glDisable(GLES20.GL_SCISSOR_TEST);
    }

    private static void clearScissorRect(int x, int y, int width, int height) {
        if (width <= 0 || height <= 0) return;
        GLES20.glScissor(x, y, width, height);
        GLES20.glClear(GLES20.GL_COLOR_BUFFER_BIT);
    }

    private void updateDisplayMetrics() {
        float previousRasterScale = displayViewport.rasterScale;
        int nextLogicalWidth = frameI32.get(I_LOGICAL_W);
        int nextLogicalHeight = frameI32.get(I_LOGICAL_H);
        if (nextLogicalWidth <= 0 || nextLogicalHeight <= 0) {
            nextLogicalWidth = surfaceWidth;
            nextLogicalHeight = surfaceHeight;
        }
        int nextDisplayGeneration = frameI32.get(I_DISPLAY_GENERATION);
        int nextDensityGeneration = frameI32.get(I_DENSITY_GENERATION);
        if (displayGeneration != nextDisplayGeneration
                || logicalWidth != nextLogicalWidth || logicalHeight != nextLogicalHeight) {
            logicalWidth = nextLogicalWidth;
            logicalHeight = nextLogicalHeight;
            displayViewport = fitViewport(logicalWidth, logicalHeight, surfaceWidth, surfaceHeight);
            displayGeneration = nextDisplayGeneration;
            Log.i(LOG_TAG, "logical=" + logicalWidth + "x" + logicalHeight
                    + " viewport=" + displayViewport.x + "," + displayViewport.y + ","
                    + displayViewport.width + "x" + displayViewport.height
                    + " generation=" + displayGeneration);
        }
        if (densityGeneration != nextDensityGeneration
                || Math.abs(previousRasterScale - displayViewport.rasterScale) >= 0.001f) {
            densityGeneration = nextDensityGeneration;
            textures.onDisplayMetricsChanged(displayViewport.rasterScale, densityGeneration);
        }
    }

    static DisplayViewport fitViewport(int logicalWidth, int logicalHeight,
            int drawableWidth, int drawableHeight) {
        logicalWidth = Math.max(1, logicalWidth);
        logicalHeight = Math.max(1, logicalHeight);
        drawableWidth = Math.max(1, drawableWidth);
        drawableHeight = Math.max(1, drawableHeight);
        float scale = Math.min((float)drawableWidth / logicalWidth,
                (float)drawableHeight / logicalHeight);
        int width = Math.max(1, Math.round(logicalWidth * scale));
        int height = Math.max(1, Math.round(logicalHeight * scale));
        int x = (drawableWidth - width) / 2;
        int y = (drawableHeight - height) / 2;
        float contentScale = Math.min((float)width / logicalWidth,
                (float)height / logicalHeight);
        return new DisplayViewport(x, y, width, height, contentScale,
                Math.max(1.0f, Math.min(8.0f, contentScale)));
    }

    static String formatResourceFailure(String stage, int handle, String path,
            int logicalWidth, int logicalHeight, int rasterWidth, int rasterHeight,
            int surfaceGeneration, int rendererGeneration, String transitionReason,
            String failure) {
        return "stage=" + stage + " handle=" + handle + " path=" + path
                + " logical=" + logicalWidth + "x" + logicalHeight
                + " raster=" + rasterWidth + "x" + rasterHeight + " backend=gles"
                + " surface_generation=" + surfaceGeneration
                + " renderer_generation=" + rendererGeneration
                + " reason=" + transitionReason + " failure=" + failure;
    }

    static boolean textureCreationSucceeded(int texture, int glError) {
        return texture != 0 && glError == GLES20.GL_NO_ERROR;
    }

    private void drawLines(int first, int count) {
        int end = first + count;
        while (first < end) {
            int horizontalRun = horizontalRunLength(frameF32, first, end);
            if (horizontalRun >= 2) {
                lineVertices.clear();
                int quadCount = 0;
                while (first < end && quadCount < LINE_CHUNK_SIZE / 3) {
                    horizontalRun = horizontalRunLength(frameF32, first, end);
                    if (horizontalRun < 2) break;
                    int base = F_LINE_BASE + first * LINE_F32_STRIDE;
                    appendColorQuad(lineVertices,
                            frameF32.get(base), frameF32.get(base + 1), frameF32.get(base + 2),
                            frameF32.get(base + 1) + horizontalRun,
                            frameF32.get(base + 4), frameF32.get(base + 5),
                            frameF32.get(base + 6), frameF32.get(base + 7));
                    first += horizontalRun;
                    quadCount += 1;
                }
                lineVertices.flip();
                drawColorBatch(lineVertices,
                        quadCount * VERTICES_PER_QUAD, GLES20.GL_TRIANGLES);
                continue;
            }
            int chunk = Math.min(LINE_CHUNK_SIZE, end - first);
            for (int offset = 1; offset < chunk; offset += 1) {
                if (horizontalRunLength(frameF32, first + offset, end) >= 2) {
                    chunk = offset;
                    break;
                }
            }
            lineVertices.clear();
            for (int index = 0; index < chunk; index += 1) {
                int base = F_LINE_BASE + (first + index) * LINE_F32_STRIDE;
                putColorVertex(lineVertices, frameF32.get(base), frameF32.get(base + 1), base);
                putColorVertex(lineVertices, frameF32.get(base + 2), frameF32.get(base + 3), base);
            }
            lineVertices.flip();
            drawColorBatch(lineVertices, chunk * 2, GLES20.GL_LINES);
            first += chunk;
        }
    }

    private void drawRects(int first, int count) {
        int end = first + count;
        while (first < end) {
            int chunk = Math.min(LINE_CHUNK_SIZE / 3, end - first);
            lineVertices.clear();
            for (int index = 0; index < chunk; index += 1) {
                int base = F_RECT_REVERSE_BASE
                        - (first + index) * GEOMETRY_F32_STRIDE;
                float left = frameF32.get(base);
                float top = frameF32.get(base + 1);
                appendColorQuad(lineVertices, left, top,
                        left + frameF32.get(base + 2), top + frameF32.get(base + 3),
                        frameF32.get(base + 4), frameF32.get(base + 5),
                        frameF32.get(base + 6), frameF32.get(base + 7));
            }
            lineVertices.flip();
            drawColorBatch(lineVertices,
                    chunk * VERTICES_PER_QUAD, GLES20.GL_TRIANGLES);
            first += chunk;
        }
    }

    static int horizontalRunLength(FloatBuffer values, int first, int count) {
        if (values == null || first < 0 || first >= count) return 0;
        int base = F_LINE_BASE + first * LINE_F32_STRIDE;
        float left = values.get(base);
        float top = values.get(base + 1);
        float right = values.get(base + 2);
        if (top != values.get(base + 3)) return 1;
        int run = 1;
        while (first + run < count) {
            int next = F_LINE_BASE + (first + run) * LINE_F32_STRIDE;
            if (values.get(next) != left || values.get(next + 2) != right
                    || values.get(next + 1) != top + run || values.get(next + 3) != top + run
                    || values.get(next + 4) != values.get(base + 4)
                    || values.get(next + 5) != values.get(base + 5)
                    || values.get(next + 6) != values.get(base + 6)
                    || values.get(next + 7) != values.get(base + 7)) {
                break;
            }
            run += 1;
        }
        return run;
    }

    private static void appendColorQuad(FloatBuffer output, float left, float top,
            float right, float bottom, float red, float green, float blue, float alpha) {
        putColorVertex(output, left, top, red, green, blue, alpha);
        putColorVertex(output, right, top, red, green, blue, alpha);
        putColorVertex(output, left, bottom, red, green, blue, alpha);
        putColorVertex(output, right, top, red, green, blue, alpha);
        putColorVertex(output, right, bottom, red, green, blue, alpha);
        putColorVertex(output, left, bottom, red, green, blue, alpha);
    }

    private static void putColorVertex(FloatBuffer output, float x, float y,
            float red, float green, float blue, float alpha) {
        output.put(x).put(y).put(red).put(green).put(blue).put(alpha);
    }

    private void putColorVertex(FloatBuffer output, float x, float y, int lineBase) {
        output.put(x).put(y)
                .put(frameF32.get(lineBase + 4)).put(frameF32.get(lineBase + 5))
                .put(frameF32.get(lineBase + 6)).put(frameF32.get(lineBase + 7));
    }

    private void drawSprites(int first, int count) {
        if (count == 0) return;
        int index = first;
        int end = first + count;
        while (index < end) {
            int base = I_SPRITE_BASE + index * SPRITE_I32_STRIDE;
            int texture = frameSpriteTextures[index];
            int filter = frameSpriteFilters[index];
            spriteVertices.clear();
            appendSprite(base);
            int chunk = 1;
            while (index + chunk < end && chunk < SPRITE_CHUNK_SIZE) {
                if (frameSpriteTextures[index + chunk] != texture
                        || frameSpriteFilters[index + chunk] != filter) break;
                int next = I_SPRITE_BASE + (index + chunk) * SPRITE_I32_STRIDE;
                appendSprite(next);
                chunk += 1;
            }
            spriteVertices.flip();
            drawPreparedTextureBatch(chunk * VERTICES_PER_QUAD, texture);
            index += chunk;
        }
    }

    private void appendSprite(int base) {
        int index = (base - I_SPRITE_BASE) / SPRITE_I32_STRIDE;
        int values = F_SPRITE_BASE + index * SPRITE_F32_STRIDE;
        float left = frameF32.get(values);
        float top = frameF32.get(values + 1);
        float right = left + frameF32.get(values + 2);
        float bottom = top + frameF32.get(values + 3);
        float centerX = (left + right) * 0.5f;
        float centerY = (top + bottom) * 0.5f;
        double radians = Math.toRadians(frameI32.get(base + 1) % 360);
        float cosine = (float)Math.cos(radians);
        float sine = (float)Math.sin(radians);
        float alpha = clampUnit(frameI32.get(base + 2) / 255.0f);
        float u0 = frameF32.get(values + 4);
        float v0 = frameF32.get(values + 5);
        float u1 = frameF32.get(values + 6);
        float v1 = frameF32.get(values + 7);
        if (u0 < 0 || v0 < 0 || u1 > 1 || v1 > 1 || u0 >= u1 || v0 >= v1) return;
        putTextureVertex(spriteVertices, left, top, centerX, centerY, cosine, sine, u0, v0, 1, 1, 1, alpha);
        putTextureVertex(spriteVertices, right, top, centerX, centerY, cosine, sine, u1, v0, 1, 1, 1, alpha);
        putTextureVertex(spriteVertices, left, bottom, centerX, centerY, cosine, sine, u0, v1, 1, 1, 1, alpha);
        putTextureVertex(spriteVertices, right, top, centerX, centerY, cosine, sine, u1, v0, 1, 1, 1, alpha);
        putTextureVertex(spriteVertices, right, bottom, centerX, centerY, cosine, sine, u1, v1, 1, 1, 1, alpha);
        putTextureVertex(spriteVertices, left, bottom, centerX, centerY, cosine, sine, u0, v1, 1, 1, 1, alpha);
    }

    private void drawText(int first, int count) {
        if (count == 0) return;
        int end = first + count;
        for (int index = first; index < end; index += 1) {
            long packed = frameTextTextures[index];
            int texture = (int)packed;
            int width = (int)((packed >>> 32) & 0xffffL);
            int height = (int)((packed >>> 48) & 0xffffL);
            if (texture == 0 || width == 0 || height == 0) continue;
            int values = F_TEXT_BASE + index * TEXT_F32_STRIDE;
            float left = frameF32.get(values);
            float top = frameF32.get(values + 1);
            spriteVertices.clear();
            appendAxisAlignedQuad(spriteVertices, left, top, left + width, top + height,
                    clampUnit(frameF32.get(values + 2)), clampUnit(frameF32.get(values + 3)),
                    clampUnit(frameF32.get(values + 4)), clampUnit(frameF32.get(values + 5)));
            spriteVertices.flip();
            drawPreparedTextureBatch(VERTICES_PER_QUAD, texture);
        }
    }

    private static void appendAxisAlignedQuad(FloatBuffer output, float left, float top,
            float right, float bottom, float red, float green, float blue, float alpha) {
        putTextureVertex(output, left, top, left, top, 1, 0, 0, 0, red, green, blue, alpha);
        putTextureVertex(output, right, top, left, top, 1, 0, 1, 0, red, green, blue, alpha);
        putTextureVertex(output, left, bottom, left, top, 1, 0, 0, 1, red, green, blue, alpha);
        putTextureVertex(output, right, top, left, top, 1, 0, 1, 0, red, green, blue, alpha);
        putTextureVertex(output, right, bottom, left, top, 1, 0, 1, 1, red, green, blue, alpha);
        putTextureVertex(output, left, bottom, left, top, 1, 0, 0, 1, red, green, blue, alpha);
    }

    private static void putTextureVertex(FloatBuffer output, float x, float y,
            float centerX, float centerY, float cosine, float sine, float u, float v,
            float red, float green, float blue, float alpha) {
        float offsetX = x - centerX;
        float offsetY = y - centerY;
        output.put(centerX + offsetX * cosine - offsetY * sine)
                .put(centerY + offsetX * sine + offsetY * cosine)
                .put(u).put(v).put(red).put(green).put(blue).put(alpha);
    }

    private void drawColorBatch(FloatBuffer vertices, int vertexCount, int mode) {
        beginColorBatches(vertices);
        GLES20.glDrawArrays(mode, 0, vertexCount);
        frameDrawCalls += 1;
    }

    private void beginColorBatches(FloatBuffer vertices) {
        if (activePipeline != PIPELINE_COLOR) {
            finishPipeline();
            GLES20.glBlendFunc(GLES20.GL_SRC_ALPHA, GLES20.GL_ONE_MINUS_SRC_ALPHA);
            GLES20.glUseProgram(colorProgram);
            GLES20.glUniform2f(colorResolution, logicalWidth, logicalHeight);
            GLES20.glEnableVertexAttribArray(colorPosition);
            GLES20.glEnableVertexAttribArray(colorValue);
            activePipeline = PIPELINE_COLOR;
        }
        vertices.position(0);
        GLES20.glVertexAttribPointer(colorPosition, 2, GLES20.GL_FLOAT, false, COLOR_VERTEX_BYTES, vertices);
        vertices.position(2);
        GLES20.glVertexAttribPointer(colorValue, 4, GLES20.GL_FLOAT, false, COLOR_VERTEX_BYTES, vertices);
        vertices.position(0);
    }

    private void beginTextureBatches(FloatBuffer vertices) {
        if (activePipeline != PIPELINE_TEXTURE) {
            finishPipeline();
            GLES20.glBlendFunc(GLES20.GL_ONE, GLES20.GL_ONE_MINUS_SRC_ALPHA);
            GLES20.glUseProgram(textureProgram);
            GLES20.glUniform2f(textureResolution, logicalWidth, logicalHeight);
            GLES20.glActiveTexture(GLES20.GL_TEXTURE0);
            GLES20.glUniform1i(textureSampler, 0);
            GLES20.glEnableVertexAttribArray(texturePosition);
            GLES20.glEnableVertexAttribArray(textureCoordinate);
            GLES20.glEnableVertexAttribArray(textureColor);
            activePipeline = PIPELINE_TEXTURE;
        }
        vertices.position(0);
        GLES20.glVertexAttribPointer(texturePosition, 2, GLES20.GL_FLOAT, false, TEXTURE_VERTEX_BYTES, vertices);
        vertices.position(2);
        GLES20.glVertexAttribPointer(textureCoordinate, 2, GLES20.GL_FLOAT, false, TEXTURE_VERTEX_BYTES, vertices);
        vertices.position(4);
        GLES20.glVertexAttribPointer(textureColor, 4, GLES20.GL_FLOAT, false, TEXTURE_VERTEX_BYTES, vertices);
        vertices.position(0);
    }

    private void drawPreparedTextureBatch(int vertexCount, int texture) {
        beginTextureBatches(spriteVertices);
        GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, texture);
        GLES20.glDrawArrays(GLES20.GL_TRIANGLES, 0, vertexCount);
        frameDrawCalls += 1;
    }

    private void finishPipeline() {
        if (activePipeline == PIPELINE_COLOR) {
            GLES20.glDisableVertexAttribArray(colorValue);
            GLES20.glDisableVertexAttribArray(colorPosition);
        } else if (activePipeline == PIPELINE_TEXTURE) {
            GLES20.glDisableVertexAttribArray(textureColor);
            GLES20.glDisableVertexAttribArray(textureCoordinate);
            GLES20.glDisableVertexAttribArray(texturePosition);
            GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, 0);
        }
        activePipeline = PIPELINE_NONE;
    }

    static int clampCount(int value, int maximum) {
        return Math.max(0, Math.min(maximum, value));
    }

    static int orderKind(int entry) {
        return entry < 0 ? 0 : entry / ORDER_KIND_SCALE;
    }

    static int orderIndex(int entry) {
        return entry < 0 ? -1 : entry % ORDER_KIND_SCALE;
    }

    static boolean isValidFrame(IntBuffer values) {
        return values != null && values.capacity() >= FRAME_I32_CAPACITY
                && values.get(I_MAGIC) == RENDER_MAGIC
                && (values.get(I_VERSION) == RENDER_V2_VERSION
                    || values.get(I_VERSION) == RENDER_V3_VERSION
                    || values.get(I_VERSION) == RENDER_V4_VERSION
                    || values.get(I_VERSION) == RENDER_VERSION);
    }

    static boolean shouldPresent(IntBuffer values) {
        return isValidFrame(values) && (values.get(I_FLAGS) & FLAG_PRESENT) != 0;
    }

    static boolean isValidTextSpan(int offset, int length, int bytesUsed) {
        return offset >= 0 && length >= 0 && offset < bytesUsed
                && length < bytesUsed - offset;
    }

    static int activeSpriteI32Count(int spriteCount) {
        return clampCount(spriteCount, MAX_SPRITES) * SPRITE_I32_STRIDE;
    }

    static int activeSpriteF32Count(int spriteCount) {
        return clampCount(spriteCount, MAX_SPRITES) * SPRITE_F32_STRIDE;
    }

    static int activeTextI32Count(int textCount) {
        return clampCount(textCount, MAX_TEXT) * TEXT_I32_STRIDE;
    }

    static int activeLineF32Count(int lineCount) {
        return clampCount(lineCount, MAX_LINES) * LINE_F32_STRIDE;
    }

    static int clampedRectCount(int version, int lineCount, int rectCount) {
        if (version < RENDER_V4_VERSION) return 0;
        int lines = clampCount(lineCount, MAX_GEOMETRY);
        return clampCount(rectCount, MAX_GEOMETRY - lines);
    }

    static int activeRectF32Count(int version, int lineCount, int rectCount) {
        return clampedRectCount(version, lineCount, rectCount) * GEOMETRY_F32_STRIDE;
    }

    static int activeTextF32Count(int textCount) {
        return clampCount(textCount, MAX_TEXT) * TEXT_F32_STRIDE;
    }

    static int activeTextU8Count(int bytesUsed) {
        return clampCount(bytesUsed, TEXT_U8_CAPACITY);
    }

    static long packTexture(int texture, int width, int height) {
        if (texture == 0 || width <= 0 || height <= 0 || width > 0xffff || height > 0xffff) return 0L;
        return (texture & 0xffffffffL) | ((long)width << 32) | ((long)height << 48);
    }

    private static float clampUnit(float value) {
        return Math.max(0.0f, Math.min(1.0f, value));
    }

    synchronized LogicalFrameSnapshot captureLogicalFrame() {
        int[] header = new int[CAPTURE_HEADER_I32S];
        for (int index = 0; index < header.length; index += 1) header[index] = frameI32.get(index);
        int lineCount = clampCount(header[I_LINE_COUNT], MAX_LINES);
        int rectCount = clampedRectCount(
                header[I_VERSION], lineCount, header[I_RECT_COUNT]);
        int spriteCount = clampCount(header[I_SPRITE_COUNT], MAX_SPRITES);
        int textCount = clampCount(header[I_TEXT_COUNT], MAX_TEXT);
        int textByteCount = clampCount(header[I_TEXT_BYTES_USED], TEXT_U8_CAPACITY);
        float[] lines = new float[activeLineF32Count(lineCount)];
        for (int index = 0; index < lines.length; index += 1) {
            lines[index] = frameF32.get(F_LINE_BASE + index);
        }
        float[] rectangles = new float[activeRectF32Count(
                header[I_VERSION], lineCount, rectCount)];
        for (int rect = 0; rect < rectCount; rect += 1) {
            int source = F_RECT_REVERSE_BASE - rect * GEOMETRY_F32_STRIDE;
            int destination = rect * GEOMETRY_F32_STRIDE;
            for (int field = 0; field < GEOMETRY_F32_STRIDE; field += 1) {
                rectangles[destination + field] = frameF32.get(source + field);
            }
        }
        int[] sprites = new int[activeSpriteI32Count(spriteCount)];
        for (int index = 0; index < sprites.length; index += 1) {
            sprites[index] = frameI32.get(I_SPRITE_BASE + index);
        }
        float[] spriteValues = new float[activeSpriteF32Count(spriteCount)];
        for (int index = 0; index < spriteValues.length; index += 1) {
            spriteValues[index] = frameF32.get(F_SPRITE_BASE + index);
        }
        int[] textMetadata = new int[activeTextI32Count(textCount)];
        for (int index = 0; index < textMetadata.length; index += 1) {
            textMetadata[index] = frameI32.get(I_TEXT_BASE + index);
        }
        float[] textValues = new float[activeTextF32Count(textCount)];
        for (int index = 0; index < textValues.length; index += 1) {
            textValues[index] = frameF32.get(F_TEXT_BASE + index);
        }
        byte[] textBytes = new byte[textByteCount];
        for (int index = 0; index < textBytes.length; index += 1) {
            textBytes[index] = frameU8Bytes.get(index);
        }
        int orderCount = frameI32.get(I_VERSION) >= RENDER_V3_VERSION
                ? clampCount(frameI32.get(I_ORDER_COUNT), MAX_ORDER) : 0;
        int[] order = new int[orderCount];
        for (int index = 0; index < order.length; index += 1) {
            order[index] = frameI32.get(I_ORDER_BASE + index);
        }
        return new LogicalFrameSnapshot(
                header, lines, rectangles, sprites, spriteValues,
                textMetadata, textValues, textBytes, order);
    }

    private static ByteBuffer directBytes(int capacity) {
        return ByteBuffer.allocateDirect(capacity).order(ByteOrder.nativeOrder());
    }

    private void captureIfRequested(CaptureCallback callback, LogicalFrameSnapshot capturedFrame) {
        if (callback == null) return;
        try {
            long pixelCount = (long)surfaceWidth * surfaceHeight;
            if (pixelCount > MAX_CAPTURE_PIXELS) {
                callback.onCaptured(null, "preview framebuffer exceeds the 8 megapixel capture limit", capturedFrame);
                return;
            }
            java.nio.IntBuffer pixels = directBytes(surfaceWidth * surfaceHeight * 4).asIntBuffer();
            GLES20.glReadPixels(0, 0, surfaceWidth, surfaceHeight, GLES20.GL_RGBA,
                    GLES20.GL_UNSIGNED_BYTE, pixels);
            int[] flipped = new int[surfaceWidth * surfaceHeight];
            for (int y = 0; y < surfaceHeight; y += 1) {
                int sourceRow = y * surfaceWidth;
                int targetRow = (surfaceHeight - y - 1) * surfaceWidth;
                for (int x = 0; x < surfaceWidth; x += 1) {
                    int rgba = pixels.get(sourceRow + x);
                    flipped[targetRow + x] = (rgba & 0xff00ff00)
                            | ((rgba << 16) & 0x00ff0000) | ((rgba >> 16) & 0x000000ff);
                }
            }
            Bitmap full = Bitmap.createBitmap(flipped, surfaceWidth, surfaceHeight, Bitmap.Config.ARGB_8888);
            int largest = Math.max(surfaceWidth, surfaceHeight);
            if (largest <= 1024) {
                callback.onCaptured(full, "", capturedFrame);
                return;
            }
            float scale = 1024.0f / largest;
            Bitmap bounded = Bitmap.createScaledBitmap(full,
                    Math.max(1, Math.round(surfaceWidth * scale)),
                    Math.max(1, Math.round(surfaceHeight * scale)), true);
            full.recycle();
            callback.onCaptured(bounded, "", capturedFrame);
        } catch (OutOfMemoryError error) {
            callback.onCaptured(null, "not enough memory for bounded pixel capture", capturedFrame);
        } catch (RuntimeException error) {
            callback.onCaptured(null, error.getMessage(), capturedFrame);
        }
    }

    private static int createProgram(String vertexSource, String fragmentSource) {
        int vertex = compileShader(GLES20.GL_VERTEX_SHADER, vertexSource);
        int fragment = compileShader(GLES20.GL_FRAGMENT_SHADER, fragmentSource);
        int program = GLES20.glCreateProgram();
        GLES20.glAttachShader(program, vertex);
        GLES20.glAttachShader(program, fragment);
        GLES20.glLinkProgram(program);
        GLES20.glDeleteShader(vertex);
        GLES20.glDeleteShader(fragment);
        int[] linked = new int[1];
        GLES20.glGetProgramiv(program, GLES20.GL_LINK_STATUS, linked, 0);
        if (linked[0] == 0) {
            String log = GLES20.glGetProgramInfoLog(program);
            GLES20.glDeleteProgram(program);
            throw new IllegalStateException("OpenGL program link failed: " + log);
        }
        return program;
    }

    private static int compileShader(int type, String source) {
        int shader = GLES20.glCreateShader(type);
        GLES20.glShaderSource(shader, source);
        GLES20.glCompileShader(shader);
        int[] compiled = new int[1];
        GLES20.glGetShaderiv(shader, GLES20.GL_COMPILE_STATUS, compiled, 0);
        if (compiled[0] == 0) {
            String log = GLES20.glGetShaderInfoLog(shader);
            GLES20.glDeleteShader(shader);
            throw new IllegalStateException("OpenGL shader compile failed: " + log);
        }
        return shader;
    }
}
