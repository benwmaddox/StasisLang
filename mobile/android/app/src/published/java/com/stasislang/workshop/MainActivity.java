package com.stasislang.workshop;

import android.app.Activity;
import android.graphics.Color;
import android.opengl.GLES20;
import android.opengl.GLSurfaceView;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.view.DisplayCutout;
import android.view.Gravity;
import android.view.MotionEvent;
import android.view.View;
import android.view.Window;
import android.view.WindowInsets;
import android.widget.FrameLayout;
import android.widget.TextView;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.FloatBuffer;

public final class MainActivity extends Activity {
    private static final String PUBLISHED_RUNTIME_ID = BuildConfig.STASIS_RUNTIME_ID;

    private static final long FRAME_DELAY_MS = 16L;
    private static final double FRAME_BUDGET_MILLIS = 1000.0 / 60.0;
    private static final long DEBUG_UPDATE_INTERVAL_NANOS = 200_000_000L;
    private static final int MAX_RENDER_COMMANDS = 8;
    private static final int RENDER_FRAME_HEADER_SIZE = 6;
    private static final int RENDER_COMMAND_STRIDE = 13;
    private static final int RENDER_FRAME_I32_CAPACITY = RENDER_FRAME_HEADER_SIZE + MAX_RENDER_COMMANDS * RENDER_COMMAND_STRIDE;
    private static final int RECT_VERTICES = 6;
    private static final int RECT_VERTEX_FLOATS = 6;
    private static final int RENDER_VERTEX_BUFFER_FLOATS = MAX_RENDER_COMMANDS * RECT_VERTICES * RECT_VERTEX_FLOATS;
    private static final int SPRITE_VERTEX_FLOATS = 8;
    private static final int SPRITE_VERTEX_BYTES = SPRITE_VERTEX_FLOATS * 4;
    private static final int SPRITE_VERTEX_BUFFER_FLOATS = MAX_RENDER_COMMANDS * RECT_VERTICES * SPRITE_VERTEX_FLOATS;

    static {
        System.loadLibrary("stasis_mobile_smoke");
    }

    private final Handler frameHandler = new Handler(Looper.getMainLooper());
    private final int[] frameValues = new int[RENDER_FRAME_I32_CAPACITY];
    private final RollingMetric tickMetric = new RollingMetric();
    private final RollingMetric renderMetric = new RollingMetric();
    private final StringBuilder hudText = new StringBuilder(80);
    private GameSurfaceView gameSurface;
    private TextView hud;
    private Runnable frameLoop;
    private boolean compileAttempted;
    private boolean compileReady;
    private long lastHudUpdateNanos;
    private AndroidAudioFocus audioFocus;

    private static native String nativeCompileProject(String projectRoot);
    private static native int nativeRunFrameInto(String projectRoot, int touchX, int touchY, int touchActive, int screenWidth, int screenHeight, int[] frameValues);
    private static native void nativeAudioSetPaused(boolean paused);
    private static native void nativeAudioSetFocus(boolean focused);
    private static native void nativeAudioShutdown();
    private static native boolean nativeAudioRequested();
    private static native int[] nativeAudioMetrics();
    static native int[] nativeDecodeSvgSpriteBytes(byte[] bytes, int width, int height);

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        Window window = getWindow();
        window.setStatusBarColor(Color.BLACK);
        window.setNavigationBarColor(Color.BLACK);

        FrameLayout root = new FrameLayout(this);
        root.setBackgroundColor(Color.BLACK);
        installSystemInsetGuard(root);

        gameSurface = new GameSurfaceView(this);
        root.addView(gameSurface, new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT,
                FrameLayout.LayoutParams.MATCH_PARENT));

        hud = new TextView(this);
        hud.setTextColor(Color.WHITE);
        hud.setTextSize(12.0f);
        hud.setSingleLine(true);
        hud.setPadding(dp(10), dp(6), dp(10), dp(6));
        hud.setBackgroundColor(Color.argb(135, 20, 28, 38));
        hud.setVisibility(View.GONE);
        FrameLayout.LayoutParams hudParams = new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.WRAP_CONTENT,
                FrameLayout.LayoutParams.WRAP_CONTENT,
                Gravity.TOP | Gravity.START);
        hudParams.setMargins(dp(8), dp(8), dp(8), 0);
        root.addView(hud, hudParams);

        setContentView(root);
        audioFocus = new AndroidAudioFocus(this, MainActivity::nativeAudioSetFocus);
        startFrameLoop();
    }

    @Override
    protected void onResume() {
        super.onResume();
        nativeAudioSetPaused(false);
    }

    @Override
    protected void onPause() {
        if (audioFocus != null) audioFocus.pause();
        nativeAudioSetPaused(true);
        super.onPause();
    }

    @Override
    protected void onDestroy() {
        if (frameLoop != null) {
            frameHandler.removeCallbacks(frameLoop);
        }
        if (audioFocus != null) audioFocus.pause();
        nativeAudioShutdown();
        super.onDestroy();
    }

    void toggleHud() {
        if (hud == null) {
            return;
        }
        hud.setVisibility(hud.getVisibility() == View.VISIBLE ? View.GONE : View.VISIBLE);
        updateHud(true);
    }

    void recordRenderTimeNanos(long durationNanos) {
        renderMetric.add(System.nanoTime(), durationNanos);
    }

    private void startFrameLoop() {
        if (frameLoop != null) {
            return;
        }
        frameLoop = new Runnable() {
            @Override
            public void run() {
                if (!compileAttempted) {
                    String result = nativeCompileProject(PUBLISHED_RUNTIME_ID);
                    compileReady = result != null && result.startsWith("CompilePlanned") && result.contains("status=0");
                    compileAttempted = true;
                }
                if (compileReady) {
                    runFrame();
                }
                frameHandler.postDelayed(this, FRAME_DELAY_MS);
            }
        };
        frameHandler.post(frameLoop);
    }

    private void runFrame() {
        int width = gameSurface == null ? 0 : gameSurface.getWidth();
        int height = gameSurface == null ? 0 : gameSurface.getHeight();
        long start = System.nanoTime();
        int status = nativeRunFrameInto(
                PUBLISHED_RUNTIME_ID,
                gameSurface == null ? 0 : gameSurface.touchX(),
                gameSurface == null ? 0 : gameSurface.touchY(),
                gameSurface == null ? 0 : gameSurface.touchActive(),
                width,
                height,
                frameValues);
        if (audioFocus != null && nativeAudioRequested()) audioFocus.resume();
        tickMetric.add(System.nanoTime(), System.nanoTime() - start);
        if (status == 0 && frameValues[0] == 0 && gameSurface != null) {
            gameSurface.setRenderFrameValues(frameValues);
        }
        updateHud(false);
    }

    private void updateHud(boolean force) {
        if (hud == null || hud.getVisibility() != View.VISIBLE) {
            return;
        }
        long now = System.nanoTime();
        if (!force && now - lastHudUpdateNanos < DEBUG_UPDATE_INTERVAL_NANOS) {
            return;
        }
        lastHudUpdateNanos = now;
        double tickMillis = tickMetric.averageMillis();
        double renderMillis = renderMetric.averageMillis();
        int budgetPercent = Math.max(0, (int)(((tickMillis + renderMillis) * 100.0 / FRAME_BUDGET_MILLIS) + 0.5));
        hudText.setLength(0);
        hudText.append("tick=");
        appendMillis(hudText, tickMillis);
        hudText.append(" ms  render=");
        appendMillis(hudText, renderMillis);
        hudText.append(" ms  budget=").append(budgetPercent).append('%');
        int[] audio = nativeAudioMetrics();
        if (audio != null && audio.length >= 6) {
            hudText.append("  audio=").append(audio[0] != 0 ? "on" : "off")
                    .append(" q=").append(audio[3])
                    .append(" u=").append(audio[4]);
            if (audio[5] != 0) hudText.append(" err=").append(audio[5]);
        }
        hud.setTextColor(debugColorForBudget(budgetPercent));
        hud.setText(hudText.toString());
    }

    private static int debugColorForBudget(int budgetPercent) {
        if (budgetPercent >= 100) {
            return Color.rgb(186, 104, 255);
        }
        if (budgetPercent >= 80) {
            return Color.rgb(255, 91, 91);
        }
        if (budgetPercent >= 50) {
            return Color.rgb(255, 214, 102);
        }
        return Color.WHITE;
    }

    private static void appendMillis(StringBuilder builder, double millis) {
        int hundredths = Math.max(0, (int)(millis * 100.0 + 0.5));
        builder.append(hundredths / 100).append('.');
        int fraction = hundredths % 100;
        if (fraction < 10) {
            builder.append('0');
        }
        builder.append(fraction);
    }

    private void installSystemInsetGuard(final View root) {
        root.setOnApplyWindowInsetsListener(new View.OnApplyWindowInsetsListener() {
            @Override
            public WindowInsets onApplyWindowInsets(View view, WindowInsets insets) {
                int left = insets.getSystemWindowInsetLeft();
                int top = insets.getSystemWindowInsetTop();
                int right = insets.getSystemWindowInsetRight();
                int bottom = insets.getSystemWindowInsetBottom();
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                    DisplayCutout cutout = insets.getDisplayCutout();
                    if (cutout != null) {
                        left = Math.max(left, cutout.getSafeInsetLeft());
                        top = Math.max(top, cutout.getSafeInsetTop());
                        right = Math.max(right, cutout.getSafeInsetRight());
                        bottom = Math.max(bottom, cutout.getSafeInsetBottom());
                    }
                }
                view.setPadding(left, top, right, bottom);
                return insets;
            }
        });
        root.requestApplyInsets();
    }

    private int dp(int value) {
        return Math.round(value * getResources().getDisplayMetrics().density);
    }

    private static final class GameSurfaceView extends GLSurfaceView {
        private final MainActivity activity;
        private final PreviewRenderer renderer;
        private int touchX;
        private int touchY;
        private boolean touchActive;

        GameSurfaceView(MainActivity activity) {
            super(activity);
            this.activity = activity;
            setEGLContextClientVersion(2);
            renderer = new PreviewRenderer(activity);
            setRenderer(renderer);
            setRenderMode(GLSurfaceView.RENDERMODE_WHEN_DIRTY);
        }

        int touchX() {
            return touchX;
        }

        int touchY() {
            return touchY;
        }

        int touchActive() {
            return touchActive ? 1 : 0;
        }

        void setRenderFrameValues(int[] values) {
            renderer.setFrameValues(values);
            requestRender();
        }

        @Override
        public boolean onTouchEvent(MotionEvent event) {
            touchX = Math.round(event.getX());
            touchY = Math.round(event.getY());
            int action = event.getActionMasked();
            if (action == MotionEvent.ACTION_POINTER_DOWN && event.getPointerCount() >= 3) {
                activity.toggleHud();
            }
            touchActive = action != MotionEvent.ACTION_UP && action != MotionEvent.ACTION_CANCEL;
            return true;
        }
    }

    private static final class PreviewRenderer implements GLSurfaceView.Renderer {
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
                "void main() {" +
                "  gl_FragColor = vColor;" +
                "}";
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

        private final MainActivity activity;
        private final FloatBuffer vertexBuffer = ByteBuffer
                .allocateDirect(RENDER_VERTEX_BUFFER_FLOATS * 4)
                .order(ByteOrder.nativeOrder())
                .asFloatBuffer();
        private final FloatBuffer spriteVertexBuffer = ByteBuffer
                .allocateDirect(SPRITE_VERTEX_BUFFER_FLOATS * 4)
                .order(ByteOrder.nativeOrder())
                .asFloatBuffer();
        private final int[] frameValues = new int[RENDER_FRAME_I32_CAPACITY];
        private final PublishedSpriteCatalog spriteCatalog;
        private int program;
        private int positionHandle;
        private int colorHandle;
        private int resolutionHandle;
        private int textureProgram;
        private int texturePositionHandle;
        private int textureCoordHandle;
        private int textureColorHandle;
        private int textureResolutionHandle;
        private int textureSamplerHandle;
        private int surfaceWidth = 1;
        private int surfaceHeight = 1;

        PreviewRenderer(MainActivity activity) {
            this.activity = activity;
            spriteCatalog = new PublishedSpriteCatalog(activity.getAssets());
        }

        synchronized void setFrameValues(int[] values) {
            System.arraycopy(values, 0, frameValues, 0, RENDER_FRAME_I32_CAPACITY);
        }

        @Override
        public void onSurfaceCreated(javax.microedition.khronos.opengles.GL10 gl, javax.microedition.khronos.egl.EGLConfig config) {
            program = createProgram(VERTEX_SHADER, FRAGMENT_SHADER);
            positionHandle = GLES20.glGetAttribLocation(program, "aPosition");
            colorHandle = GLES20.glGetAttribLocation(program, "aColor");
            resolutionHandle = GLES20.glGetUniformLocation(program, "uResolution");
            textureProgram = createProgram(TEXTURE_VERTEX_SHADER, TEXTURE_FRAGMENT_SHADER);
            texturePositionHandle = GLES20.glGetAttribLocation(textureProgram, "aPosition");
            textureCoordHandle = GLES20.glGetAttribLocation(textureProgram, "aTexCoord");
            textureColorHandle = GLES20.glGetAttribLocation(textureProgram, "aColor");
            textureResolutionHandle = GLES20.glGetUniformLocation(textureProgram, "uResolution");
            textureSamplerHandle = GLES20.glGetUniformLocation(textureProgram, "uTexture");
            spriteCatalog.onSurfaceCreated();
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
            long start = System.nanoTime();
            GLES20.glClear(GLES20.GL_COLOR_BUFFER_BIT);
            GLES20.glUseProgram(program);
            GLES20.glUniform2f(resolutionHandle, (float)surfaceWidth, (float)surfaceHeight);
            synchronized (this) {
                int commandCount = Math.max(0, Math.min(MAX_RENDER_COMMANDS, frameValues[5]));
                int index = 0;
                while (index < commandCount) {
                    int base = RENDER_FRAME_HEADER_SIZE + index * RENDER_COMMAND_STRIDE;
                    int kind = frameValues[base];
                    if (kind == 1) {
                        vertexBuffer.clear();
                        int runEnd = index;
                        while (runEnd < commandCount) {
                            int runBase = RENDER_FRAME_HEADER_SIZE + runEnd * RENDER_COMMAND_STRIDE;
                            int runKind = frameValues[runBase];
                            if (runKind != 1 || !sameClip(base, runBase)) break;
                            appendRect(runBase);
                            runEnd += 1;
                        }
                        vertexBuffer.flip();
                        applyClip(base);
                        drawBatch((runEnd - index) * RECT_VERTICES);
                        index = runEnd;
                    } else if (kind == 2) {
                        int texture = spriteCatalog.textureFor(frameValues[base + 6]);
                        spriteVertexBuffer.clear();
                        int runEnd = index;
                        while (runEnd < commandCount) {
                            int runBase = RENDER_FRAME_HEADER_SIZE + runEnd * RENDER_COMMAND_STRIDE;
                            if (frameValues[runBase] != 2 || !sameClip(base, runBase)) break;
                            int runTexture = spriteCatalog.textureFor(frameValues[runBase + 6]);
                            if (runTexture != texture) break;
                            appendSprite(runBase);
                            runEnd += 1;
                        }
                        spriteVertexBuffer.flip();
                        applyClip(base);
                        drawSpriteBatch((runEnd - index) * RECT_VERTICES, texture);
                        index = runEnd;
                    } else {
                        index += 1;
                    }
                }
                GLES20.glDisable(GLES20.GL_SCISSOR_TEST);
            }
            activity.recordRenderTimeNanos(System.nanoTime() - start);
        }

        private void drawBatch(int vertexCount) {
            GLES20.glUseProgram(program);
            GLES20.glUniform2f(resolutionHandle, (float)surfaceWidth, (float)surfaceHeight);
            GLES20.glEnableVertexAttribArray(positionHandle);
            GLES20.glEnableVertexAttribArray(colorHandle);
            vertexBuffer.position(0);
            GLES20.glVertexAttribPointer(positionHandle, 2, GLES20.GL_FLOAT, false,
                    RECT_VERTEX_FLOATS * 4, vertexBuffer);
            vertexBuffer.position(2);
            GLES20.glVertexAttribPointer(colorHandle, 4, GLES20.GL_FLOAT, false,
                    RECT_VERTEX_FLOATS * 4, vertexBuffer);
            GLES20.glDrawArrays(GLES20.GL_TRIANGLES, 0, vertexCount);
            GLES20.glDisableVertexAttribArray(positionHandle);
            GLES20.glDisableVertexAttribArray(colorHandle);
        }

        private void appendSprite(int base) {
            int color = frameValues[base + 5];
            float red = ((color >> 16) & 255) / 255.0f;
            float green = ((color >> 8) & 255) / 255.0f;
            float blue = (color & 255) / 255.0f;
            float alpha = Math.max(0, Math.min(255, frameValues[base + 8])) / 255.0f;
            float left = frameValues[base + 1];
            float top = frameValues[base + 2];
            float right = left + frameValues[base + 3];
            float bottom = top + frameValues[base + 4];
            float centerX = (left + right) * 0.5f;
            float centerY = (top + bottom) * 0.5f;
            double radians = Math.toRadians(frameValues[base + 7] % 360);
            float cosine = (float)Math.cos(radians);
            float sine = (float)Math.sin(radians);
            putSpriteVertex(left, top, centerX, centerY, cosine, sine, 0.0f, 0.0f, red, green, blue, alpha);
            putSpriteVertex(right, top, centerX, centerY, cosine, sine, 1.0f, 0.0f, red, green, blue, alpha);
            putSpriteVertex(left, bottom, centerX, centerY, cosine, sine, 0.0f, 1.0f, red, green, blue, alpha);
            putSpriteVertex(right, top, centerX, centerY, cosine, sine, 1.0f, 0.0f, red, green, blue, alpha);
            putSpriteVertex(right, bottom, centerX, centerY, cosine, sine, 1.0f, 1.0f, red, green, blue, alpha);
            putSpriteVertex(left, bottom, centerX, centerY, cosine, sine, 0.0f, 1.0f, red, green, blue, alpha);
        }

        private void putSpriteVertex(float x, float y, float centerX, float centerY,
                float cosine, float sine, float u, float v, float red, float green, float blue,
                float alpha) {
            float offsetX = x - centerX;
            float offsetY = y - centerY;
            float rotatedX = centerX + offsetX * cosine - offsetY * sine;
            float rotatedY = centerY + offsetX * sine + offsetY * cosine;
            spriteVertexBuffer.put(rotatedX).put(rotatedY).put(u).put(v)
                    .put(red).put(green).put(blue).put(alpha);
        }

        private void drawSpriteBatch(int vertexCount, int texture) {
            GLES20.glUseProgram(textureProgram);
            GLES20.glUniform2f(textureResolutionHandle, (float)surfaceWidth, (float)surfaceHeight);
            GLES20.glActiveTexture(GLES20.GL_TEXTURE0);
            GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, texture);
            GLES20.glUniform1i(textureSamplerHandle, 0);
            GLES20.glEnableVertexAttribArray(texturePositionHandle);
            GLES20.glEnableVertexAttribArray(textureCoordHandle);
            GLES20.glEnableVertexAttribArray(textureColorHandle);
            spriteVertexBuffer.position(0);
            GLES20.glVertexAttribPointer(texturePositionHandle, 2, GLES20.GL_FLOAT, false,
                    SPRITE_VERTEX_BYTES, spriteVertexBuffer);
            spriteVertexBuffer.position(2);
            GLES20.glVertexAttribPointer(textureCoordHandle, 2, GLES20.GL_FLOAT, false,
                    SPRITE_VERTEX_BYTES, spriteVertexBuffer);
            spriteVertexBuffer.position(4);
            GLES20.glVertexAttribPointer(textureColorHandle, 4, GLES20.GL_FLOAT, false,
                    SPRITE_VERTEX_BYTES, spriteVertexBuffer);
            GLES20.glDrawArrays(GLES20.GL_TRIANGLES, 0, vertexCount);
            GLES20.glDisableVertexAttribArray(textureColorHandle);
            GLES20.glDisableVertexAttribArray(textureCoordHandle);
            GLES20.glDisableVertexAttribArray(texturePositionHandle);
            GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, 0);
        }

        private void appendRect(int base) {
            int color = frameValues[base + 5];
            float red = ((color >> 16) & 255) / 255.0f;
            float green = ((color >> 8) & 255) / 255.0f;
            float blue = (color & 255) / 255.0f;
            float alpha = Math.max(0, Math.min(255, frameValues[base + 8])) / 255.0f;
            float left = frameValues[base + 1];
            float top = frameValues[base + 2];
            float right = frameValues[base + 1] + frameValues[base + 3];
            float bottom = frameValues[base + 2] + frameValues[base + 4];
            float centerX = (left + right) * 0.5f;
            float centerY = (top + bottom) * 0.5f;
            double radians = Math.toRadians(frameValues[base + 7] % 360);
            float cosine = (float)Math.cos(radians);
            float sine = (float)Math.sin(radians);
            putVertex(left, top, centerX, centerY, cosine, sine, red, green, blue, alpha);
            putVertex(right, top, centerX, centerY, cosine, sine, red, green, blue, alpha);
            putVertex(left, bottom, centerX, centerY, cosine, sine, red, green, blue, alpha);
            putVertex(right, top, centerX, centerY, cosine, sine, red, green, blue, alpha);
            putVertex(right, bottom, centerX, centerY, cosine, sine, red, green, blue, alpha);
            putVertex(left, bottom, centerX, centerY, cosine, sine, red, green, blue, alpha);
        }

        private boolean sameClip(int leftBase, int rightBase) {
            return frameValues[leftBase + 9] == frameValues[rightBase + 9]
                    && frameValues[leftBase + 10] == frameValues[rightBase + 10]
                    && frameValues[leftBase + 11] == frameValues[rightBase + 11]
                    && frameValues[leftBase + 12] == frameValues[rightBase + 12];
        }

        private void applyClip(int base) {
            int width = frameValues[base + 11];
            int height = frameValues[base + 12];
            if (width <= 0 || height <= 0) {
                GLES20.glDisable(GLES20.GL_SCISSOR_TEST);
                return;
            }
            long sourceRight = (long)frameValues[base + 9] + width;
            long sourceBottom = (long)frameValues[base + 10] + height;
            int left = Math.max(0, Math.min(surfaceWidth, frameValues[base + 9]));
            int top = Math.max(0, Math.min(surfaceHeight, frameValues[base + 10]));
            int right = Math.max(left, (int)Math.max(0L, Math.min((long)surfaceWidth, sourceRight)));
            int bottom = Math.max(top, (int)Math.max(0L, Math.min((long)surfaceHeight, sourceBottom)));
            GLES20.glEnable(GLES20.GL_SCISSOR_TEST);
            GLES20.glScissor(left, surfaceHeight - bottom, right - left, bottom - top);
        }

        private void putVertex(float x, float y, float centerX, float centerY,
                float cosine, float sine, float red, float green, float blue, float alpha) {
            float offsetX = x - centerX;
            float offsetY = y - centerY;
            float rotatedX = centerX + offsetX * cosine - offsetY * sine;
            float rotatedY = centerY + offsetX * sine + offsetY * cosine;
            vertexBuffer.put(rotatedX).put(rotatedY).put(red).put(green).put(blue).put(alpha);
        }

        private static int createProgram(String vertexSource, String fragmentSource) {
            int vertexShader = loadShader(GLES20.GL_VERTEX_SHADER, vertexSource);
            int fragmentShader = loadShader(GLES20.GL_FRAGMENT_SHADER, fragmentSource);
            int program = GLES20.glCreateProgram();
            GLES20.glAttachShader(program, vertexShader);
            GLES20.glAttachShader(program, fragmentShader);
            GLES20.glLinkProgram(program);
            GLES20.glDeleteShader(vertexShader);
            GLES20.glDeleteShader(fragmentShader);
            return program;
        }

        private static int loadShader(int type, String source) {
            int shader = GLES20.glCreateShader(type);
            GLES20.glShaderSource(shader, source);
            GLES20.glCompileShader(shader);
            return shader;
        }
    }

    private static final class RollingMetric {
        private static final long WINDOW_NANOS = 5_000_000_000L;
        private static final int CAPACITY = 360;
        private final long[] sampleTimes = new long[CAPACITY];
        private final long[] durations = new long[CAPACITY];
        private int nextIndex;
        private int count;

        void add(long now, long durationNanos) {
            sampleTimes[nextIndex] = now;
            durations[nextIndex] = durationNanos;
            nextIndex = (nextIndex + 1) % CAPACITY;
            if (count < CAPACITY) {
                count += 1;
            }
        }

        double averageMillis() {
            if (count == 0) {
                return 0.0;
            }
            long now = System.nanoTime();
            long total = 0L;
            int samples = 0;
            for (int index = 0; index < count; index += 1) {
                if (now - sampleTimes[index] <= WINDOW_NANOS) {
                    total += durations[index];
                    samples += 1;
                }
            }
            if (samples == 0) {
                return 0.0;
            }
            return (double)total / (double)samples / 1_000_000.0;
        }
    }
}
