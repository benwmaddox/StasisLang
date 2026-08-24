package @STASIS_PACKAGE_ID@;

import android.content.res.AssetManager;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.pm.ActivityInfo;
import android.content.pm.PackageInfo;
import android.graphics.Color;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.util.Log;
import android.view.Gravity;
import android.view.MotionEvent;
import android.view.View;
import android.widget.FrameLayout;
import android.widget.TextView;
import com.stasislang.shell.StasisAssetCache;
import org.libsdl.app.SDLActivity;
import java.io.File;
import java.io.IOException;
import java.io.InputStream;

public final class MainActivity extends SDLActivity {
    private static final String STASIS_ANDROID_ORIENTATION = "@STASIS_ANDROID_ORIENTATION@";
    private static final String INVALID_ASSET_ROOT = ".stasis_asset_root_unavailable";
    private static final long HUD_UPDATE_INTERVAL_MS = 200L;
    private static final double FRAME_BUDGET_MILLIS = 1000.0 / 60.0;
    private static final boolean STASIS_NETWORK_ENABLED = @STASIS_NETWORK_ENABLED@ != 0;

    private static native void nativeSetAssetRoot(String path);
    private static native void nativeSetSeamTestId(String testId);
    private static native boolean nativeReadPerformanceMetrics(float[] output);
    private static native void nativeSetPerformanceMetricsEnabled(boolean enabled);
    private static native String nativeReadRuntimeError();
    private static native String nativeReadNetworkJoinUrl();

    private final Handler hudHandler = new Handler(Looper.getMainLooper());
    private final float[] nativePerformance = new float[14];
    private final RollingMetric[] phaseMetrics = new RollingMetric[] {
            new RollingMetric(), new RollingMetric(), new RollingMetric(), new RollingMetric(),
            new RollingMetric(), new RollingMetric(), new RollingMetric(), new RollingMetric()
    };
    private final RollingMetric frameWorst = new RollingMetric();
    private final StringBuilder hudText = new StringBuilder(420);
    private FrameLayout diagnosticLayer;
    private TextView performanceHud;
    private TextView runtimeError;
    private TextView joinUrl;
    private String joinUrlValue;
    private Runnable hudUpdater;
    private String displayedRuntimeError;

    @Override
    public void setOrientationBis(int width, int height, boolean resizable, String hint) {
        int requestedOrientation;
        switch (STASIS_ANDROID_ORIENTATION) {
            case "sensorLandscape":
                requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE;
                break;
            case "sensorPortrait":
                requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_SENSOR_PORTRAIT;
                break;
            case "fullSensor":
                requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_FULL_SENSOR;
                break;
            default:
                super.setOrientationBis(width, height, resizable, hint);
                return;
        }
        setRequestedOrientation(requestedOrientation);
    }

    @Override
    protected void onCreate(Bundle state) {
        System.loadLibrary("SDL3");
        System.loadLibrary("SDL3_image");
        System.loadLibrary("main");
        String seamTestId = getIntent().getStringExtra("stasis.seam_test_id");
        if (BuildConfig.STASIS_SEAM_TESTS && seamTestId != null) {
            nativeSetSeamTestId(seamTestId);
        }
        File invalidAssetRoot = new File(getFilesDir(), INVALID_ASSET_ROOT + "."
                + Long.toHexString(System.nanoTime()));
        while (invalidAssetRoot.exists()) {
            invalidAssetRoot = new File(getFilesDir(), INVALID_ASSET_ROOT + "."
                    + Long.toHexString(System.nanoTime()));
        }
        nativeSetAssetRoot(invalidAssetRoot.getAbsolutePath());
        String startupError = null;
        try {
            PackageInfo packageInfo = getPackageManager().getPackageInfo(getPackageName(), 0);
            long versionCode = Build.VERSION.SDK_INT >= Build.VERSION_CODES.P
                    ? packageInfo.getLongVersionCode() : packageInfo.versionCode;
            String releaseIdentity = versionCode + ":" + packageInfo.versionName + ":"
                    + packageInfo.lastUpdateTime;
            long startedNanos = System.nanoTime();
            StasisAssetCache.Result result = new StasisAssetCache(
                    new AndroidAssetSource(getAssets()), getFilesDir(), getPackageName(),
                    releaseIdentity).prepare();
            StasisAssetCache.Metrics metrics = result.getMetrics();
            long elapsedMillis = (System.nanoTime() - startedNanos) / 1_000_000L;
            Log.i("Stasis", "Asset cache mode=" + (result.isReused() ? "reuse" : "cold")
                    + " elapsed_ms=" + elapsedMillis
                    + " packaged_read_bytes=" + metrics.getPackagedReadBytes()
                    + " cache_read_bytes=" + metrics.getCacheReadBytes()
                    + " cache_write_bytes=" + metrics.getCacheWriteBytes());
            File root = result.getRoot();
            File assetBase = new File(root, "@STASIS_ASSET_BASE@");
            if (!assetBase.isDirectory()) throw new IOException("validated asset base is unavailable");
            nativeSetAssetRoot(assetBase.getAbsolutePath());
        } catch (Exception error) {
            Log.e("Stasis", "Asset cache preparation failed before runtime startup", error);
            startupError = "Asset verification failed: " + error.getMessage();
        }
        super.onCreate(state);
        installDiagnosticOverlay();
        if (startupError != null) showRuntimeError(startupError);
        startPerformanceHudUpdates();
    }

    @Override
    public boolean dispatchTouchEvent(MotionEvent event) {
        if (event.getActionMasked() == MotionEvent.ACTION_POINTER_DOWN
                && event.getPointerCount() >= 3) {
            togglePerformanceHud();
        }
        return super.dispatchTouchEvent(event);
    }

    @Override
    protected void onDestroy() {
        nativeSetPerformanceMetricsEnabled(false);
        stopPerformanceHudUpdates();
        super.onDestroy();
    }

    private void installDiagnosticOverlay() {
        diagnosticLayer = new FrameLayout(this);
        diagnosticLayer.setOnApplyWindowInsetsListener((view, insets) -> {
            int left = insets.getSystemWindowInsetLeft();
            int top = insets.getSystemWindowInsetTop();
            int right = insets.getSystemWindowInsetRight();
            int bottom = insets.getSystemWindowInsetBottom();
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P && insets.getDisplayCutout() != null) {
                left = Math.max(left, insets.getDisplayCutout().getSafeInsetLeft());
                top = Math.max(top, insets.getDisplayCutout().getSafeInsetTop());
                right = Math.max(right, insets.getDisplayCutout().getSafeInsetRight());
                bottom = Math.max(bottom, insets.getDisplayCutout().getSafeInsetBottom());
            }
            view.setPadding(left, top, right, bottom);
            return insets;
        });
        addContentView(diagnosticLayer, new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT,
                FrameLayout.LayoutParams.MATCH_PARENT));

        performanceHud = new TextView(this);
        performanceHud.setTextColor(Color.WHITE);
        performanceHud.setTextSize(10.0f);
        performanceHud.setSingleLine(false);
        performanceHud.setPadding(dp(6), dp(4), dp(6), dp(4));
        performanceHud.setBackgroundColor(Color.argb(150, 20, 28, 38));
        performanceHud.setContentDescription("Stasis performance timing overlay");
        performanceHud.setVisibility(View.GONE);
        FrameLayout.LayoutParams params = new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.WRAP_CONTENT,
                FrameLayout.LayoutParams.WRAP_CONTENT,
                Gravity.TOP | Gravity.START);
        params.setMargins(dp(4), dp(4), dp(4), 0);
        diagnosticLayer.addView(performanceHud, params);

        runtimeError = new TextView(this);
        runtimeError.setTextColor(Color.rgb(255, 210, 210));
        runtimeError.setTextSize(12.0f);
        runtimeError.setPadding(dp(10), dp(8), dp(10), dp(8));
        runtimeError.setBackgroundColor(Color.argb(220, 90, 0, 0));
        runtimeError.setContentDescription("Stasis runtime error");
        runtimeError.setVisibility(View.GONE);
        FrameLayout.LayoutParams errorParams = new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT,
                FrameLayout.LayoutParams.WRAP_CONTENT,
                Gravity.BOTTOM | Gravity.START);
        errorParams.setMargins(dp(8), 0, dp(8), dp(8));
        diagnosticLayer.addView(runtimeError, errorParams);
        if (STASIS_NETWORK_ENABLED) {
            joinUrl = new TextView(this);
            joinUrl.setTextColor(Color.WHITE);
            joinUrl.setTextSize(14.0f);
            joinUrl.setPadding(dp(10), dp(8), dp(10), dp(8));
            joinUrl.setBackgroundColor(Color.argb(220, 20, 28, 38));
            joinUrl.setText("Waiting for local network host…");
            joinUrl.setContentDescription("Manual network join URL");
            joinUrl.setTextIsSelectable(true);
            joinUrl.setOnClickListener(view -> {
                if (joinUrlValue == null || joinUrlValue.isEmpty()) return;
                ClipboardManager clipboard =
                        (ClipboardManager)getSystemService(CLIPBOARD_SERVICE);
                if (clipboard != null) {
                    clipboard.setPrimaryClip(ClipData.newPlainText("Stasis join URL", joinUrlValue));
                    joinUrl.setText("Join URL copied\n" + joinUrlValue);
                }
            });
            FrameLayout.LayoutParams joinParams = new FrameLayout.LayoutParams(
                    FrameLayout.LayoutParams.WRAP_CONTENT,
                    FrameLayout.LayoutParams.WRAP_CONTENT,
                    Gravity.TOP | Gravity.CENTER_HORIZONTAL);
            joinParams.setMargins(dp(8), dp(8), dp(8), 0);
            diagnosticLayer.addView(joinUrl, joinParams);
        }
        diagnosticLayer.requestApplyInsets();
    }

    private void togglePerformanceHud() {
        if (performanceHud == null) return;
        boolean show = performanceHud.getVisibility() != View.VISIBLE;
        nativeSetPerformanceMetricsEnabled(show);
        performanceHud.setVisibility(show ? View.VISIBLE : View.GONE);
        if (show) {
            for (RollingMetric metric : phaseMetrics) metric.clear();
            frameWorst.clear();
            updatePerformanceHud();
        }
    }

    private void startPerformanceHudUpdates() {
        if (hudUpdater != null) return;
        hudUpdater = new Runnable() {
            @Override public void run() {
                updatePerformanceHud();
                updateRuntimeError();
                updateJoinUrl();
                if (hudUpdater != null) {
                    hudHandler.postDelayed(this, HUD_UPDATE_INTERVAL_MS);
                }
            }
        };
        hudHandler.postDelayed(hudUpdater, HUD_UPDATE_INTERVAL_MS);
    }

    private void stopPerformanceHudUpdates() {
        if (hudUpdater == null) return;
        hudHandler.removeCallbacks(hudUpdater);
        hudUpdater = null;
    }

    private void updatePerformanceHud() {
        if (performanceHud == null || performanceHud.getVisibility() != View.VISIBLE
                || !nativeReadPerformanceMetrics(nativePerformance)) return;
        long now = System.nanoTime();
        for (int index = 0; index < phaseMetrics.length; index++) {
            if (nativePerformance[index] >= 0.0f) phaseMetrics[index].add(now, nativePerformance[index]);
        }
        if (nativePerformance[6] >= 0.0f) frameWorst.add(now, nativePerformance[6]);
        int budgetPercent = nativePerformance[6] < 0.0f ? 0 : Math.max(0,
                (int)((nativePerformance[6] * 100.0 / FRAME_BUDGET_MILLIS) + 0.5));
        hudText.setLength(0);
        hudText.append("SDL · Android\n");
        appendPhase(hudText, "tick", nativePerformance[0], phaseMetrics[0].worst());
        appendPhase(hudText, "guest render", nativePerformance[1], phaseMetrics[1].worst());
        appendPhase(hudText, "host replay", nativePerformance[2], phaseMetrics[2].worst());
        appendPhase(hudText, "render prep", nativePerformance[3], phaseMetrics[3].worst());
        appendPhase(hudText, "GPU submit", nativePerformance[4], phaseMetrics[4].worst());
        appendPhase(hudText, "GPU execution", nativePerformance[5], phaseMetrics[5].worst());
        appendPhase(hudText, "frame work", nativePerformance[6], frameWorst.worst());
        appendPhase(hudText, "present wait", nativePerformance[7], phaseMetrics[7].worst());
        if (nativePerformance[6] >= 0.0f) {
            hudText.append(nativePerformance[6] <= FRAME_BUDGET_MILLIS ? "UNDER" : "OVER")
                    .append(" 16.67 ms · ");
        }
        appendWorkload(hudText, "commands", nativePerformance[9], true);
        appendWorkload(hudText, "lines", nativePerformance[10], false);
        appendWorkload(hudText, "rects", nativePerformance[11], false);
        appendWorkload(hudText, "sprites", nativePerformance[12], false);
        appendWorkload(hudText, "text", nativePerformance[13], false);
        performanceHud.setTextColor(debugColorForBudget(budgetPercent));
        performanceHud.setText(hudText.toString());
    }

    private static void appendPhase(StringBuilder builder, String name, float current, double worst) {
        if (current < 0.0f) return;
        builder.append(name).append(' ');
        appendMillis(builder, current); builder.append(" (worst "); appendMillis(builder, worst); builder.append(')');
        builder.append('\n');
    }

    private static void appendWorkload(StringBuilder builder, String label, float value, boolean first) {
        if (value < 0.0f) return;
        builder.append(first ? "workload " : " · ").append(label).append(' ')
                .append((int)(value + 0.5f));
    }

    private void updateRuntimeError() {
        String message = nativeReadRuntimeError();
        if (message != null && !message.isEmpty()) showRuntimeError(message);
    }

    private void updateJoinUrl() {
        if (!STASIS_NETWORK_ENABLED || joinUrl == null) return;
        String candidate = nativeReadNetworkJoinUrl();
        if (candidate == null || candidate.isEmpty()) {
            if (joinUrlValue == null) joinUrl.setText("Waiting for local network host…");
            return;
        }
        if (!isUsableJoinUrl(candidate)) {
            joinUrlValue = null;
            joinUrl.setText("Local network address unavailable");
            return;
        }
        if (!candidate.equals(joinUrlValue)) {
            joinUrlValue = candidate;
            joinUrl.setText("Tap to copy join URL\n" + candidate);
        }
    }

    private static boolean isUsableJoinUrl(String value) {
        String lower = value.toLowerCase(java.util.Locale.ROOT);
        return lower.startsWith("http://")
                && !lower.contains("localhost")
                && !lower.contains("127.0.0.1")
                && !lower.contains("0.0.0.0");
    }

    private void showRuntimeError(String message) {
        if (runtimeError == null || message == null || message.equals(displayedRuntimeError)) return;
        displayedRuntimeError = message;
        runtimeError.setText("Release runtime error\n" + message);
        runtimeError.setVisibility(View.VISIBLE);
        runtimeError.announceForAccessibility(message);
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

    private int dp(int value) {
        return Math.round(value * getResources().getDisplayMetrics().density);
    }

    private static final class AndroidAssetSource implements StasisAssetCache.AssetSource {
        private final AssetManager assets;

        AndroidAssetSource(AssetManager assets) {
            this.assets = assets;
        }

        @Override
        public String[] list(String path) throws IOException {
            String[] children = assets.list(path);
            return children == null ? new String[0] : children;
        }

        @Override
        public InputStream open(String path) throws IOException {
            return assets.open(path);
        }
    }

    @Override
    protected String[] getLibraries() {
        return new String[] {"SDL3", "SDL3_image", "main"};
    }

    private static final class RollingMetric {
        private static final long WINDOW_NANOS = 5_000_000_000L;
        private static final int CAPACITY = 32;
        private final long[] times = new long[CAPACITY];
        private final double[] values = new double[CAPACITY];
        private int next;
        private int count;

        void add(long now, double value) {
            times[next] = now;
            values[next] = value;
            next = (next + 1) % CAPACITY;
            if (count < CAPACITY) count++;
        }

        void clear() {
            next = 0;
            count = 0;
        }

        double average() {
            long cutoff = System.nanoTime() - WINDOW_NANOS;
            double total = 0.0;
            int samples = 0;
            for (int i = 0; i < count; i++) {
                if (times[i] >= cutoff) {
                    total += values[i];
                    samples++;
                }
            }
            return samples == 0 ? 0.0 : total / samples;
        }

        double worst() {
            long cutoff = System.nanoTime() - WINDOW_NANOS;
            double result = -1.0;
            for (int i = 0; i < count; i++) {
                if (times[i] >= cutoff && values[i] > result) result = values[i];
            }
            return result < 0.0 ? 0.0 : result;
        }
    }
}
