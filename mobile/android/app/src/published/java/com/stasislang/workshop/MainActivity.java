package com.stasislang.workshop;

import android.app.Activity;
import android.graphics.Color;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.Rect;
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
import java.io.IOException;
import java.util.HashMap;

public final class MainActivity extends Activity {
    private static final String PUBLISHED_RUNTIME_ID = BuildConfig.STASIS_RUNTIME_ID;

    private static final long FRAME_DELAY_MS = 16L;
    private static final double FRAME_BUDGET_MILLIS = 1000.0 / 60.0;
    private static final long DEBUG_UPDATE_INTERVAL_NANOS = 200_000_000L;
    private static final int MAX_RENDER_COMMANDS = 64;
    private static final int RENDER_FRAME_HEADER_SIZE = 6;
    private static final int RENDER_COMMAND_STRIDE = 7;
    private static final int RENDER_FRAME_I32_CAPACITY = RENDER_FRAME_HEADER_SIZE + MAX_RENDER_COMMANDS * RENDER_COMMAND_STRIDE;
    private static final int RECT_VERTICES = 6;
    private static final int RECT_VERTEX_FLOATS = 6;
    private static final int RENDER_VERTEX_BUFFER_FLOATS = MAX_RENDER_COMMANDS * RECT_VERTICES * RECT_VERTEX_FLOATS;

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

    private static native String nativeCompileProject(String projectRoot);
    private static native int nativeRunFrameInto(String projectRoot, int touchX, int touchY, int touchActive, int screenWidth, int screenHeight, int[] frameValues);
    private static native String nativePublishedSpritePath(int handle);
    private static native String nativePublishedTextForRun(int runHandle);

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
        startFrameLoop();
    }

    @Override
    protected void onDestroy() {
        if (frameLoop != null) {
            frameHandler.removeCallbacks(frameLoop);
        }
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

    private static final class GameSurfaceView extends View {
        private final MainActivity activity;
        private final int[] frameValues = new int[RENDER_FRAME_I32_CAPACITY];
        private final Paint paint = new Paint(Paint.FILTER_BITMAP_FLAG | Paint.ANTI_ALIAS_FLAG);
        private final HashMap<Integer, Bitmap> sprites = new HashMap<>();
        private final HashMap<Integer, String> textRuns = new HashMap<>();
        private int touchX;
        private int touchY;
        private boolean touchActive;

        GameSurfaceView(MainActivity activity) {
            super(activity);
            this.activity = activity;
            setBackgroundColor(Color.rgb(15, 20, 28));
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
            System.arraycopy(values, 0, frameValues, 0, RENDER_FRAME_I32_CAPACITY);
            invalidate();
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

        @Override
        protected void onDraw(Canvas canvas) {
            long start = System.nanoTime();
            super.onDraw(canvas);
            int commandCount = Math.max(0, Math.min(MAX_RENDER_COMMANDS, frameValues[5]));
            for (int index = 0; index < commandCount; index += 1) {
                int base = RENDER_FRAME_HEADER_SIZE + index * RENDER_COMMAND_STRIDE;
                int kind = frameValues[base];
                int x = frameValues[base + 1];
                int y = frameValues[base + 2];
                int w = Math.max(1, frameValues[base + 3]);
                int h = Math.max(1, frameValues[base + 4]);
                if (kind == 1) {
                    Bitmap sprite = spriteFor(frameValues[base + 6]);
                    if (sprite != null) {
                        canvas.drawBitmap(sprite, null, new Rect(x, y, x + w, y + h), paint);
                        continue;
                    }
                }
                if (kind == 3) {
                    String text = textFor(frameValues[base + 6]);
                    if (text != null) {
                        paint.setColor(0xff000000 | frameValues[base + 5]);
                        paint.setTextSize(20.0f);
                        canvas.drawText(text, x, y + 20, paint);
                    }
                    continue;
                }
                if (kind == 1 || kind == 2) {
                    paint.setColor(0xff000000 | frameValues[base + 5]);
                    canvas.drawRect(x, y, x + w, y + h, paint);
                }
            }
            activity.recordRenderTimeNanos(System.nanoTime() - start);
        }

        private Bitmap spriteFor(int handle) {
            if (handle <= 0) {
                return null;
            }
            if (sprites.containsKey(handle)) {
                return sprites.get(handle);
            }
            String path = nativePublishedSpritePath(handle);
            Bitmap sprite = null;
            if (path != null) {
                try {
                    sprite = BitmapFactory.decodeStream(activity.getAssets().open(path));
                } catch (IOException ignored) {
                }
            }
            sprites.put(handle, sprite);
            return sprite;
        }

        private String textFor(int handle) {
            if (handle <= 0) {
                return null;
            }
            if (textRuns.containsKey(handle)) {
                return textRuns.get(handle);
            }
            String text = nativePublishedTextForRun(handle);
            textRuns.put(handle, text);
            return text;
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

        private final MainActivity activity;
        private final FloatBuffer vertexBuffer = ByteBuffer
                .allocateDirect(RENDER_VERTEX_BUFFER_FLOATS * 4)
                .order(ByteOrder.nativeOrder())
                .asFloatBuffer();
        private final int[] frameValues = new int[RENDER_FRAME_I32_CAPACITY];
        private int program;
        private int positionHandle;
        private int colorHandle;
        private int resolutionHandle;
        private int surfaceWidth = 1;
        private int surfaceHeight = 1;

        PreviewRenderer(MainActivity activity) {
            this.activity = activity;
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
            vertexBuffer.clear();
            int vertexCount = 0;
            synchronized (this) {
                int commandCount = Math.max(0, Math.min(MAX_RENDER_COMMANDS, frameValues[5]));
                for (int index = 0; index < commandCount; index += 1) {
                    int base = RENDER_FRAME_HEADER_SIZE + index * RENDER_COMMAND_STRIDE;
                    int kind = frameValues[base];
                    if (kind == 1 || kind == 2) {
                        appendRect(base);
                        vertexCount += RECT_VERTICES;
                    }
                }
            }
            vertexBuffer.flip();
            if (vertexCount > 0) {
                GLES20.glEnableVertexAttribArray(positionHandle);
                GLES20.glEnableVertexAttribArray(colorHandle);
                vertexBuffer.position(0);
                GLES20.glVertexAttribPointer(positionHandle, 2, GLES20.GL_FLOAT, false, RECT_VERTEX_FLOATS * 4, vertexBuffer);
                vertexBuffer.position(2);
                GLES20.glVertexAttribPointer(colorHandle, 4, GLES20.GL_FLOAT, false, RECT_VERTEX_FLOATS * 4, vertexBuffer);
                GLES20.glDrawArrays(GLES20.GL_TRIANGLES, 0, vertexCount);
                GLES20.glDisableVertexAttribArray(positionHandle);
                GLES20.glDisableVertexAttribArray(colorHandle);
            }
            activity.recordRenderTimeNanos(System.nanoTime() - start);
        }

        private void appendRect(int base) {
            int color = frameValues[base + 5];
            float red = ((color >> 16) & 255) / 255.0f;
            float green = ((color >> 8) & 255) / 255.0f;
            float blue = (color & 255) / 255.0f;
            float left = frameValues[base + 1];
            float top = frameValues[base + 2];
            float right = frameValues[base + 1] + frameValues[base + 3];
            float bottom = frameValues[base + 2] + frameValues[base + 4];
            putVertex(left, top, red, green, blue);
            putVertex(right, top, red, green, blue);
            putVertex(left, bottom, red, green, blue);
            putVertex(right, top, red, green, blue);
            putVertex(right, bottom, red, green, blue);
            putVertex(left, bottom, red, green, blue);
        }

        private void putVertex(float x, float y, float red, float green, float blue) {
            vertexBuffer.put(x).put(y).put(red).put(green).put(blue).put(1.0f);
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
