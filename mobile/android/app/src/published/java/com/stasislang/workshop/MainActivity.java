package com.stasislang.workshop;

import android.app.Activity;
import android.graphics.Color;
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

public final class MainActivity extends Activity {
    private static final String PUBLISHED_RUNTIME_ID = BuildConfig.STASIS_RUNTIME_ID;
    private static final long FRAME_DELAY_MS = 16L;
    private static final double FRAME_BUDGET_MILLIS = 1000.0 / 60.0;
    private static final long DEBUG_UPDATE_INTERVAL_NANOS = 200_000_000L;

    static {
        System.loadLibrary("stasis_mobile_smoke");
    }

    private final Handler frameHandler = new Handler(Looper.getMainLooper());
    private final int[] frameValues = new int[StasisPreviewRenderer.FRAME_I32_CAPACITY];
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
    private static native int nativeRunFrameInto(String projectRoot, int touchX, int touchY,
            int touchActive, int screenWidth, int screenHeight, int[] frameValues);
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
        startFrameLoop();
    }

    @Override
    protected void onDestroy() {
        if (frameLoop != null) frameHandler.removeCallbacks(frameLoop);
        super.onDestroy();
    }

    private void toggleHud() {
        if (hud == null) return;
        hud.setVisibility(hud.getVisibility() == View.VISIBLE ? View.GONE : View.VISIBLE);
        updateHud(true);
    }

    private void recordRenderTimeNanos(long durationNanos) {
        renderMetric.add(System.nanoTime(), durationNanos);
    }

    private void startFrameLoop() {
        if (frameLoop != null) return;
        frameLoop = new Runnable() {
            @Override
            public void run() {
                if (!compileAttempted) {
                    String result = nativeCompileProject(PUBLISHED_RUNTIME_ID);
                    compileReady = result != null && result.startsWith("CompilePlanned")
                            && result.contains("status=0");
                    compileAttempted = true;
                }
                if (compileReady) runFrame();
                frameHandler.postDelayed(this, FRAME_DELAY_MS);
            }
        };
        frameHandler.post(frameLoop);
    }

    private void runFrame() {
        int width = gameSurface == null ? 0 : gameSurface.getWidth();
        int height = gameSurface == null ? 0 : gameSurface.getHeight();
        long started = System.nanoTime();
        int status = nativeRunFrameInto(
                PUBLISHED_RUNTIME_ID,
                gameSurface == null ? 0 : gameSurface.touchX(),
                gameSurface == null ? 0 : gameSurface.touchY(),
                gameSurface == null ? 0 : gameSurface.touchActive(),
                width,
                height,
                frameValues);
        tickMetric.add(System.nanoTime(), System.nanoTime() - started);
        if (status == 0 && frameValues[0] == 0 && gameSurface != null) {
            gameSurface.setRenderFrameValues(frameValues);
        }
        updateHud(false);
    }

    private void updateHud(boolean force) {
        if (hud == null || hud.getVisibility() != View.VISIBLE) return;
        long now = System.nanoTime();
        if (!force && now - lastHudUpdateNanos < DEBUG_UPDATE_INTERVAL_NANOS) return;
        lastHudUpdateNanos = now;
        double tickMillis = tickMetric.averageMillis();
        double renderMillis = renderMetric.averageMillis();
        int budgetPercent = Math.max(0,
                (int)(((tickMillis + renderMillis) * 100.0 / FRAME_BUDGET_MILLIS) + 0.5));
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
        if (budgetPercent >= 100) return Color.rgb(186, 104, 255);
        if (budgetPercent >= 80) return Color.rgb(255, 91, 91);
        if (budgetPercent >= 50) return Color.rgb(255, 214, 102);
        return Color.WHITE;
    }

    private static void appendMillis(StringBuilder builder, double millis) {
        int hundredths = Math.max(0, (int)(millis * 100.0 + 0.5));
        builder.append(hundredths / 100).append('.');
        int fraction = hundredths % 100;
        if (fraction < 10) builder.append('0');
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

    private static final class GameSurfaceView extends android.opengl.GLSurfaceView {
        private final MainActivity activity;
        private final StasisPreviewRenderer renderer;
        private int touchX;
        private int touchY;
        private boolean touchActive;

        GameSurfaceView(MainActivity activity) {
            super(activity);
            this.activity = activity;
            setEGLContextClientVersion(2);
            renderer = new StasisPreviewRenderer(
                    new PublishedSpriteCatalog(activity.getAssets()),
                    activity::recordRenderTimeNanos);
            setRenderer(renderer);
            setRenderMode(RENDERMODE_WHEN_DIRTY);
        }

        int touchX() { return touchX; }
        int touchY() { return touchY; }
        int touchActive() { return touchActive ? 1 : 0; }

        void setRenderFrameValues(int[] values) {
            renderer.setFrame(values);
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
            if (count < CAPACITY) count += 1;
        }

        double averageMillis() {
            if (count == 0) return 0.0;
            long now = System.nanoTime();
            long total = 0L;
            int samples = 0;
            for (int index = 0; index < count; index += 1) {
                if (now - sampleTimes[index] <= WINDOW_NANOS) {
                    total += durations[index];
                    samples += 1;
                }
            }
            return samples == 0 ? 0.0 : (double)total / samples / 1_000_000.0;
        }
    }
}
