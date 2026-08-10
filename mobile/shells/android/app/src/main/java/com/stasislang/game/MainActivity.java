package @STASIS_PACKAGE_ID@;

import android.content.res.AssetManager;
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
import org.libsdl.app.SDLActivity;
import org.json.JSONArray;
import org.json.JSONObject;
import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.HashSet;

public final class MainActivity extends SDLActivity {
    private static final long HUD_UPDATE_INTERVAL_MS = 200L;
    private static final double FRAME_BUDGET_MILLIS = 1000.0 / 60.0;
    private static final int MAX_MANIFEST_BYTES = 1024 * 1024;
    private static final int MAX_MANIFEST_ASSETS = 4096;
    private static final long MAX_ASSET_BYTES = 128L * 1024L * 1024L;
    private static final long MAX_TOTAL_ASSET_BYTES = 150L * 1024L * 1024L;

    private static native void nativeSetAssetRoot(String path);
    private static native boolean nativeReadPerformanceMetrics(float[] output);
    private static native String nativeReadRuntimeError();

    private final Handler hudHandler = new Handler(Looper.getMainLooper());
    private final float[] nativePerformance = new float[2];
    private final RollingMetric tickMetric = new RollingMetric();
    private final RollingMetric renderMetric = new RollingMetric();
    private final StringBuilder hudText = new StringBuilder(96);
    private FrameLayout diagnosticLayer;
    private TextView performanceHud;
    private TextView runtimeError;
    private Runnable hudUpdater;
    private String displayedRuntimeError;

    @Override
    protected void onCreate(Bundle state) {
        System.loadLibrary("SDL3");
        System.loadLibrary("SDL3_image");
        System.loadLibrary("main");
        File root = new File(getFilesDir(), "stasis_game");
        File staging = new File(getFilesDir(), ".stasis_game.staging");
        String startupError = null;
        try {
            deleteTree(staging);
            copyAssetTree(getAssets(), "stasis_game", staging);
            verifyAssetManifest(staging);
            deleteTree(root);
            if (!staging.renameTo(root)) {
                throw new IOException("Unable to publish " + root);
            }
        } catch (IOException error) {
            Log.e("Stasis", "Asset verification failed before runtime startup", error);
            try {
                deleteTree(staging);
            } catch (IOException ignored) {
                // The original failure is the actionable diagnostic.
            }
            startupError = "Asset verification failed: " + error.getMessage();
        }
        File assetBase = new File(root, "@STASIS_ASSET_BASE@");
        if (!assetBase.isDirectory()) {
            if (!root.isDirectory()) root.mkdirs();
            startupError = "Bundled Stasis asset base is unavailable";
        }
        nativeSetAssetRoot(assetBase.getAbsolutePath());
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
        performanceHud.setSingleLine(true);
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
        diagnosticLayer.requestApplyInsets();
    }

    private void togglePerformanceHud() {
        if (performanceHud == null) return;
        boolean show = performanceHud.getVisibility() != View.VISIBLE;
        performanceHud.setVisibility(show ? View.VISIBLE : View.GONE);
        if (show) {
            updatePerformanceHud();
        }
    }

    private void startPerformanceHudUpdates() {
        if (hudUpdater != null) return;
        hudUpdater = new Runnable() {
            @Override public void run() {
                updatePerformanceHud();
                updateRuntimeError();
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
        tickMetric.add(now, nativePerformance[0]);
        renderMetric.add(now, nativePerformance[1]);
        double tickAverage = tickMetric.average();
        double renderAverage = renderMetric.average();
        double totalAverage = tickAverage + renderAverage;
        int budgetPercent = Math.max(0,
                (int)((totalAverage * 100.0 / FRAME_BUDGET_MILLIS) + 0.5));
        hudText.setLength(0);
        hudText.append("tick=");
        appendMillis(hudText, tickAverage);
        hudText.append("  render=");
        appendMillis(hudText, renderAverage);
        hudText.append("  total=");
        appendMillis(hudText, totalAverage);
        hudText.append(" ms  budget@60fps=").append(budgetPercent).append('%');
        performanceHud.setTextColor(debugColorForBudget(budgetPercent));
        performanceHud.setText(hudText.toString());
    }

    private void updateRuntimeError() {
        String message = nativeReadRuntimeError();
        if (message != null && !message.isEmpty()) showRuntimeError(message);
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

    private static void verifyAssetManifest(File root) throws IOException {
        File manifestFile = new File(root, "assets/manifest.json");
        byte[] manifestBytes = readBounded(manifestFile, MAX_MANIFEST_BYTES);
        try {
            JSONObject manifest = new JSONObject(new String(manifestBytes, "UTF-8"));
            int manifestVersion = manifest.optInt("version", -1);
            if (!"stasis-assets".equals(manifest.optString("schema"))
                    || (manifestVersion != 1 && manifestVersion != 2)) {
                throw new IOException("Unsupported packaged asset manifest");
            }
            JSONArray assets = manifest.getJSONArray("assets");
            if (assets.length() > MAX_MANIFEST_ASSETS) {
                throw new IOException("Asset manifest exceeds the entry limit");
            }
            String rootPath = root.getCanonicalPath() + File.separator;
            HashSet<String> ids = new HashSet<>();
            HashSet<String> paths = new HashSet<>();
            long totalBytes = 0;
            for (int index = 0; index < assets.length(); index++) {
                JSONObject asset = assets.getJSONObject(index);
                String id = asset.getString("id");
                String path = asset.getString("path");
                String expectedHash = asset.getString("content_sha256");
                if (id.isEmpty() || !ids.add(id)) {
                    throw new IOException("Asset manifest contains an invalid or duplicate id");
                }
                if (!isSafeAssetPath(path) || !paths.add(path)) {
                    throw new IOException("Asset manifest contains an unsafe or duplicate path");
                }
                if (!expectedHash.matches("[0-9a-f]{64}")) {
                    throw new IOException("Asset manifest contains an invalid SHA-256 value");
                }
                File assetFile = new File(root, path);
                String assetPath = assetFile.getCanonicalPath();
                if (!assetPath.startsWith(rootPath) || !assetFile.isFile()
                        || assetFile.length() > MAX_ASSET_BYTES) {
                    throw new IOException("Packaged asset is missing, unsafe, or oversized: " + path);
                }
                totalBytes += assetFile.length();
                if (totalBytes > MAX_TOTAL_ASSET_BYTES) {
                    throw new IOException("Packaged assets exceed the total byte limit");
                }
                if (!expectedHash.equals(sha256(assetFile))) {
                    throw new IOException("Packaged asset hash mismatch: " + path);
                }
            }
        } catch (IOException error) {
            throw error;
        } catch (Exception error) {
            throw new IOException("Asset manifest could not be parsed", error);
        }
    }

    private static boolean isSafeAssetPath(String path) {
        return path.startsWith("assets/") && !path.endsWith("/")
                && path.indexOf('\\') < 0 && path.indexOf('\0') < 0
                && !path.contains("//") && !path.contains("/../")
                && !path.endsWith("/..") && !path.contains("/./")
                && !path.endsWith("/.");
    }

    private static byte[] readBounded(File file, int limit) throws IOException {
        if (!file.isFile() || file.length() > limit) {
            throw new IOException("Asset manifest is missing or oversized");
        }
        try (FileInputStream input = new FileInputStream(file);
                ByteArrayOutputStream output = new ByteArrayOutputStream((int)file.length())) {
            byte[] buffer = new byte[16384];
            int total = 0;
            int count;
            while ((count = input.read(buffer)) != -1) {
                total += count;
                if (total > limit) throw new IOException("Asset manifest exceeds the byte limit");
                output.write(buffer, 0, count);
            }
            return output.toByteArray();
        }
    }

    private static String sha256(File file) throws IOException {
        MessageDigest digest;
        try {
            digest = MessageDigest.getInstance("SHA-256");
        } catch (NoSuchAlgorithmException error) {
            throw new IOException("SHA-256 is unavailable", error);
        }
        try (FileInputStream input = new FileInputStream(file)) {
            byte[] buffer = new byte[16384];
            int count;
            while ((count = input.read(buffer)) != -1) digest.update(buffer, 0, count);
        }
        StringBuilder result = new StringBuilder(64);
        for (byte value : digest.digest()) {
            int unsigned = value & 0xff;
            if (unsigned < 16) result.append('0');
            result.append(Integer.toHexString(unsigned));
        }
        return result.toString();
    }

    private static void copyAssetTree(AssetManager assets, String assetPath, File output)
            throws IOException {
        String[] children = assets.list(assetPath);
        if (children != null && children.length > 0) {
            if (!output.isDirectory() && !output.mkdirs()) {
                throw new IOException("Unable to create " + output);
            }
            for (String child : children) {
                copyAssetTree(assets, assetPath + "/" + child, new File(output, child));
            }
            return;
        }
        File parent = output.getParentFile();
        if (parent != null && !parent.isDirectory() && !parent.mkdirs()) {
            throw new IOException("Unable to create " + parent);
        }
        try (InputStream input = assets.open(assetPath);
                FileOutputStream stream = new FileOutputStream(output)) {
            byte[] buffer = new byte[16384];
            int count;
            while ((count = input.read(buffer)) != -1) {
                stream.write(buffer, 0, count);
            }
        }
    }

    private static void deleteTree(File path) throws IOException {
        if (!path.exists()) {
            return;
        }
        File[] children = path.listFiles();
        if (children != null) {
            for (File child : children) {
                deleteTree(child);
            }
        }
        if (!path.delete()) {
            throw new IOException("Unable to remove " + path);
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
    }
}
