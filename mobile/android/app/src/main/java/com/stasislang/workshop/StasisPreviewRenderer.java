package com.stasislang.workshop;

import android.graphics.Bitmap;
import android.opengl.GLES20;
import android.opengl.GLSurfaceView;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.FloatBuffer;

final class StasisPreviewRenderer implements GLSurfaceView.Renderer {
    static final int COMMAND_CAPACITY = 8;
    static final int FRAME_HEADER_SIZE = 6;
    static final int COMMAND_STRIDE = 13;
    static final int FRAME_I32_CAPACITY = FRAME_HEADER_SIZE + COMMAND_CAPACITY * COMMAND_STRIDE;

    private static final int RECT_VERTICES = 6;
    private static final int RECT_VERTEX_FLOATS = 6;
    private static final int RECT_VERTEX_BYTES = RECT_VERTEX_FLOATS * 4;
    private static final int SPRITE_VERTEX_FLOATS = 8;
    private static final int SPRITE_VERTEX_BYTES = SPRITE_VERTEX_FLOATS * 4;
    private static final int MAX_CAPTURE_PIXELS = 8_000_000;

    interface TextureProvider {
        void onSurfaceCreated();
        int textureFor(int handle);
    }

    interface TimingListener {
        void onRendered(long durationNanos);
    }

    interface CaptureCallback {
        void onCaptured(Bitmap bitmap, String error, int[] capturedFrame);
    }

    private static final String VERTEX_SHADER =
            "attribute vec2 aPosition;" +
            "attribute vec4 aColor;" +
            "uniform vec2 uResolution;" +
            "varying vec4 vColor;" +
            "void main() {" +
            "  vec2 zeroToOne = aPosition / uResolution;" +
            "  vec2 clip = zeroToOne * 2.0 - 1.0;" +
            "  vColor = aColor;" +
            "  gl_Position = vec4(clip.x, -clip.y, 0.0, 1.0);" +
            "}";
    private static final String FRAGMENT_SHADER =
            "precision mediump float;" +
            "varying vec4 vColor;" +
            "void main() { gl_FragColor = vColor; }";
    private static final String TEXTURE_VERTEX_SHADER =
            "attribute vec2 aPosition;" +
            "attribute vec2 aTexCoord;" +
            "attribute vec4 aColor;" +
            "uniform vec2 uResolution;" +
            "varying vec2 vTexCoord;" +
            "varying vec4 vColor;" +
            "void main() {" +
            "  vec2 zeroToOne = aPosition / uResolution;" +
            "  vec2 clip = zeroToOne * 2.0 - 1.0;" +
            "  vTexCoord = aTexCoord;" +
            "  vColor = aColor;" +
            "  gl_Position = vec4(clip.x, -clip.y, 0.0, 1.0);" +
            "}";
    private static final String TEXTURE_FRAGMENT_SHADER =
            "precision mediump float;" +
            "uniform sampler2D uTexture;" +
            "varying vec2 vTexCoord;" +
            "varying vec4 vColor;" +
            "void main() { gl_FragColor = texture2D(uTexture, vTexCoord) * vColor; }";

    private final TextureProvider textures;
    private final TimingListener timing;
    private final FloatBuffer rectVertices = ByteBuffer
            .allocateDirect(COMMAND_CAPACITY * RECT_VERTICES * RECT_VERTEX_FLOATS * 4)
            .order(ByteOrder.nativeOrder())
            .asFloatBuffer();
    private final FloatBuffer spriteVertices = ByteBuffer
            .allocateDirect(COMMAND_CAPACITY * RECT_VERTICES * SPRITE_VERTEX_FLOATS * 4)
            .order(ByteOrder.nativeOrder())
            .asFloatBuffer();
    private final int[] frame = new int[FRAME_I32_CAPACITY];
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
    private CaptureCallback pendingCapture;

    StasisPreviewRenderer(TextureProvider textures, TimingListener timing) {
        this.textures = textures;
        this.timing = timing;
    }

    synchronized void setFrame(int[] values) {
        if (values.length < FRAME_I32_CAPACITY) {
            throw new IllegalArgumentException("render frame is smaller than schema v3");
        }
        System.arraycopy(values, 0, frame, 0, FRAME_I32_CAPACITY);
    }

    synchronized void requestCapture(CaptureCallback callback) {
        if (pendingCapture != null) {
            pendingCapture.onCaptured(null, "a newer preview capture replaced this request", new int[0]);
        }
        pendingCapture = callback;
    }

    @Override
    public void onSurfaceCreated(javax.microedition.khronos.opengles.GL10 gl,
            javax.microedition.khronos.egl.EGLConfig config) {
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
        textures.onSurfaceCreated();
        GLES20.glEnable(GLES20.GL_BLEND);
        GLES20.glBlendFunc(GLES20.GL_SRC_ALPHA, GLES20.GL_ONE_MINUS_SRC_ALPHA);
        GLES20.glClearColor(15.0f / 255.0f, 20.0f / 255.0f, 28.0f / 255.0f, 1.0f);
    }

    @Override
    public void onSurfaceChanged(javax.microedition.khronos.opengles.GL10 gl, int width, int height) {
        surfaceWidth = Math.max(1, width);
        surfaceHeight = Math.max(1, height);
        GLES20.glViewport(0, 0, surfaceWidth, surfaceHeight);
    }

    @Override
    public void onDrawFrame(javax.microedition.khronos.opengles.GL10 gl) {
        long started = System.nanoTime();
        GLES20.glClear(GLES20.GL_COLOR_BUFFER_BIT);
        CaptureCallback capture;
        int[] capturedFrame;
        synchronized (this) {
            drawCommands();
            capture = pendingCapture;
            pendingCapture = null;
            capturedFrame = capture == null ? null : frame.clone();
        }
        captureIfRequested(capture, capturedFrame);
        timing.onRendered(System.nanoTime() - started);
    }

    private void drawCommands() {
        int commandCount = Math.max(0, Math.min(COMMAND_CAPACITY, frame[5]));
        int index = 0;
        while (index < commandCount) {
            int base = FRAME_HEADER_SIZE + index * COMMAND_STRIDE;
            int kind = frame[base];
            if (kind == 1) {
                rectVertices.clear();
                int runEnd = index;
                while (runEnd < commandCount) {
                    int runBase = FRAME_HEADER_SIZE + runEnd * COMMAND_STRIDE;
                    if (frame[runBase] != 1 || !sameClip(base, runBase)) break;
                    appendRect(runBase);
                    runEnd += 1;
                }
                rectVertices.flip();
                applyClip(base);
                drawRectBatch((runEnd - index) * RECT_VERTICES);
                index = runEnd;
            } else if (kind == 2) {
                int texture = textures.textureFor(frame[base + 6]);
                spriteVertices.clear();
                appendSprite(base);
                int runEnd = index + 1;
                while (runEnd < commandCount) {
                    int runBase = FRAME_HEADER_SIZE + runEnd * COMMAND_STRIDE;
                    if (frame[runBase] != 2 || !sameClip(base, runBase)
                            || textures.textureFor(frame[runBase + 6]) != texture) break;
                    appendSprite(runBase);
                    runEnd += 1;
                }
                spriteVertices.flip();
                applyClip(base);
                drawSpriteBatch((runEnd - index) * RECT_VERTICES, texture);
                index = runEnd;
            } else {
                index += 1;
            }
        }
        GLES20.glDisable(GLES20.GL_SCISSOR_TEST);
    }

    private void appendRect(int base) {
        int color = frame[base + 5];
        float red = ((color >> 16) & 255) / 255.0f;
        float green = ((color >> 8) & 255) / 255.0f;
        float blue = (color & 255) / 255.0f;
        float alpha = Math.max(0, Math.min(255, frame[base + 8])) / 255.0f;
        float left = frame[base + 1];
        float top = frame[base + 2];
        float right = left + frame[base + 3];
        float bottom = top + frame[base + 4];
        float centerX = (left + right) * 0.5f;
        float centerY = (top + bottom) * 0.5f;
        double radians = Math.toRadians(frame[base + 7] % 360);
        float cosine = (float)Math.cos(radians);
        float sine = (float)Math.sin(radians);
        putRectVertex(left, top, centerX, centerY, cosine, sine, red, green, blue, alpha);
        putRectVertex(right, top, centerX, centerY, cosine, sine, red, green, blue, alpha);
        putRectVertex(left, bottom, centerX, centerY, cosine, sine, red, green, blue, alpha);
        putRectVertex(right, top, centerX, centerY, cosine, sine, red, green, blue, alpha);
        putRectVertex(right, bottom, centerX, centerY, cosine, sine, red, green, blue, alpha);
        putRectVertex(left, bottom, centerX, centerY, cosine, sine, red, green, blue, alpha);
    }

    private void appendSprite(int base) {
        int color = frame[base + 5];
        float red = ((color >> 16) & 255) / 255.0f;
        float green = ((color >> 8) & 255) / 255.0f;
        float blue = (color & 255) / 255.0f;
        float alpha = Math.max(0, Math.min(255, frame[base + 8])) / 255.0f;
        float left = frame[base + 1];
        float top = frame[base + 2];
        float right = left + frame[base + 3];
        float bottom = top + frame[base + 4];
        float centerX = (left + right) * 0.5f;
        float centerY = (top + bottom) * 0.5f;
        double radians = Math.toRadians(frame[base + 7] % 360);
        float cosine = (float)Math.cos(radians);
        float sine = (float)Math.sin(radians);
        putSpriteVertex(left, top, centerX, centerY, cosine, sine, 0.0f, 0.0f, red, green, blue, alpha);
        putSpriteVertex(right, top, centerX, centerY, cosine, sine, 1.0f, 0.0f, red, green, blue, alpha);
        putSpriteVertex(left, bottom, centerX, centerY, cosine, sine, 0.0f, 1.0f, red, green, blue, alpha);
        putSpriteVertex(right, top, centerX, centerY, cosine, sine, 1.0f, 0.0f, red, green, blue, alpha);
        putSpriteVertex(right, bottom, centerX, centerY, cosine, sine, 1.0f, 1.0f, red, green, blue, alpha);
        putSpriteVertex(left, bottom, centerX, centerY, cosine, sine, 0.0f, 1.0f, red, green, blue, alpha);
    }

    private void putRectVertex(float x, float y, float centerX, float centerY,
            float cosine, float sine, float red, float green, float blue, float alpha) {
        float offsetX = x - centerX;
        float offsetY = y - centerY;
        rectVertices.put(centerX + offsetX * cosine - offsetY * sine)
                .put(centerY + offsetX * sine + offsetY * cosine)
                .put(red).put(green).put(blue).put(alpha);
    }

    private void putSpriteVertex(float x, float y, float centerX, float centerY,
            float cosine, float sine, float u, float v, float red, float green, float blue,
            float alpha) {
        float offsetX = x - centerX;
        float offsetY = y - centerY;
        spriteVertices.put(centerX + offsetX * cosine - offsetY * sine)
                .put(centerY + offsetX * sine + offsetY * cosine)
                .put(u).put(v).put(red).put(green).put(blue).put(alpha);
    }

    private boolean sameClip(int leftBase, int rightBase) {
        return frame[leftBase + 9] == frame[rightBase + 9]
                && frame[leftBase + 10] == frame[rightBase + 10]
                && frame[leftBase + 11] == frame[rightBase + 11]
                && frame[leftBase + 12] == frame[rightBase + 12];
    }

    private void applyClip(int base) {
        int width = frame[base + 11];
        int height = frame[base + 12];
        if (width <= 0 || height <= 0) {
            GLES20.glDisable(GLES20.GL_SCISSOR_TEST);
            return;
        }
        long sourceRight = (long)frame[base + 9] + width;
        long sourceBottom = (long)frame[base + 10] + height;
        int left = Math.max(0, Math.min(surfaceWidth, frame[base + 9]));
        int top = Math.max(0, Math.min(surfaceHeight, frame[base + 10]));
        int right = Math.max(left, (int)Math.max(0L, Math.min((long)surfaceWidth, sourceRight)));
        int bottom = Math.max(top, (int)Math.max(0L, Math.min((long)surfaceHeight, sourceBottom)));
        GLES20.glEnable(GLES20.GL_SCISSOR_TEST);
        GLES20.glScissor(left, surfaceHeight - bottom, right - left, bottom - top);
    }

    private void drawRectBatch(int vertexCount) {
        GLES20.glUseProgram(colorProgram);
        GLES20.glUniform2f(colorResolution, (float)surfaceWidth, (float)surfaceHeight);
        GLES20.glEnableVertexAttribArray(colorPosition);
        GLES20.glEnableVertexAttribArray(colorValue);
        rectVertices.position(0);
        GLES20.glVertexAttribPointer(colorPosition, 2, GLES20.GL_FLOAT, false,
                RECT_VERTEX_BYTES, rectVertices);
        rectVertices.position(2);
        GLES20.glVertexAttribPointer(colorValue, 4, GLES20.GL_FLOAT, false,
                RECT_VERTEX_BYTES, rectVertices);
        GLES20.glDrawArrays(GLES20.GL_TRIANGLES, 0, vertexCount);
        rectVertices.position(0);
        GLES20.glDisableVertexAttribArray(colorValue);
        GLES20.glDisableVertexAttribArray(colorPosition);
    }

    private void drawSpriteBatch(int vertexCount, int texture) {
        GLES20.glUseProgram(textureProgram);
        GLES20.glUniform2f(textureResolution, (float)surfaceWidth, (float)surfaceHeight);
        GLES20.glActiveTexture(GLES20.GL_TEXTURE0);
        GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, texture);
        GLES20.glUniform1i(textureSampler, 0);
        GLES20.glEnableVertexAttribArray(texturePosition);
        GLES20.glEnableVertexAttribArray(textureCoordinate);
        GLES20.glEnableVertexAttribArray(textureColor);
        spriteVertices.position(0);
        GLES20.glVertexAttribPointer(texturePosition, 2, GLES20.GL_FLOAT, false,
                SPRITE_VERTEX_BYTES, spriteVertices);
        spriteVertices.position(2);
        GLES20.glVertexAttribPointer(textureCoordinate, 2, GLES20.GL_FLOAT, false,
                SPRITE_VERTEX_BYTES, spriteVertices);
        spriteVertices.position(4);
        GLES20.glVertexAttribPointer(textureColor, 4, GLES20.GL_FLOAT, false,
                SPRITE_VERTEX_BYTES, spriteVertices);
        GLES20.glDrawArrays(GLES20.GL_TRIANGLES, 0, vertexCount);
        spriteVertices.position(0);
        GLES20.glDisableVertexAttribArray(textureColor);
        GLES20.glDisableVertexAttribArray(textureCoordinate);
        GLES20.glDisableVertexAttribArray(texturePosition);
        GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, 0);
    }

    private void captureIfRequested(CaptureCallback callback, int[] capturedFrame) {
        if (callback == null) return;
        try {
            long pixelCount = (long)surfaceWidth * surfaceHeight;
            if (pixelCount > MAX_CAPTURE_PIXELS) {
                callback.onCaptured(null, "preview framebuffer exceeds the 8 megapixel capture limit", capturedFrame);
                return;
            }
            java.nio.IntBuffer pixels = ByteBuffer.allocateDirect(surfaceWidth * surfaceHeight * 4)
                    .order(ByteOrder.nativeOrder()).asIntBuffer();
            GLES20.glReadPixels(0, 0, surfaceWidth, surfaceHeight, GLES20.GL_RGBA,
                    GLES20.GL_UNSIGNED_BYTE, pixels);
            int[] flipped = new int[surfaceWidth * surfaceHeight];
            for (int y = 0; y < surfaceHeight; y++) {
                int sourceRow = y * surfaceWidth;
                int targetRow = (surfaceHeight - y - 1) * surfaceWidth;
                for (int x = 0; x < surfaceWidth; x++) {
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
