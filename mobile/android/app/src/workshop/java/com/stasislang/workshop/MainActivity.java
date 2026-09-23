package com.stasislang.workshop;

import android.app.Activity;
import android.app.AlertDialog;
import android.Manifest;
import android.content.BroadcastReceiver;
import android.content.ActivityNotFoundException;
import android.content.Context;
import android.content.DialogInterface;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.SharedPreferences;
import android.content.res.AssetManager;
import android.content.res.Configuration;
import android.content.pm.PackageManager;
import android.graphics.Color;
import android.graphics.Bitmap;
import android.opengl.GLSurfaceView;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.graphics.drawable.StateListDrawable;
import android.media.AudioManager;
import android.media.MediaPlayer;
import android.media.MediaRecorder;
import android.media.ToneGenerator;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.Uri;
import android.os.Build;
import android.os.BatteryManager;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.PowerManager;
import android.os.SystemClock;
import android.text.InputType;
import android.view.DisplayCutout;
import android.view.Gravity;
import android.view.MotionEvent;
import android.view.View;
import android.view.Window;
import android.view.WindowInsets;
import android.widget.Button;
import android.widget.CheckBox;
import android.widget.ArrayAdapter;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.Spinner;
import android.widget.TextView;
import android.widget.Toast;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.URL;
import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.nio.file.AtomicMoveNotSupportedException;
import java.nio.file.Files;
import java.nio.file.StandardCopyOption;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.Comparator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.TreeSet;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;

import org.json.JSONArray;
import org.json.JSONObject;

public final class MainActivity extends Activity {
    private static final String PROJECT_DIR = WorkshopProjectRegistry.LEGACY_PROJECT_DIR;
    private static final String PROJECT_BASELINES_DIR = "workshop_project_baselines";
    private static final String PROJECT_BASELINE_READY = ".ready";
    private static final String SAMPLE_MIGRATION_PREFS = "workshop_sample_migrations";
    private static final String PONG_SLOW_BALL_MIGRATION = "pong_slow_ball_v1";
    private static final String PONG_GFX_CMD_MIGRATION = "pong_gfx_cmd_v6";
    private static final String ONBOARDING_PREFS = "onboarding_settings";
    private static final String EXPLORATION_LESSON_PREFS = "exploration_lesson_progress";
    private static final String GITHUB_PREFS = "github_sync_settings";
    private static final String GITHUB_PREF_TOKEN = "github_token";
    private static final String GITHUB_PREF_REPOSITORY = "github_repository";
    private static final String GITHUB_PREF_BRANCH = "github_branch";
    private static final String GITHUB_PREF_OPERATION = "github_pending_operation";
    private static final String GITHUB_PREF_OPERATION_STATE = "github_operation_state";
    private static final String GITHUB_PREF_OPERATION_DETAIL = "github_operation_detail";
    private static final String GITHUB_PREF_OPERATION_AUTOMATIC = "github_operation_automatic";
    private static final String GITHUB_PREF_REVIEW_FINGERPRINT = "github_review_fingerprint";
    private static final String GITHUB_PREF_AUTO_SYNC = "github_auto_sync";
    private static final String GITHUB_PREF_REMOTE_STATE = "github_remote_state";
    private static final String GITHUB_PREF_LAST_SYNC_FINGERPRINT = "github_last_sync_fingerprint";
    private static final String GITHUB_PREF_VALIDATED_TARGET = "github_validated_target_v1";
    private static final long DEFAULT_TICK_INTERVAL_MS = 16L;
    private static final long DEBUG_UPDATE_INTERVAL_NANOS = 250_000_000L;
    private static final long GITHUB_AUTO_SYNC_DEBOUNCE_MS = 2_000L;
    private static final int MAX_GITHUB_BACKUP_BYTES = 32 * 1024 * 1024;
    private static final int TOP_CONTROL_END_MARGIN_DP = 10;
    private static final long STABLE_LAUNCH_DELAY_MS = 60_000L;
    private static final int AUDIO_RECORD_PERMISSION_REQUEST = 42;
    private static final int EXPORT_PROJECT_REQUEST = 71;
    private static final int IMPORT_PROJECT_REQUEST = 72;
    private static final int IMPORT_IMAGE_REQUEST = 73;
    private static final int IMPORT_AUDIO_REQUEST = 74;
    private static final int EXPORT_SUPPORT_BUNDLE_REQUEST = 75;
    private static final int RENDER_FRAME_HEADER_SIZE = 22;
    private static final String RENDER_PERFORMANCE_ACCEPTANCE_EXTRA =
            "stasis_render_performance";
    private TextView sourceTitle;
    private LinearLayout selectedSourcePanel;
    private LinearLayout manualEditBody;
    private LinearLayout diagnosticBody;
    private LinearLayout moreToolsBody;
    private EditText sourceEditor;
    private LinearLayout githubSettingsBody;
    private LinearLayout privacySettingsBody;
    private LinearLayout onboardingBody;
    private TextView onboardingSummary;
    private WorkshopOnboardingPolicy.Progress onboardingState;
    private EditText githubTokenEditor;
    private EditText githubRepositoryEditor;
    private EditText githubBranchEditor;
    private CheckBox githubAutoSync;
    private TextView githubSyncStatus;
    private LinearLayout projectSettingsBody;
    private EditText newProjectNameEditor;
    private Spinner projectSelector;
    private Spinner templateSelector;
    private TextView projectStatus;
    private LinearLayout imageAssetList;
    private LinearLayout audioAssetList;
    private EditText audioRecordingNameEditor;
    private final ArrayList<WorkshopProjectRegistry.ProjectInfo> availableProjects = new ArrayList<>();
    private WorkshopProjectRegistry.ProjectInfo activeProject;
    private WorkshopProjectRegistry.ProjectInfo pendingExportProject;
    private String pendingImportProjectName = "";
    private String projectRegistryError = "";
    private String reviewedGitHubChangeFingerprint = "";
    private String credentialStorageError = "";
    private volatile boolean projectIoActive;
    private volatile boolean activityDestroyed;
    private boolean restartLoopRecoveryActive;
    private ConnectivityManager connectivityManager;
    private ConnectivityManager.NetworkCallback networkCallback;
    private boolean networkCallbackRegistered;
    private BroadcastReceiver powerReceiver;
    private boolean powerReceiverRegistered;
    private MediaPlayer activeAudioPreview;
    private MediaRecorder activeAudioRecorder;
    private ToneGenerator explorationTone;
    private int lastExplorationAudioSerial;
    private File activeAudioRecordingFile;
    private boolean audioRecordingActive;
    private TextView reloadStatus;
    private TextView diagnosticStatus;
    private String diagnosticFile = "";
    private String diagnosticSymbol = "";
    private int diagnosticLine;
    private int diagnosticColumn;
    private int diagnosticEndLine;
    private int diagnosticEndColumn;
    private AndroidEditRecoveryStore.Entry selectedRecoveryEntry;
    private TextView changeSummary;
    private TextView gameStatus;
    private LinearLayout blockingErrorPanel;
    private TextView blockingErrorBody;
    private GamePreviewView gamePreview;
    private boolean previewFocusabilityCaptured;
    private boolean previewFocusableWhenUncovered;
    private boolean previewFocusableInTouchModeWhenUncovered;
    private LinearLayout symbolList;
    private File projectRootFile;
    private String projectRootPath;
    private ScrollView editorPanel;
    private Button editorToggle;
    private WorkshopPaintView activePaintView;
    private AlertDialog activePaintDialog;
    private EditText activePaintName;
    private final Handler gameLoopHandler = new Handler(Looper.getMainLooper());
    private final Runnable githubAutoSyncRequest = new Runnable() {
        @Override public void run() { scheduleGitHubAutoSync(); }
    };
    private final ExecutorService githubSyncExecutor = Executors.newSingleThreadExecutor();
    private final ExecutorService projectIoExecutor = Executors.newSingleThreadExecutor();
    private Runnable gameLoop;
    private final int[] nativeFrameValues = new int[RENDER_FRAME_HEADER_SIZE];
    private final StringBuilder debugTextBuilder = new StringBuilder(64);
    private final RollingMetric tickMetric = new RollingMetric();
    private final RollingMetric syncMetric = new RollingMetric();
    private final RollingMetric renderMetric = new RollingMetric();
    private AndroidAudioFocus audioFocus;
    private boolean compileReady;
    private boolean compileAttempted;
    private boolean jniFrameAbiAcceptanceRun;
    private boolean workshopTouchAcceptanceRun;
    private boolean workshopHotEditAcceptanceRun;
    private boolean workshopResourceScopeAcceptanceRun;
    private boolean workshopTestRunnerAcceptanceRun;
    private boolean workshopDiagnosticSeamAcceptanceRun;
    private boolean workshopSoakAcceptanceRun;
    private boolean gameRuntimeActive;
    private String lastCompileResult = "CompileNotRun";
    private long lastDebugUpdateNanos;
    private SymbolEntry selectedSymbol;
    private static volatile MainActivity externalUrlActivity;

    static {
        System.loadLibrary("stasis_mobile_smoke");
    }

    private static native String nativeStatus();
    private static native void nativeAudioSetPaused(boolean paused);
    private static native void nativeAudioSetFocus(boolean focused);
    private static native void nativeAudioShutdown();
    private static native boolean nativeAudioRequested();
    private static native String nativeCompileProject(String projectRoot);
    private static native int nativeSetStorageRoot(String storageRoot);
    private static native String nativeSourceItems(String projectRoot);
    private static native String nativeRunTick(String projectRoot, int touchX, int touchY, int touchActive, int screenWidth, int screenHeight);
    private static native void nativeArmExternalUrlAction();
    private static native void nativeClearExternalUrlAction();
    static native int nativeRunFrameInto(String projectRoot, int touchX, int touchY,
            int touchActive, int screenWidth, int screenHeight, ByteBuffer frameI32,
            ByteBuffer frameF32, ByteBuffer frameU8);
    static native String nativeFrameAbiDescriptor();
    static native int nativeFrameTrace(ByteBuffer frameI32, ByteBuffer frameF32, ByteBuffer frameU8);
    private static native String nativeDrainSpriteReleases();
    private static native String nativePollSpriteReleaseCancellations();
    static native String nativeLastFrameError();
    private static native String nativeInspectRuntimeState(String projectRoot);
    private static native String nativeSetRuntimeI32(String projectRoot, String path, int value);
    private static native String nativeGetRuntimeI32(String projectRoot, String path);
    static native String nativeResolveSpriteAsset(String projectRoot, int handle);
    static native String nativeResolveCachedText(String projectRoot, int handle);
    static native String nativeResolveFont(String projectRoot, int handle);
    static native int[] nativeDecodeSvgSprite(String path, int width, int height);

    void reportPreviewResourceError(String message) {
        runOnUiThread(() -> setStatusText("RenderResourceError: " + message));
    }
    private static native String nativeRunTests(String projectRoot);
    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        audioFocus = new AndroidAudioFocus(this, MainActivity::nativeAudioSetFocus);
        if (nativeSetStorageRoot(new File(getFilesDir(), "stasis_preferences").getAbsolutePath()) == 0) {
            throw new IllegalStateException("Unable to initialize preference storage");
        }
        AndroidCrashStore.install(this);
        JSONObject crashState = AndroidCrashStore.noteLaunch(this);
        restartLoopRecoveryActive = crashState.optBoolean("restart_loop_detected", false);

        try {
            activeProject = WorkshopProjectRegistry.initialize(this,
                    BuildConfig.STASIS_RENDER_ACCEPTANCE
                            ? WorkshopTemplateCatalog.RENDER_ACCEPTANCE_TEMPLATE_ID
                            : WorkshopTemplateCatalog.DEFAULT_TEMPLATE_ID);
            projectRootFile = activeProject.root;
        } catch (Exception error) {
            projectRegistryError = error.getMessage();
            projectRootFile = new File(getFilesDir(), PROJECT_DIR);
        }
        projectRootPath = projectRootFile.getAbsolutePath();

        Window window = getWindow();
        window.setStatusBarColor(Color.BLACK);
        window.setNavigationBarColor(Color.BLACK);
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1) {
            View decor = window.getDecorView();
            decor.setSystemUiVisibility(decor.getSystemUiVisibility()
                    & ~View.SYSTEM_UI_FLAG_LIGHT_NAVIGATION_BAR);
        }

        ProjectSnapshot project = loadBundledProject();
        try {
            project = loadAndMigrateActiveBundledProject();
            ensureActiveProjectBaseline(project);
        } catch (IOException error) {
            projectRegistryError = "baseline: " + error.getMessage();
        }
        setContentView(createWorkshopView(project));
        handleRenderPerformanceAcceptanceIntent(getIntent());
        registerNetworkMonitoring();
        registerPowerMonitoring();
        restoreWorkshopUiState(savedInstanceState);
        restorePendingDraft();
        restoreRetainedPaintSession();
        gameLoopHandler.postDelayed(new Runnable() {
            @Override public void run() { AndroidCrashStore.markLaunchStable(MainActivity.this); }
        }, STABLE_LAUNCH_DELAY_MS);
        if (crashState.optBoolean("present", false)) {
            setStatusText(restartLoopRecoveryActive
                    ? "Restart loop detected; preview is paused until the local crash record is cleared in Privacy & Data"
                    : "Previous crash detected; export a redacted support bundle or clear the local crash record in Privacy & Data");
        }
        if (savedInstanceState == null && !BuildConfig.STASIS_RENDER_ACCEPTANCE) {
            gameLoopHandler.post(new Runnable() {
                @Override public void run() {
                    WorkshopOnboardingPolicy.Progress progress = onboardingProgress();
                    if (!progress.isComplete() && !progress.deferred) {
                        showOnboardingGuide(true);
                    }
                }
            });
        }
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        handleRenderPerformanceAcceptanceIntent(intent);
    }

    private void handleRenderPerformanceAcceptanceIntent(Intent intent) {
        if (!BuildConfig.STASIS_RENDER_ACCEPTANCE || intent == null
                || !intent.getBooleanExtra(RENDER_PERFORMANCE_ACCEPTANCE_EXTRA, false)) {
            return;
        }
        intent.removeExtra(RENDER_PERFORMANCE_ACCEPTANCE_EXTRA);
        if (gamePreview != null) gamePreview.startPerformanceSamplingForAcceptance();
    }

    @Override
    protected void onResume() {
        super.onResume();
        externalUrlActivity = this;
        nativeAudioSetPaused(false);
        if (gamePreview != null) gamePreview.onHostResume();
        refreshGitHubSyncStatus();
        resumeGitHubAfterNetworkChange();
        requestGitHubAutoSync();
    }

    @Override
    protected void onPause() {
        if (externalUrlActivity == this) externalUrlActivity = null;
        nativeClearExternalUrlAction();
        if (audioFocus != null) audioFocus.pause();
        nativeAudioSetPaused(true);
        if (gamePreview != null) gamePreview.onHostPause();
        gameLoopHandler.removeCallbacks(githubAutoSyncRequest);
        scheduleGitHubAutoSync();
        persistPendingDraft();
        stopAudioPreview();
        cancelAudioRecording(false);
        super.onPause();
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (!hasFocus) nativeClearExternalUrlAction();
    }

    @Override
    protected void onSaveInstanceState(Bundle outState) {
        persistPendingDraft();
        outState.putBoolean("editor_open", editorPanel != null && editorPanel.getVisibility() == View.VISIBLE);
        outState.putBoolean("manual_open", manualEditBody != null && manualEditBody.getVisibility() == View.VISIBLE);
        outState.putBoolean("diagnostics_open", diagnosticBody != null && diagnosticBody.getVisibility() == View.VISIBLE);
        outState.putBoolean("more_tools_open", moreToolsBody != null && moreToolsBody.getVisibility() == View.VISIBLE);
        outState.putBoolean("projects_open", projectSettingsBody != null && projectSettingsBody.getVisibility() == View.VISIBLE);
        outState.putBoolean("github_settings_open", githubSettingsBody != null && githubSettingsBody.getVisibility() == View.VISIBLE);
        outState.putBoolean("privacy_open", privacySettingsBody != null && privacySettingsBody.getVisibility() == View.VISIBLE);
        outState.putBoolean("onboarding_open", onboardingBody != null && onboardingBody.getVisibility() == View.VISIBLE);
        outState.putInt("editor_scroll_y", editorPanel == null ? 0 : editorPanel.getScrollY());
        if (selectedSymbol != null) {
            outState.putString("selected_file", selectedSymbol.file);
            outState.putString("selected_kind", selectedSymbol.kind);
            outState.putString("selected_name", selectedSymbol.name);
            outState.putString("selected_owner", selectedSymbol.owner);
        }
        super.onSaveInstanceState(outState);
    }

    @Override
    public Object onRetainNonConfigurationInstance() {
        if (activePaintView == null || activePaintDialog == null
                || !activePaintDialog.isShowing()) return null;
        return new RetainedPaintSession(activePaintView.snapshot(),
                activePaintName == null ? "painted_image" : activePaintName.getText().toString(),
                activePaintView.brushColor(),
                activePaintView.brushSize(), activePaintView.isErasing());
    }

    @Override
    protected void onDestroy() {
        if (externalUrlActivity == this) externalUrlActivity = null;
        nativeClearExternalUrlAction();
        activityDestroyed = true;
        shutdownGameAudio();
        gameLoopHandler.removeCallbacks(githubAutoSyncRequest);
        unregisterNetworkMonitoring();
        unregisterPowerMonitoring();
        if (!WorkshopLongWorkCoordinator.isGitHubActive()) githubSyncExecutor.shutdownNow();
        if (!WorkshopLongWorkCoordinator.isProjectIoActive()) projectIoExecutor.shutdownNow();
        stopAudioPreview();
        cancelAudioRecording(false);
        if (explorationTone != null) {
            explorationTone.release();
            explorationTone = null;
        }
        if (gameLoop != null) {
            gameLoopHandler.removeCallbacks(gameLoop);
        }
        super.onDestroy();
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode == EXPORT_PROJECT_REQUEST) {
            completeProjectExport(resultCode, data);
        } else if (requestCode == IMPORT_PROJECT_REQUEST) {
            completeProjectImport(resultCode, data);
        } else if (requestCode == IMPORT_IMAGE_REQUEST) {
            completeImageImport(resultCode, data);
        } else if (requestCode == IMPORT_AUDIO_REQUEST) {
            completeAudioImport(resultCode, data);
        } else if (requestCode == EXPORT_SUPPORT_BUNDLE_REQUEST) {
            completeSupportBundleExport(resultCode, data);
        }
    }

    private void completeImageImport(int resultCode, Intent data) {
        if (resultCode != RESULT_OK || data == null || data.getData() == null) {
            setStatusText("Image import cancelled");
            return;
        }
        if (activeProject == null) {
            setStatusText("Image import needs a registered active project");
            return;
        }
        final Uri source = data.getData();
        final File targetProject = activeProject.root;
        if (!beginProjectIoWork("Importing a project image")) return;
        setStatusText("Image import started");
        projectIoExecutor.submit(new Runnable() {
            @Override public void run() {
                try {
                    final WorkshopImageAssets.AssetInfo asset = WorkshopImageAssets.importImage(
                            getContentResolver(), source, targetProject);
                    runOnUiThread(new Runnable() {
                        @Override public void run() {
                            refreshImageAssetList();
                            setStatusText("Image imported: " + asset.relativePath + " ("
                                    + asset.width + "x" + asset.height + ", " + asset.bytes + " bytes)");
                        }
                    });
                } catch (final Exception error) {
                    runOnUiThread(new Runnable() {
                        @Override public void run() {
                            setStatusText("Image import failed: " + error.getMessage());
                        }
                    });
                } finally {
                    finishProjectIoWork();
                }
            }
        });
    }

    private void completeAudioImport(int resultCode, Intent data) {
        if (resultCode != RESULT_OK || data == null || data.getData() == null) {
            setStatusText("Audio import cancelled");
            return;
        }
        if (activeProject == null) {
            setStatusText("Audio import needs a registered active project");
            return;
        }
        final Uri source = data.getData();
        final File targetProject = activeProject.root;
        if (!beginProjectIoWork("Importing project audio")) return;
        setStatusText("Audio import started");
        projectIoExecutor.submit(new Runnable() {
            @Override public void run() {
                try {
                    final WorkshopAudioAssets.AssetInfo asset = WorkshopAudioAssets.importAudio(
                            getContentResolver(), source, targetProject);
                    runOnUiThread(new Runnable() {
                        @Override public void run() {
                            refreshAudioAssetList();
                            setStatusText("Audio imported: " + asset.relativePath + " ("
                                    + asset.durationMs + " ms, " + asset.bytes + " bytes)");
                        }
                    });
                } catch (final Exception error) {
                    runOnUiThread(new Runnable() {
                        @Override public void run() { setStatusText("Audio import failed: " + error.getMessage()); }
                    });
                } finally {
                    finishProjectIoWork();
                }
            }
        });
    }

    private void completeProjectExport(int resultCode, Intent data) {
        final WorkshopProjectRegistry.ProjectInfo exportProject = pendingExportProject == null
                ? activeProject : pendingExportProject;
        pendingExportProject = null;
        if (resultCode != RESULT_OK || data == null || data.getData() == null) {
            setStatusText("Project export cancelled");
            return;
        }
        if (exportProject == null) {
            setStatusText("Project export failed: no active registered project");
            return;
        }
        final Uri destination = data.getData();
        if (!beginProjectIoWork("Exporting the active project")) return;
        setStatusText("Project export started");
        projectIoExecutor.submit(new Runnable() {
            @Override public void run() {
                try {
                    OutputStream output = getContentResolver().openOutputStream(destination, "w");
                    if (output == null) throw new IOException("document provider did not open the destination");
                    final WorkshopProjectArchive.ExportSummary summary;
                    try {
                        summary = WorkshopProjectArchive.exportProject(exportProject.root, output);
                    } finally {
                        output.close();
                    }
                    runOnUiThread(new Runnable() {
                        @Override public void run() {
                            setStatusText("Project export complete: " + summary.fileCount
                                    + " files, " + summary.totalBytes + " bytes");
                        }
                    });
                } catch (final Exception error) {
                    runOnUiThread(new Runnable() {
                        @Override public void run() {
                            setStatusText("Project export failed: " + error.getMessage());
                        }
                    });
                } finally {
                    finishProjectIoWork();
                }
            }
        });
    }

    private void completeProjectImport(int resultCode, Intent data) {
        final String projectName = pendingImportProjectName;
        pendingImportProjectName = "";
        if (resultCode != RESULT_OK || data == null || data.getData() == null) {
            setStatusText("Project import cancelled");
            return;
        }
        final Uri source = data.getData();
        if (!beginProjectIoWork("Importing a Stasis project")) return;
        setStatusText("Project import started");
        projectIoExecutor.submit(new Runnable() {
            @Override public void run() {
                WorkshopProjectRegistry.ProjectInfo imported = null;
                try {
                    imported = WorkshopProjectRegistry.createForImport(MainActivity.this, projectName);
                    InputStream input = getContentResolver().openInputStream(source);
                    if (input == null) throw new IOException("document provider did not open the archive");
                    final WorkshopProjectArchive.ImportSummary summary;
                    try {
                        summary = WorkshopProjectArchive.importProject(input, imported.root);
                    } finally {
                        input.close();
                    }
                    final WorkshopProjectRegistry.ProjectInfo completedProject = imported;
                    finishProjectIoWork();
                    runOnUiThread(new Runnable() {
                        @Override public void run() {
                            if (activateProject(completedProject)) {
                                setStatusText("Project import complete: " + summary.fileCount
                                        + " files, " + summary.totalBytes + " bytes - " + lastCompileResult);
                            } else {
                                setStatusText("Project imported but could not be activated; select it from Projects");
                                refreshProjectControls();
                            }
                        }
                    });
                } catch (final Exception error) {
                    if (imported != null) {
                        try {
                            WorkshopProjectRegistry.deleteFailedImport(MainActivity.this, imported);
                        } catch (Exception cleanupError) {
                            error.addSuppressed(cleanupError);
                        }
                    }
                    finishProjectIoWork();
                    runOnUiThread(new Runnable() {
                        @Override public void run() {
                            setStatusText("Project import failed and was discarded: " + error.getMessage());
                            refreshProjectControls();
                        }
                    });
                }
            }
        });
    }

    private View createWorkshopView(ProjectSnapshot project) {
        WorkshopAdaptiveLayout.Profile layout = adaptiveLayoutProfile();
        FrameLayout root = new FrameLayout(this);
        root.setBackgroundColor(Color.rgb(15, 20, 28));
        installSystemInsetGuard(root);

        gamePreview = new GamePreviewView(this);
        gamePreview.setContentDescription("Interactive Stasis game preview. Touch the game to control it.");
        root.addView(gamePreview, new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT,
                FrameLayout.LayoutParams.MATCH_PARENT));


        installGameStatusOverlay(root, true);
        installBlockingErrorPanel(root);
        LinearLayout content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        content.setPadding(dp(14), dp(12), dp(14), dp(12));
        content.setBackground(createPanelBackground(WorkshopAccessibilityPolicy.PANEL_BACKGROUND,
                Color.rgb(190, 199, 212)));

        TextView title = new TextView(this);
        title.setText("Stasis Workshop");
        title.setTextColor(WorkshopAccessibilityPolicy.PRIMARY_TEXT);
        title.setTextSize(20.0f);
        title.setTypeface(Typeface.DEFAULT_BOLD);
        if (Build.VERSION.SDK_INT >= 28) title.setAccessibilityHeading(true);
        title.setPadding(0, 0, 0, dp(8));
        content.addView(title, fullWidth());

        content.addView(createProjectControls(), fullWidth());

        Button manualToggle = new Button(this);
        manualToggle.setText("Manual Symbols and Source");
        manualToggle.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                toggleManualEditSection();
            }
        });
        content.addView(manualToggle, fullWidth());

        manualEditBody = new LinearLayout(this);
        manualEditBody.setOrientation(LinearLayout.VERTICAL);
        manualEditBody.setVisibility(View.GONE);
        content.addView(manualEditBody, fullWidth());

        selectedSourcePanel = new LinearLayout(this);
        selectedSourcePanel.setOrientation(LinearLayout.VERTICAL);
        selectedSourcePanel.setPadding(0, 0, 0, dp(6));

        sourceTitle = new TextView(this);
        sourceTitle.setTextColor(WorkshopAccessibilityPolicy.PRIMARY_TEXT);
        sourceTitle.setTextSize(15.0f);
        sourceTitle.setTypeface(Typeface.DEFAULT_BOLD);
        sourceTitle.setPadding(0, dp(8), 0, dp(6));
        selectedSourcePanel.addView(sourceTitle, fullWidth());

        sourceEditor = new EditText(this);
        sourceEditor.setTextColor(Color.rgb(28, 37, 49));
        sourceEditor.setTextSize(12.0f);
        sourceEditor.setTypeface(Typeface.MONOSPACE);
        sourceEditor.setMinLines(8);
        sourceEditor.setGravity(Gravity.TOP | Gravity.START);
        sourceEditor.setPadding(dp(12), dp(10), dp(12), dp(10));
        sourceEditor.setSingleLine(false);
        sourceEditor.setBackground(createPanelBackground(Color.WHITE, Color.rgb(207, 214, 224)));
        sourceEditor.setHint("Selected Stasis source code");
        sourceEditor.setContentDescription("Stasis source editor for the selected symbol");
        selectedSourcePanel.addView(sourceEditor, fullWidth());
        selectedSourcePanel.addView(createEditControls(), fullWidth());

        symbolList = new LinearLayout(this);
        symbolList.setOrientation(LinearLayout.VERTICAL);
        manualEditBody.addView(symbolList, fullWidth());
        rebuildSymbolList(project);
        reloadStatus = new TextView(this);
        reloadStatus.setTextColor(WorkshopAccessibilityPolicy.SECONDARY_TEXT);
        reloadStatus.setTextSize(13.0f);
        reloadStatus.setPadding(0, dp(8), 0, dp(6));
        reloadStatus.setAccessibilityLiveRegion(View.ACCESSIBILITY_LIVE_REGION_POLITE);
        content.addView(reloadStatus, fullWidth());

        Button diagnosticToggle = new Button(this);
        diagnosticToggle.setText("Diagnostics & Recovery");
        content.addView(diagnosticToggle, fullWidth());
        diagnosticBody = new LinearLayout(this);
        diagnosticBody.setOrientation(LinearLayout.VERTICAL);
        diagnosticBody.setVisibility(View.GONE);
        diagnosticToggle.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) {
                diagnosticBody.setVisibility(diagnosticBody.getVisibility() == View.VISIBLE
                        ? View.GONE : View.VISIBLE);
            }
        });

        diagnosticStatus = new TextView(this);
        diagnosticStatus.setTextSize(12.0f);
        diagnosticStatus.setTextColor(WorkshopAccessibilityPolicy.DIAGNOSTIC_TEXT);
        diagnosticStatus.setTypeface(Typeface.MONOSPACE);
        diagnosticStatus.setAccessibilityLiveRegion(View.ACCESSIBILITY_LIVE_REGION_ASSERTIVE);
        diagnosticBody.addView(diagnosticStatus, fullWidth());
        LinearLayout diagnosticActions = new LinearLayout(this);
        configureActionRow(diagnosticActions, layout);
        Button goToDiagnostic = new Button(this);
        goToDiagnostic.setText("Go to Diagnostic");
        goToDiagnostic.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { goToDiagnosticSource(); }
        });
        diagnosticActions.addView(goToDiagnostic, actionWidth(layout));
        Button recoveryHistory = new Button(this);
        recoveryHistory.setText("Recovery History");
        recoveryHistory.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { showRecoveryHistory(); }
        });
        diagnosticActions.addView(recoveryHistory, actionWidth(layout));
        Button undoFailedApply = new Button(this);
        undoFailedApply.setText("Undo Failed Apply");
        undoFailedApply.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { undoSelectedFailedApply(); }
        });
        diagnosticActions.addView(undoFailedApply, actionWidth(layout));
        diagnosticBody.addView(diagnosticActions, fullWidth());
        refreshRecoveryStatus();

        changeSummary = new TextView(this);
        changeSummary.setTextColor(WorkshopAccessibilityPolicy.SECONDARY_TEXT);
        changeSummary.setTextSize(12.0f);
        changeSummary.setTypeface(Typeface.MONOSPACE);
        changeSummary.setPadding(0, dp(6), 0, dp(6));
        diagnosticBody.addView(changeSummary, fullWidth());
        content.addView(diagnosticBody, fullWidth());
        refreshChangeSummary(project);

        View keyboardSpacer = new View(this);
        content.addView(keyboardSpacer, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                dp(360)));

        editorPanel = new ScrollView(this);
        editorPanel.setFillViewport(false);
        editorPanel.setFocusable(true);
        editorPanel.setFocusableInTouchMode(true);
        editorPanel.setVisibility(View.GONE);
        editorPanel.addView(content);
        if (Build.VERSION.SDK_INT >= 28) editorPanel.setAccessibilityPaneTitle("Stasis Workshop");
        FrameLayout.LayoutParams editorParams = new FrameLayout.LayoutParams(
                layout.fullWidthEditor ? FrameLayout.LayoutParams.MATCH_PARENT : dp(layout.editorWidthDp),
                FrameLayout.LayoutParams.MATCH_PARENT,
                Gravity.TOP | Gravity.END);
        editorParams.setMargins(dp(12), dp(64), dp(12), dp(18));
        root.addView(editorPanel, editorParams);

        sourceEditor.setOnFocusChangeListener(new View.OnFocusChangeListener() {
            @Override
            public void onFocusChange(View view, boolean hasFocus) {
                if (hasFocus) {
                    scrollEditorIntoView(editorPanel);
                }
            }
        });

        if (project.firstSymbol != null) {
            showSymbol(project.firstSymbol);
        }

        editorToggle = new Button(this);
        editorToggle.setText("\u2630");
        editorToggle.setTextSize(20.0f);
        editorToggle.setTextColor(WorkshopAccessibilityPolicy.ON_DARK_CONTROL);
        editorToggle.setContentDescription("Open Workshop menu");
        editorToggle.setMinWidth(dp(52));
        editorToggle.setMinHeight(dp(48));
        editorToggle.setBackground(createFocusableControlBackground());
        editorToggle.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                toggleEditorPanel();
            }
        });
        FrameLayout.LayoutParams toggleParams = new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.WRAP_CONTENT, FrameLayout.LayoutParams.WRAP_CONTENT,
                Gravity.TOP | Gravity.END);
        toggleParams.setMargins(0, dp(8), dp(TOP_CONTROL_END_MARGIN_DP), 0);
        root.addView(editorToggle, toggleParams);
        if (!credentialStorageError.isEmpty()) {
            setStatusText("Credential storage error: " + credentialStorageError);
        } else if (!projectRegistryError.isEmpty()) {
            setStatusText("Project registry error: " + projectRegistryError);
        }

        startGameLoop();
        return root;
    }

    private void installGameStatusOverlay(FrameLayout root, boolean visible) {
        gameStatus = new TextView(this);
        gameStatus.setText("tick avg=-- p50=-- p95=-- ms\n"
                + "render avg=-- p50=-- p95=-- ms\n"
                + "sync avg=-- p95=-- ms\n"
                + "budget tick=--% render=--% sync=--% total=--%");
        gameStatus.setTextColor(Color.WHITE);
        gameStatus.setTextSize(12.0f);
        gameStatus.setSingleLine(false);
        gameStatus.setPadding(dp(10), dp(6), dp(10), dp(6));
        gameStatus.setBackgroundColor(Color.argb(150, 20, 28, 38));
        gameStatus.setVisibility(visible ? View.VISIBLE : View.GONE);
        FrameLayout.LayoutParams statusParams = new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.WRAP_CONTENT,
                FrameLayout.LayoutParams.WRAP_CONTENT,
                Gravity.TOP | Gravity.START);
        statusParams.setMargins(dp(8), dp(8), dp(68), 0);
        root.addView(gameStatus, statusParams);
    }

    private void installBlockingErrorPanel(FrameLayout root) {
        blockingErrorPanel = new LinearLayout(this);
        blockingErrorPanel.setOrientation(LinearLayout.VERTICAL);
        blockingErrorPanel.setPadding(dp(18), dp(16), dp(18), dp(16));
        blockingErrorPanel.setBackground(createPanelBackground(
                Color.rgb(66, 24, 30), Color.rgb(235, 105, 115)));
        blockingErrorPanel.setElevation(dp(12));
        blockingErrorPanel.setVisibility(View.GONE);

        TextView title = new TextView(this);
        title.setText("Game is not running");
        title.setTextColor(Color.WHITE);
        title.setTextSize(20.0f);
        title.setTypeface(Typeface.DEFAULT_BOLD);
        if (Build.VERSION.SDK_INT >= 28) title.setAccessibilityHeading(true);
        blockingErrorPanel.addView(title, fullWidth());

        blockingErrorBody = new TextView(this);
        blockingErrorBody.setTextColor(Color.WHITE);
        blockingErrorBody.setTextSize(14.0f);
        blockingErrorBody.setTypeface(Typeface.MONOSPACE);
        blockingErrorBody.setPadding(0, dp(10), 0, dp(12));
        blockingErrorBody.setAccessibilityLiveRegion(View.ACCESSIBILITY_LIVE_REGION_ASSERTIVE);
        blockingErrorPanel.addView(blockingErrorBody, fullWidth());

        LinearLayout actions = new LinearLayout(this);
        configureActionRow(actions, adaptiveLayoutProfile());
        Button openDiagnostics = new Button(this);
        openDiagnostics.setText("Open Diagnostics");
        openDiagnostics.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) {
                if (editorPanel != null && editorPanel.getVisibility() != View.VISIBLE) {
                    toggleEditorPanel();
                }
                if (diagnosticBody != null) diagnosticBody.setVisibility(View.VISIBLE);
            }
        });
        actions.addView(openDiagnostics, actionWidth(adaptiveLayoutProfile()));
        Button retryCompile = new Button(this);
        retryCompile.setText("Retry Compile");
        retryCompile.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { runNativeCompile(); }
        });
        actions.addView(retryCompile, actionWidth(adaptiveLayoutProfile()));
        blockingErrorPanel.addView(actions, fullWidth());

        FrameLayout.LayoutParams params = new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT,
                FrameLayout.LayoutParams.WRAP_CONTENT,
                Gravity.CENTER);
        params.setMargins(dp(24), dp(96), dp(24), dp(96));
        root.addView(blockingErrorPanel, params);
    }

    @Override
    public void onRequestPermissionsResult(int requestCode, String[] permissions, int[] grantResults) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults);
        if (requestCode == AUDIO_RECORD_PERMISSION_REQUEST) {
            if (grantResults.length > 0 && grantResults[0] == PackageManager.PERMISSION_GRANTED) {
                startAudioRecording();
            } else {
                setStatusText("Audio recording needs microphone permission");
            }
        }
    }

    private void toggleBenchmarkHudFromPreview() {
        if (gameStatus == null) {
            return;
        }
        gameStatus.setVisibility(gameStatus.getVisibility() == View.VISIBLE ? View.GONE : View.VISIBLE);
        if (gameStatus.getVisibility() == View.VISIBLE) {
            updateGameDebugText();
        }
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
    private void toggleEditorPanel() {
        if (editorPanel == null) {
            return;
        }
        boolean opening = editorPanel.getVisibility() != View.VISIBLE;
        editorPanel.setVisibility(opening ? View.VISIBLE : View.GONE);
        if (opening) recordExplorationLesson(WorkshopExplorationLessonPolicy.OPENED_EDITOR);
        boolean coverPreview = opening && adaptiveLayoutProfile().fullWidthEditor;
        setPreviewCovered(coverPreview);
        if (opening) {
            editorPanel.bringToFront();
            editorPanel.post(new Runnable() {
                @Override public void run() {
                    editorPanel.requestFocus();
                    if (Build.VERSION.SDK_INT < 28) {
                        editorPanel.announceForAccessibility("Workshop menu opened");
                    }
                }
            });
        }
        if (editorToggle != null) {
            editorToggle.setText(opening ? "\u00D7" : "\u2630");
            editorToggle.setContentDescription(opening ? "Close Workshop menu" : "Open Workshop menu");
            editorToggle.bringToFront();
            if (!opening) {
                editorToggle.requestFocus();
                editorToggle.announceForAccessibility("Workshop menu closed");
            }
        }
    }

    private void setPreviewCovered(boolean covered) {
        int importance = covered ? View.IMPORTANT_FOR_ACCESSIBILITY_NO_HIDE_DESCENDANTS
                : View.IMPORTANT_FOR_ACCESSIBILITY_AUTO;
        if (gamePreview != null) {
            gamePreview.setImportantForAccessibility(importance);
            if (covered && !previewFocusabilityCaptured) {
                previewFocusableWhenUncovered = gamePreview.isFocusable();
                previewFocusableInTouchModeWhenUncovered = gamePreview.isFocusableInTouchMode();
                previewFocusabilityCaptured = true;
            }
            if (covered) {
                gamePreview.setFocusable(false);
                gamePreview.setFocusableInTouchMode(false);
            } else if (previewFocusabilityCaptured) {
                gamePreview.setFocusable(previewFocusableWhenUncovered);
                gamePreview.setFocusableInTouchMode(previewFocusableInTouchModeWhenUncovered);
                previewFocusabilityCaptured = false;
            }
        }
        if (gameStatus != null) gameStatus.setImportantForAccessibility(importance);
    }

    private void startGameLoop() {
        if (restartLoopRecoveryActive) return;
        if (gameLoop != null) {
            return;
        }
        gameLoop = new Runnable() {
            @Override
            public void run() {
                if (!compileReady && !compileAttempted) {
                    String compileResult = nativeCompileProject(projectRootPath());
                    lastCompileResult = compileResult;
                    compileReady = isRunnableCompile(compileResult);
                    compileAttempted = true;
                    setStatusText(compileResult);
                }
                if (BuildConfig.STASIS_RENDER_ACCEPTANCE && compileReady && !jniFrameAbiAcceptanceRun) {
                    String abiResult = WorkshopJniFrameAbiAcceptance.run(projectRootPath());
                    jniFrameAbiAcceptanceRun = true;
                    boolean abiPassed = false;
                    try {
                        abiPassed = "passed".equals(new JSONObject(abiResult).optString("status"));
                    } catch (Exception ignored) {
                        // The acceptance runner reports its own structured failure marker.
                    }
                    if (!abiPassed) {
                        compileReady = false;
                        setStatusText("IT-026 JNI frame ABI acceptance failed: " + abiResult);
                    }
                }
                if (BuildConfig.STASIS_RENDER_ACCEPTANCE && compileReady
                        && !workshopTouchAcceptanceRun) {
                    String touchResult = WorkshopTouchAcceptance.run(
                            MainActivity.this, projectRootPath());
                    workshopTouchAcceptanceRun = true;
                    boolean touchPassed = false;
                    try {
                        touchPassed = "passed".equals(new JSONObject(touchResult).optString("status"));
                    } catch (Exception ignored) {
                        // The acceptance runner reports its own structured failure marker.
                    }
                    if (!touchPassed) {
                        compileReady = false;
                        setStatusText("IT-027 Workshop touch acceptance failed: " + touchResult);
                    }
                }
                if (BuildConfig.STASIS_RENDER_ACCEPTANCE && compileReady
                        && workshopTouchAcceptanceRun && !workshopHotEditAcceptanceRun) {
                    String hotEditResult = WorkshopHotEditAcceptance.run(
                            MainActivity.this, projectRootPath());
                    workshopHotEditAcceptanceRun = true;
                    boolean hotEditPassed = false;
                    try {
                        hotEditPassed = "passed".equals(new JSONObject(hotEditResult).optString("status"));
                    } catch (Exception ignored) {
                        // The acceptance runner reports its own structured failure marker.
                    }
                    if (!hotEditPassed) {
                        compileReady = false;
                        gameRuntimeActive = false;
                        setStatusText("IT-028 Workshop hot-edit acceptance failed: " + hotEditResult);
                    }
                }
                if (BuildConfig.STASIS_RENDER_ACCEPTANCE && compileReady
                        && workshopHotEditAcceptanceRun && !workshopResourceScopeAcceptanceRun) {
                    String resourceScopeResult = WorkshopResourceScopeAcceptance.run(
                            MainActivity.this);
                    workshopResourceScopeAcceptanceRun = true;
                    boolean resourceScopePassed = false;
                    try {
                        resourceScopePassed = "passed".equals(
                                new JSONObject(resourceScopeResult).optString("status"));
                    } catch (Exception ignored) {
                        // The acceptance runner reports its own structured failure marker.
                    }
                    if (!resourceScopePassed) {
                        compileReady = false;
                        gameRuntimeActive = false;
                        setStatusText("IT-029 resource-scope acceptance failed: "
                                + resourceScopeResult);
                    }
                }
                if (BuildConfig.STASIS_RENDER_ACCEPTANCE && compileReady
                        && workshopResourceScopeAcceptanceRun
                        && !workshopTestRunnerAcceptanceRun) {
                    String testRunnerResult = WorkshopTestRunnerAcceptance.run(
                            MainActivity.this, projectRootPath());
                    workshopTestRunnerAcceptanceRun = true;
                    boolean testRunnerPassed = false;
                    try {
                        testRunnerPassed = "passed".equals(
                                new JSONObject(testRunnerResult).optString("status"));
                    } catch (Exception ignored) {
                        // The acceptance runner reports its own structured failure marker.
                    }
                    if (!testRunnerPassed) {
                        compileReady = false;
                        gameRuntimeActive = false;
                        setStatusText("IT-030 Workshop test-runner acceptance failed: "
                                + testRunnerResult);
                    }
                }
                if (BuildConfig.STASIS_RENDER_ACCEPTANCE && compileReady
                        && workshopTestRunnerAcceptanceRun
                        && !workshopDiagnosticSeamAcceptanceRun) {
                    String diagnosticResult = WorkshopDiagnosticSeamAcceptance.run(
                            MainActivity.this, projectRootPath());
                    workshopDiagnosticSeamAcceptanceRun = true;
                    boolean diagnosticPassed = false;
                    try {
                        diagnosticPassed = "passed".equals(
                                new JSONObject(diagnosticResult).optString("status"));
                    } catch (Exception ignored) {
                        // The acceptance runner reports its own structured failure marker.
                    }
                    if (!diagnosticPassed) {
                        compileReady = false;
                        gameRuntimeActive = false;
                        setStatusText("IT-031 diagnostic seam acceptance failed: "
                                + diagnosticResult);
                    }
                }
                if (BuildConfig.STASIS_RENDER_ACCEPTANCE && compileReady
                        && workshopDiagnosticSeamAcceptanceRun
                        && !workshopSoakAcceptanceRun) {
                    String soakResult = WorkshopSoakAcceptance.run(
                            MainActivity.this, projectRootPath());
                    workshopSoakAcceptanceRun = true;
                    boolean soakPassed = false;
                    try {
                        soakPassed = "passed".equals(
                                new JSONObject(soakResult).optString("status"));
                    } catch (Exception ignored) {
                        // The acceptance runner reports its own structured failure marker.
                    }
                    if (!soakPassed) {
                        compileReady = false;
                        gameRuntimeActive = false;
                        setStatusText("IT-032 Workshop soak acceptance failed: " + soakResult);
                    }
                }
                if (compileReady || gameRuntimeActive) {
                    runNativeTick();
                }
                gameLoopHandler.postDelayed(this, DEFAULT_TICK_INTERVAL_MS);
            }
        };
        gameLoopHandler.post(gameLoop);
    }

    private static boolean isRunnableCompile(String compileResult) {
        return compileResult.startsWith("CompileReady") && compileResult.contains("status=0");
    }

    private void setStatusText(String status) {
        if (reloadStatus != null) {
            reloadStatus.setText(compactStatusText(status));
        }
        if (blockingErrorPanel != null) {
            boolean visible = WorkshopBlockingErrorPolicy.shouldShow(gameRuntimeActive, status);
            blockingErrorPanel.setVisibility(visible ? View.VISIBLE : View.GONE);
            if (visible && blockingErrorBody != null) {
                blockingErrorBody.setText(WorkshopBlockingErrorPolicy.summary(
                        status, projectRootPath));
                blockingErrorPanel.bringToFront();
                if (editorToggle != null) editorToggle.bringToFront();
            }
        }
    }

    private static String compactStatusText(String status) {
        if (status == null) return "";
        if (status.startsWith("CompileReady") && status.contains("status=0")) {
            String reload = reloadKind(status);
            if ("FastReload".equals(reload)) return "Game updated - hot swapped";
            if ("ResetRequired".equals(reload)) return "Game updated - restarted";
            return "Game ready";
        }
        return status;
    }

    private static String reloadKind(String compileResult) {
        if (compileResult == null) {
            return "unknown";
        }
        String marker = "reload=";
        int start = compileResult.indexOf(marker);
        if (start < 0) {
            return "unknown";
        }
        start += marker.length();
        int end = start;
        while (end < compileResult.length()) {
            char value = compileResult.charAt(end);
            if (!Character.isLetterOrDigit(value)) {
                break;
            }
            end += 1;
        }
        return end > start ? compileResult.substring(start, end) : "unknown";
    }

    private void updateGameDebugText() {
        if (gameStatus == null) {
            return;
        }
        long now = System.nanoTime();
        if (now - lastDebugUpdateNanos < DEBUG_UPDATE_INTERVAL_NANOS) {
            return;
        }
        lastDebugUpdateNanos = now;
        double tickMillis = tickMetric.averageMillis();
        double syncMillis = syncMetric.averageMillis();
        double renderMillis = renderMetric.averageMillis();
        double tickP50Millis = tickMetric.percentileMillis(50);
        double tickP95Millis = tickMetric.percentileMillis(95);
        double syncP95Millis = syncMetric.percentileMillis(95);
        double renderP50Millis = renderMetric.percentileMillis(50);
        double renderP95Millis = renderMetric.percentileMillis(95);
        int tickBudgetPercent = WorkshopFrameBudget.percent(tickMillis);
        int syncBudgetPercent = WorkshopFrameBudget.percent(syncMillis);
        int renderBudgetPercent = WorkshopFrameBudget.percent(renderMillis);
        int totalBudgetPercent = WorkshopFrameBudget.percent(tickMillis + syncMillis + renderMillis);
        debugTextBuilder.setLength(0);
        debugTextBuilder.append("tick avg=");
        appendMillis(debugTextBuilder, tickMillis);
        debugTextBuilder.append(" p50=");
        appendMillis(debugTextBuilder, tickP50Millis);
        debugTextBuilder.append(" p95=");
        appendMillis(debugTextBuilder, tickP95Millis);
        debugTextBuilder.append(" ms\nrender avg=");
        appendMillis(debugTextBuilder, renderMillis);
        debugTextBuilder.append(" p50=");
        appendMillis(debugTextBuilder, renderP50Millis);
        debugTextBuilder.append(" p95=");
        appendMillis(debugTextBuilder, renderP95Millis);
        debugTextBuilder.append(" ms\nsync avg=");
        appendMillis(debugTextBuilder, syncMillis);
        debugTextBuilder.append(" p95=");
        appendMillis(debugTextBuilder, syncP95Millis);
        debugTextBuilder.append(" ms\nbudget tick=");
        appendPercent(debugTextBuilder, tickBudgetPercent);
        debugTextBuilder.append(" render=");
        appendPercent(debugTextBuilder, renderBudgetPercent);
        debugTextBuilder.append(" sync=");
        appendPercent(debugTextBuilder, syncBudgetPercent);
        debugTextBuilder.append(" total=");
        appendPercent(debugTextBuilder, totalBudgetPercent);
        appendExplorationProgress(debugTextBuilder);
        gameStatus.setTextColor(debugColorForBudget(totalBudgetPercent));
        gameStatus.setText(debugTextBuilder.toString());
    }

    private void appendExplorationProgress(StringBuilder text) {
        if (!compileReady || activeProject == null || !"exploration".equals(activeProject.templateId)) return;
        String tapsResult = nativeGetRuntimeI32(projectRootPath(), "GameState.accepted_tap_count");
        String collectedResult = nativeGetRuntimeI32(projectRootPath(), "GameState.collected_count");
        String totalResult = nativeGetRuntimeI32(projectRootPath(), "GameState.total_collectibles");
        String stageResult = nativeGetRuntimeI32(projectRootPath(), "GameState.tutorial_stage");
        String audioSerialResult = nativeGetRuntimeI32(projectRootPath(), "ExplorationAudio.event_serial");
        String audioKindResult = nativeGetRuntimeI32(projectRootPath(), "ExplorationAudio.cue_kind");
        if (collectedResult == null || collectedResult.startsWith("StateError")
                || totalResult == null || totalResult.startsWith("StateError")
                || stageResult == null || stageResult.startsWith("StateError")) return;
        int collected = extractIntField(collectedResult, "value", 0);
        int total = extractIntField(totalResult, "value", 0);
        int stage = extractIntField(stageResult, "value", 0);
        boolean tapCountAvailable = tapsResult != null && !tapsResult.startsWith("StateError");
        int taps = WorkshopExplorationLessonPolicy.effectiveTapCount(tapCountAvailable,
                tapCountAvailable ? extractIntField(tapsResult, "value", 0) : 0, stage);
        int progress = explorationLessonProgress();
        int observed = WorkshopExplorationLessonPolicy.observeGame(progress, taps, collected);
        if (observed != progress) saveExplorationLessonProgress(observed);
        text.append('\n').append("keepsakes=").append(collected).append('/').append(total).append("  lesson=");
        text.append(WorkshopExplorationLessonPolicy.prompt(observed));
        if (stage >= 3) text.append("  garden complete");
        if (audioSerialResult != null && !audioSerialResult.startsWith("StateError")
                && audioKindResult != null && !audioKindResult.startsWith("StateError")) {
            playExplorationCue(extractIntField(audioSerialResult, "value", 0),
                    extractIntField(audioKindResult, "value", 0));
        }
    }

    private String explorationLessonKey() {
        return activeProject == null ? "legacy" : activeProject.id;
    }

    private int explorationLessonProgress() {
        return getSharedPreferences(EXPLORATION_LESSON_PREFS, MODE_PRIVATE)
                .getInt(explorationLessonKey(), 0);
    }

    private void saveExplorationLessonProgress(int progress) {
        getSharedPreferences(EXPLORATION_LESSON_PREFS, MODE_PRIVATE).edit()
                .putInt(explorationLessonKey(), progress).apply();
    }

    private void recordExplorationLesson(int event) {
        if (activeProject == null || !"exploration".equals(activeProject.templateId)) return;
        int progress = explorationLessonProgress();
        saveExplorationLessonProgress(WorkshopExplorationLessonPolicy.record(progress, event));
    }

    private void playExplorationCue(int serial, int kind) {
        if (serial <= 0 || serial == lastExplorationAudioSerial) return;
        lastExplorationAudioSerial = serial;
        try {
            if (explorationTone == null) explorationTone = new ToneGenerator(AudioManager.STREAM_MUSIC, 45);
            explorationTone.startTone(kind == 2 ? ToneGenerator.TONE_PROP_ACK
                    : ToneGenerator.TONE_PROP_BEEP, 110);
        } catch (RuntimeException error) {
            if (explorationTone != null) explorationTone.release();
            explorationTone = null;
        }
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

    private static void appendPercent(StringBuilder builder, int percent) {
        builder.append(percent).append('%');
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
    private void recordRenderTimeNanos(long durationNanos) {
        renderMetric.add(System.nanoTime(), durationNanos);
    }
    private void rebuildSymbolList(ProjectSnapshot project) {
        if (symbolList == null) {
            return;
        }
        symbolList.removeAllViews();
        for (SymbolSection section : project.sections) {
            addSection(symbolList, section);
        }
    }

    private void addSection(LinearLayout content, SymbolSection section) {
        TextView sectionTitle = new TextView(this);
        sectionTitle.setText(section.title);
        sectionTitle.setTextColor(Color.rgb(35, 45, 60));
        sectionTitle.setTextSize(18.0f);
        sectionTitle.setTypeface(Typeface.DEFAULT_BOLD);
        if (Build.VERSION.SDK_INT >= 28) sectionTitle.setAccessibilityHeading(true);
        sectionTitle.setPadding(0, dp(10), 0, dp(4));
        content.addView(sectionTitle, fullWidth());

        for (SymbolGroup group : section.groups) {
            if (!group.title.equals(section.title)) {
                TextView groupTitle = new TextView(this);
                groupTitle.setText(group.title);
                groupTitle.setTextColor(Color.rgb(83, 96, 115));
                groupTitle.setTextSize(13.0f);
                groupTitle.setTypeface(Typeface.DEFAULT_BOLD);
                groupTitle.setPadding(0, dp(6), 0, dp(3));
                content.addView(groupTitle, fullWidth());
            }

            for (SymbolEntry symbol : group.symbols) {
                content.addView(createSymbolRow(symbol), fullWidth());
                if (selectedSymbol != null && sameSymbolIdentity(symbol, selectedSymbol) && selectedSourcePanel != null) {
                    content.addView(selectedSourcePanel, fullWidth());
                }
            }
        }
    }

    private TextView createSymbolRow(final SymbolEntry symbol) {
        TextView row = new TextView(this);
        row.setText(symbol.displayName());
        row.setContentDescription(symbol.kind + " " + symbol.displayName() + ". Tap to edit source.");
        row.setTextColor(Color.rgb(23, 43, 77));
        row.setTextSize(14.0f);
        row.setPadding(dp(12), dp(9), dp(12), dp(9));
        row.setBackground(createPanelBackground(Color.WHITE, Color.rgb(218, 224, 233)));
        row.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                showSymbol(symbol);
                if (manualEditBody != null) {
                    manualEditBody.setVisibility(View.VISIBLE);
                }
            }
        });

        LinearLayout.LayoutParams margins = new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT);
        margins.setMargins(0, 0, 0, dp(6));
        row.setLayoutParams(margins);
        return row;
    }

    private void scrollEditorIntoView(final ScrollView scrollView) {
        scrollView.postDelayed(new Runnable() {
            @Override
            public void run() {
                scrollView.smoothScrollTo(0, sourceEditor.getBottom());
            }
        }, 250L);
    }
    private void showSymbol(SymbolEntry symbol) {
        selectedSymbol = symbol;
        sourceTitle.setText(symbol.file + " - " + symbol.displayName());
        sourceEditor.setText(symbol.source.trim());
        rebuildSymbolList(loadBundledProject());
        setStatusText("No pending edit");
    }

    private void toggleManualEditSection() {
        if (manualEditBody == null) {
            return;
        }
        manualEditBody.setVisibility(manualEditBody.getVisibility() == View.VISIBLE ? View.GONE : View.VISIBLE);
    }

    private LinearLayout createProjectControls() {
        WorkshopAdaptiveLayout.Profile layout = adaptiveLayoutProfile();
        LinearLayout controls = new LinearLayout(this);
        controls.setOrientation(LinearLayout.VERTICAL);
        controls.setPadding(0, dp(8), 0, 0);

        Button moreToolsToggle = new Button(this);
        moreToolsToggle.setText("More Tools & Settings");
        controls.addView(moreToolsToggle, fullWidth());
        moreToolsBody = new LinearLayout(this);
        moreToolsBody.setOrientation(LinearLayout.VERTICAL);
        moreToolsBody.setVisibility(View.GONE);
        moreToolsToggle.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) {
                moreToolsBody.setVisibility(moreToolsBody.getVisibility() == View.VISIBLE
                        ? View.GONE : View.VISIBLE);
            }
        });

        Button projectSettingsToggle = new Button(this);
        projectSettingsToggle.setText("Projects");
        projectSettingsToggle.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { toggleProjectSettings(); }
        });
        moreToolsBody.addView(projectSettingsToggle, fullWidth());
        projectSettingsBody = new LinearLayout(this);
        projectSettingsBody.setOrientation(LinearLayout.VERTICAL);
        projectSettingsBody.setVisibility(View.GONE);
        projectStatus = new TextView(this);
        projectStatus.setTextSize(12.0f);
        projectStatus.setTextColor(Color.rgb(73, 84, 100));
        projectSettingsBody.addView(projectStatus, fullWidth());
        projectSelector = new Spinner(this);
        projectSettingsBody.addView(projectSelector, fullWidth());
        Button switchProject = new Button(this);
        switchProject.setText("Switch Project");
        switchProject.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { switchSelectedProject(); }
        });
        projectSettingsBody.addView(switchProject, fullWidth());
        newProjectNameEditor = new EditText(this);
        newProjectNameEditor.setHint("New project name");
        newProjectNameEditor.setSingleLine(true);
        projectSettingsBody.addView(newProjectNameEditor, fullWidth());
        templateSelector = new Spinner(this);
        ArrayAdapter<WorkshopTemplateCatalog.Template> templateAdapter = new ArrayAdapter<>(this,
                android.R.layout.simple_spinner_item, WorkshopTemplateCatalog.list());
        templateAdapter.setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item);
        templateSelector.setAdapter(templateAdapter);
        templateSelector.setContentDescription("Bundled template for the new project");
        projectSettingsBody.addView(templateSelector, fullWidth());
        Button newSampleProject = new Button(this);
        newSampleProject.setText("New Project From Selected Template");
        newSampleProject.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { createAndSwitchProject(); }
        });
        projectSettingsBody.addView(newSampleProject, fullWidth());
        Button exportProject = new Button(this);
        exportProject.setText("Export Project Archive");
        exportProject.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { requestProjectExport(); }
        });
        projectSettingsBody.addView(exportProject, fullWidth());
        Button importProject = new Button(this);
        importProject.setText("Import Project Archive");
        importProject.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { requestProjectImport(); }
        });
        projectSettingsBody.addView(importProject, fullWidth());
        TextView imageAssetsTitle = new TextView(this);
        imageAssetsTitle.setText("Image Assets");
        imageAssetsTitle.setTextSize(14.0f);
        imageAssetsTitle.setTextColor(Color.rgb(34, 43, 55));
        imageAssetsTitle.setPadding(0, dp(10), 0, dp(2));
        projectSettingsBody.addView(imageAssetsTitle, fullWidth());
        Button importImage = new Button(this);
        importImage.setText("Import PNG, JPEG, or WebP");
        importImage.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { requestImageImport(); }
        });
        projectSettingsBody.addView(importImage, fullWidth());
        Button newPaintedImage = new Button(this);
        newPaintedImage.setText("New Painted Image");
        newPaintedImage.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { requestNewPaintedImage(); }
        });
        projectSettingsBody.addView(newPaintedImage, fullWidth());
        Button restoreImage = new Button(this);
        restoreImage.setText("Restore Last Deleted Image");
        restoreImage.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { restoreLastDeletedImage(); }
        });
        projectSettingsBody.addView(restoreImage, fullWidth());
        imageAssetList = new LinearLayout(this);
        imageAssetList.setOrientation(LinearLayout.VERTICAL);
        projectSettingsBody.addView(imageAssetList, fullWidth());
        TextView audioAssetsTitle = new TextView(this);
        audioAssetsTitle.setText("Audio Assets");
        audioAssetsTitle.setTextSize(14.0f);
        audioAssetsTitle.setTextColor(Color.rgb(34, 43, 55));
        audioAssetsTitle.setPadding(0, dp(10), 0, dp(2));
        projectSettingsBody.addView(audioAssetsTitle, fullWidth());
        Button importAudio = new Button(this);
        importAudio.setText("Import MP3, Ogg, WAV, or M4A");
        importAudio.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { requestAudioImport(); }
        });
        projectSettingsBody.addView(importAudio, fullWidth());
        audioRecordingNameEditor = new EditText(this);
        audioRecordingNameEditor.setHint("Recording name (saved as M4A)");
        audioRecordingNameEditor.setSingleLine(true);
        audioRecordingNameEditor.setText("recorded_audio");
        projectSettingsBody.addView(audioRecordingNameEditor, fullWidth());
        LinearLayout recordingActions = new LinearLayout(this);
        configureActionRow(recordingActions, layout);
        Button startRecording = new Button(this);
        startRecording.setText("Record Audio");
        startRecording.setContentDescription("Start a bounded microphone recording for the active project");
        startRecording.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { requestAudioRecording(); }
        });
        Button saveRecording = new Button(this);
        saveRecording.setText("Stop & Save");
        saveRecording.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { finishAudioRecording(true); }
        });
        Button cancelRecording = new Button(this);
        cancelRecording.setText("Cancel Recording");
        cancelRecording.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { cancelAudioRecording(true); }
        });
        recordingActions.addView(startRecording, actionWidth(layout));
        recordingActions.addView(saveRecording, actionWidth(layout));
        recordingActions.addView(cancelRecording, actionWidth(layout));
        projectSettingsBody.addView(recordingActions, fullWidth());
        Button stopAudio = new Button(this);
        stopAudio.setText("Stop Audio Preview");
        stopAudio.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { stopAudioPreview(); setStatusText("Audio preview stopped"); }
        });
        projectSettingsBody.addView(stopAudio, fullWidth());
        Button restoreAudio = new Button(this);
        restoreAudio.setText("Restore Last Deleted Audio");
        restoreAudio.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { restoreLastDeletedAudio(); }
        });
        projectSettingsBody.addView(restoreAudio, fullWidth());
        audioAssetList = new LinearLayout(this);
        audioAssetList.setOrientation(LinearLayout.VERTICAL);
        projectSettingsBody.addView(audioAssetList, fullWidth());
        moreToolsBody.addView(projectSettingsBody, fullWidth());
        refreshProjectControls();
        refreshImageAssetList();
        refreshAudioAssetList();

        githubSyncStatus = new TextView(this);
        githubSyncStatus.setTextSize(12.0f);
        githubSyncStatus.setTextColor(Color.rgb(73, 84, 100));
        githubSyncStatus.setPadding(0, dp(4), 0, dp(2));
        moreToolsBody.addView(githubSyncStatus, fullWidth());
        refreshGitHubSyncStatus();

        Button githubSettingsToggle = new Button(this);
        githubSettingsToggle.setText("GitHub Sync Settings");
        githubSettingsToggle.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                toggleGitHubSettings();
            }
        });
        moreToolsBody.addView(githubSettingsToggle, fullWidth());

        SharedPreferences githubPrefs = getSharedPreferences(GITHUB_PREFS, MODE_PRIVATE);
        githubSettingsBody = new LinearLayout(this);
        githubSettingsBody.setOrientation(LinearLayout.VERTICAL);
        githubSettingsBody.setVisibility(View.GONE);
        githubTokenEditor = new EditText(this);
        githubTokenEditor.setHint("GitHub token (Contents: write)");
        githubTokenEditor.setSingleLine(true);
        githubTokenEditor.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_PASSWORD);
        githubTokenEditor.setText(readSecretPreference(githubPrefs, GITHUB_PREF_TOKEN));
        githubSettingsBody.addView(githubTokenEditor, fullWidth());
        githubRepositoryEditor = new EditText(this);
        githubRepositoryEditor.setHint("owner/repository");
        githubRepositoryEditor.setSingleLine(true);
        githubRepositoryEditor.setText(readGitHubProjectPreference(githubPrefs, GITHUB_PREF_REPOSITORY, ""));
        githubSettingsBody.addView(githubRepositoryEditor, fullWidth());
        githubBranchEditor = new EditText(this);
        githubBranchEditor.setHint("Branch");
        githubBranchEditor.setSingleLine(true);
        githubBranchEditor.setText(readGitHubProjectPreference(githubPrefs, GITHUB_PREF_BRANCH, "main"));
        githubSettingsBody.addView(githubBranchEditor, fullWidth());
        githubAutoSync = new CheckBox(this);
        githubAutoSync.setText("Automatically back up validated project changes");
        githubAutoSync.setChecked(githubPrefs.getBoolean(
                githubProjectPreferenceKey(GITHUB_PREF_AUTO_SYNC), false));
        githubAutoSync.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) {
                getSharedPreferences(GITHUB_PREFS, MODE_PRIVATE).edit()
                        .putBoolean(githubProjectPreferenceKey(GITHUB_PREF_AUTO_SYNC),
                                githubAutoSync.isChecked())
                        .apply();
                if (githubAutoSync.isChecked()) requestGitHubAutoSync();
            }
        });
        githubSettingsBody.addView(githubAutoSync, fullWidth());
        Button saveGitHubSettings = new Button(this);
        saveGitHubSettings.setText("Save GitHub Sync Settings");
        saveGitHubSettings.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                saveGitHubSyncSettings();
            }
        });
        githubSettingsBody.addView(saveGitHubSettings, fullWidth());
        Button syncNow = new Button(this);
        syncNow.setText("Sync GitHub Now");
        syncNow.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { queueGitHubSync(); }
        });
        githubSettingsBody.addView(syncNow, fullWidth());
        Button reviewGitHubChanges = new Button(this);
        reviewGitHubChanges.setText("Review GitHub Changes");
        reviewGitHubChanges.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { reviewGitHubPullRequestChanges(); }
        });
        githubSettingsBody.addView(reviewGitHubChanges, fullWidth());
        Button createPullRequest = new Button(this);
        createPullRequest.setText("Create / Update Pull Request");
        createPullRequest.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { queueGitHubPullRequest(); }
        });
        githubSettingsBody.addView(createPullRequest, fullWidth());
        Button retryGitHubOperation = new Button(this);
        retryGitHubOperation.setText("Retry GitHub Operation");
        retryGitHubOperation.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { retryGitHubOperation(); }
        });
        githubSettingsBody.addView(retryGitHubOperation, fullWidth());
        moreToolsBody.addView(githubSettingsBody, fullWidth());

        Button privacyToggle = new Button(this);
        privacyToggle.setText("Privacy & Data");
        privacyToggle.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) {
                privacySettingsBody.setVisibility(
                        privacySettingsBody.getVisibility() == View.VISIBLE ? View.GONE : View.VISIBLE);
            }
        });
        moreToolsBody.addView(privacyToggle, fullWidth());
        privacySettingsBody = new LinearLayout(this);
        privacySettingsBody.setOrientation(LinearLayout.VERTICAL);
        privacySettingsBody.setVisibility(View.GONE);
        TextView privacyDisclosure = new TextView(this);
        privacyDisclosure.setText("On-device by default: project code, assets, drafts and recovery. "
                + "GitHub receives project files only when Sync or PR is pressed, or after you explicitly enable automatic backup. "
                + "Microphone access is used only for audio recording.");
        privacyDisclosure.setTextSize(12.0f);
        privacyDisclosure.setTextColor(Color.rgb(73, 84, 100));
        privacyDisclosure.setPadding(dp(8), dp(8), dp(8), dp(8));
        privacySettingsBody.addView(privacyDisclosure, fullWidth());
        Button revokeGitHub = new Button(this);
        revokeGitHub.setText("Revoke GitHub Token");
        revokeGitHub.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { revokeGitHubCredential(); }
        });
        privacySettingsBody.addView(revokeGitHub, fullWidth());
        Button exportSupport = new Button(this);
        exportSupport.setText("Export Redacted Support Bundle");
        exportSupport.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { requestSupportBundleExport(); }
        });
        privacySettingsBody.addView(exportSupport, fullWidth());
        Button clearCrash = new Button(this);
        clearCrash.setText("Clear Local Crash Record");
        clearCrash.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) {
                try {
                    AndroidCrashStore.clear(MainActivity.this);
                    restartLoopRecoveryActive = false;
                    startGameLoop();
                    setStatusText("Local redacted crash record cleared");
                } catch (Exception error) {
                    setStatusText("Crash record clear failed: " + error.getMessage());
                }
            }
        });
        privacySettingsBody.addView(clearCrash, fullWidth());
        Button deleteProject = new Button(this);
        deleteProject.setText("Delete Active Non-Bundled Project");
        deleteProject.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { confirmDeleteActiveProject(); }
        });
        privacySettingsBody.addView(deleteProject, fullWidth());
        moreToolsBody.addView(privacySettingsBody, fullWidth());

        Button onboardingToggle = new Button(this);
        onboardingToggle.setText("Help & Onboarding");
        onboardingToggle.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) {
                onboardingBody.setVisibility(onboardingBody.getVisibility() == View.VISIBLE ? View.GONE : View.VISIBLE);
            }
        });
        moreToolsBody.addView(onboardingToggle, fullWidth());
        onboardingBody = new LinearLayout(this);
        onboardingBody.setOrientation(LinearLayout.VERTICAL);
        onboardingBody.setVisibility(View.GONE);
        onboardingSummary = new TextView(this);
        onboardingSummary.setTextSize(12.0f);
        onboardingSummary.setTextColor(Color.rgb(73, 84, 100));
        onboardingSummary.setPadding(dp(8), dp(8), dp(8), dp(8));
        onboardingBody.addView(onboardingSummary, fullWidth());
        refreshOnboardingSummary();
        Button showWelcome = new Button(this);
        showWelcome.setText("Show Welcome Guide");
        showWelcome.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { showOnboardingGuide(false); }
        });
        onboardingBody.addView(showWelcome, fullWidth());
        Button startManual = new Button(this);
        startManual.setText("Resume Manual Tutorial");
        startManual.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { startManualTutorial(); }
        });
        onboardingBody.addView(startManual, fullWidth());
        Button restartManual = new Button(this);
        restartManual.setText("Restart Manual Tutorial");
        restartManual.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { restartManualTutorial(); }
        });
        onboardingBody.addView(restartManual, fullWidth());
        moreToolsBody.addView(onboardingBody, fullWidth());
        controls.addView(moreToolsBody, fullWidth());
        return controls;
    }

    private void showProjectChooser() {
        if (isFinishing()) return;
        final ArrayList<WorkshopProjectRegistry.ProjectInfo> projects = new ArrayList<>();
        try {
            projects.addAll(WorkshopProjectRegistry.list(this));
        } catch (Exception error) {
            setStatusText("Project list unavailable: " + error.getMessage());
            return;
        }
        if (projects.isEmpty()) {
            showNewProjectDialog();
            return;
        }
        String[] labels = new String[projects.size()];
        int current = 0;
        for (int index = 0; index < projects.size(); index += 1) {
            WorkshopProjectRegistry.ProjectInfo project = projects.get(index);
            labels[index] = project.name + (project.templateId.isEmpty()
                    ? " - imported" : " - " + project.templateId);
            if (activeProject != null && activeProject.id.equals(project.id)) current = index;
        }
        final int[] selected = new int[] { current };
        new AlertDialog.Builder(this)
                .setTitle("Choose project")
                .setSingleChoiceItems(labels, current, new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        selected[0] = which;
                    }
                })
                .setPositiveButton("Open", new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        WorkshopProjectRegistry.ProjectInfo project = projects.get(selected[0]);
                        if (activeProject != null && activeProject.id.equals(project.id)) {
                            recordOnboardingProjectOpened(project);
                            setStatusText("Working on " + project.name);
                        } else {
                            activateProject(project);
                        }
                    }
                })
                .setNeutralButton("New", new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        showNewProjectDialog();
                    }
                })
                .setNegativeButton("Current", new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        if (activeProject != null) {
                            recordOnboardingProjectOpened(activeProject);
                        }
                        setStatusText(activeProject == null ? "Using current workspace"
                                : "Working on " + activeProject.name);
                    }
                })
                .show();
    }

    private void showNewProjectDialog() {
        final LinearLayout content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        content.setPadding(dp(20), dp(4), dp(20), 0);
        final EditText name = new EditText(this);
        name.setHint("Project name");
        name.setSingleLine(true);
        content.addView(name, fullWidth());
        final Spinner templates = new Spinner(this);
        ArrayAdapter<WorkshopTemplateCatalog.Template> adapter = new ArrayAdapter<>(this,
                android.R.layout.simple_spinner_item, WorkshopTemplateCatalog.list());
        adapter.setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item);
        templates.setAdapter(adapter);
        content.addView(templates, fullWidth());
        new AlertDialog.Builder(this)
                .setTitle("New project")
                .setMessage("Choose a bundled template and give the new app-private project a name. "
                        + "You can switch or export it later under Projects.")
                .setView(content)
                .setPositiveButton("Create", new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        try {
                            WorkshopTemplateCatalog.Template template =
                                    (WorkshopTemplateCatalog.Template)templates.getSelectedItem();
                            WorkshopProjectRegistry.ProjectInfo project = WorkshopProjectRegistry.createFromTemplate(
                                    MainActivity.this, name.getText().toString(), template.id);
                            activateProject(project);
                        } catch (Exception error) {
                            setStatusText("Project creation failed: " + error.getMessage());
                            gameLoopHandler.post(new Runnable() {
                                @Override public void run() { showNewProjectDialog(); }
                            });
                        }
                    }
                })
                .setNegativeButton("Back", new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        showProjectChooser();
                    }
                })
                .show();
    }

    private void toggleProjectSettings() {
        if (projectSettingsBody != null) {
            projectSettingsBody.setVisibility(projectSettingsBody.getVisibility() == View.VISIBLE ? View.GONE : View.VISIBLE);
        }
    }

    private void refreshProjectControls() {
        if (projectSelector == null || projectStatus == null) return;
        try {
            availableProjects.clear();
            availableProjects.addAll(WorkshopProjectRegistry.list(this));
            ArrayAdapter<WorkshopProjectRegistry.ProjectInfo> adapter = new ArrayAdapter<>(
                    this, android.R.layout.simple_spinner_item, availableProjects);
            adapter.setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item);
            projectSelector.setAdapter(adapter);
            int selected = 0;
            for (int index = 0; index < availableProjects.size(); index += 1) {
                if (activeProject != null && availableProjects.get(index).id.equals(activeProject.id)) selected = index;
            }
            if (!availableProjects.isEmpty()) projectSelector.setSelection(selected);
            projectStatus.setText(activeProject == null
                    ? "Active project: legacy workspace"
                    : "Active project: " + activeProject.name + " (format "
                            + WorkshopProjectRegistry.FORMAT_VERSION + (activeProject.templateId.isEmpty()
                                    ? ", imported" : ", template " + activeProject.templateId) + ")");
        } catch (Exception error) {
            projectStatus.setText("Project registry error: " + error.getMessage());
        }
    }

    private void switchSelectedProject() {
        Object selected = projectSelector == null ? null : projectSelector.getSelectedItem();
        if (!(selected instanceof WorkshopProjectRegistry.ProjectInfo)) {
            setStatusText("Select a registered project first");
            return;
        }
        activateProject((WorkshopProjectRegistry.ProjectInfo)selected);
    }

    private void createAndSwitchProject() {
        if (isGitHubOperationActive() || projectIoActive || audioRecordingActive
                || pendingExportProject != null || !pendingImportProjectName.isEmpty()) {
            setStatusText("Project creation blocked while GitHub or project I/O is active");
            return;
        }
        if (hasPendingSourceEdit()) {
            setStatusText("Apply or Reset the pending source edit before creating a project");
            return;
        }
        String name = newProjectNameEditor == null ? "" : newProjectNameEditor.getText().toString();
        try {
            Object selectedTemplate = templateSelector == null ? null : templateSelector.getSelectedItem();
            if (!(selectedTemplate instanceof WorkshopTemplateCatalog.Template)) {
                setStatusText("Select a bundled template before creating the project");
                return;
            }
            WorkshopTemplateCatalog.Template template = (WorkshopTemplateCatalog.Template)selectedTemplate;
            WorkshopProjectRegistry.ProjectInfo project = WorkshopProjectRegistry.createFromTemplate(
                    this, name, template.id);
            activateProject(project);
            newProjectNameEditor.setText("");
        } catch (Exception error) {
            setStatusText("Project creation failed: " + error.getMessage());
        }
    }

    boolean activateProject(WorkshopProjectRegistry.ProjectInfo project) {
        if (isGitHubOperationActive() || projectIoActive || audioRecordingActive
                || pendingExportProject != null || !pendingImportProjectName.isEmpty()) {
            setStatusText("Project switch blocked while GitHub or project I/O is active");
            return false;
        }
        if (hasPendingSourceEdit()) {
            setStatusText("Apply or Reset the pending source edit before switching projects");
            return false;
        }
        try {
            WorkshopProjectRegistry.setActive(this, project);
            shutdownGameAudio();
            activeProject = project;
            lastExplorationAudioSerial = 0;
            projectRootFile = project.root;
            projectRootPath = project.root.getAbsolutePath();
            stopAudioPreview();
            cancelAudioRecording(false);
            selectedSymbol = null;
            diagnosticFile = "";
            diagnosticSymbol = "";
            compileAttempted = false;
            compileReady = false;
            gameRuntimeActive = false;
            lastCompileResult = "CompileNotRun";
            reviewedGitHubChangeFingerprint = "";
            ProjectSnapshot snapshot = loadAndMigrateActiveBundledProject();
            ensureActiveProjectBaseline(snapshot);
            rebuildSymbolList(snapshot);
            if (snapshot.firstSymbol != null) showSymbol(snapshot.firstSymbol);
            refreshChangeSummary(snapshot);
            refreshRecoveryStatus();
            refreshGitHubSettingsEditors();
            refreshGitHubSyncStatus();
            refreshProjectControls();
            refreshImageAssetList();
            refreshAudioAssetList();
            String compileResult = nativeCompileProject(projectRootPath());
            lastCompileResult = compileResult;
            compileReady = isRunnableCompile(compileResult);
            compileAttempted = true;
            recordOnboardingProjectOpened(project);
            setStatusText(compileReady ? "Working on " + project.name
                    : "Unable to run " + project.name + " - " + compileResult);
            return true;
        } catch (Exception error) {
            setStatusText("Project switch failed: " + error.getMessage());
            return false;
        }
    }

    private void shutdownGameAudio() {
        if (audioFocus != null) audioFocus.pause();
        nativeAudioShutdown();
    }

    private boolean hasPendingSourceEdit() {
        return selectedSymbol != null && sourceEditor != null
                && !sourceEditor.getText().toString().trim().equals(selectedSymbol.source.trim());
    }

    private void requestProjectExport() {
        if (activeProject == null) {
            setStatusText("Project export needs a registered active project");
            return;
        }
        if (isGitHubOperationActive() || projectIoActive || audioRecordingActive
                || pendingExportProject != null || !pendingImportProjectName.isEmpty()) {
            setStatusText("Project export blocked while other background work is active");
            return;
        }
        if (hasPendingSourceEdit()) {
            setStatusText("Apply or Reset the pending source edit before export");
            return;
        }
        pendingExportProject = activeProject;
        Intent intent = new Intent(Intent.ACTION_CREATE_DOCUMENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        intent.setType("application/zip");
        intent.putExtra(Intent.EXTRA_TITLE, "stasis-project-" + activeProject.id + ".zip");
        intent.addFlags(Intent.FLAG_GRANT_WRITE_URI_PERMISSION);
        try {
            startActivityForResult(intent, EXPORT_PROJECT_REQUEST);
        } catch (Exception error) {
            pendingExportProject = null;
            setStatusText("Project export picker failed: " + error.getMessage());
        }
    }

    private void requestProjectImport() {
        if (isGitHubOperationActive() || projectIoActive || audioRecordingActive
                || pendingExportProject != null || !pendingImportProjectName.isEmpty()) {
            setStatusText("Project import blocked while other background work is active");
            return;
        }
        if (hasPendingSourceEdit()) {
            setStatusText("Apply or Reset the pending source edit before import");
            return;
        }
        String name = newProjectNameEditor == null ? "" : newProjectNameEditor.getText().toString().trim();
        try {
            WorkshopProjectRegistry.validateRequestedName(name);
        } catch (Exception error) {
            setStatusText("Project import needs a valid new project name");
            return;
        }
        pendingImportProjectName = name;
        Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        intent.setType("application/zip");
        intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
        try {
            startActivityForResult(intent, IMPORT_PROJECT_REQUEST);
        } catch (Exception error) {
            pendingImportProjectName = "";
            setStatusText("Project import picker failed: " + error.getMessage());
        }
    }

    private void refreshGitHubSettingsEditors() {
        SharedPreferences preferences = getSharedPreferences(GITHUB_PREFS, MODE_PRIVATE);
        if (githubRepositoryEditor != null) {
            githubRepositoryEditor.setText(readGitHubProjectPreference(preferences, GITHUB_PREF_REPOSITORY, ""));
        }
        if (githubBranchEditor != null) {
            githubBranchEditor.setText(readGitHubProjectPreference(preferences, GITHUB_PREF_BRANCH, "main"));
        }
        if (githubAutoSync != null) {
            githubAutoSync.setChecked(preferences.getBoolean(
                    githubProjectPreferenceKey(GITHUB_PREF_AUTO_SYNC), false));
        }
    }

    private void toggleGitHubSettings() {
        if (githubSettingsBody != null) {
            githubSettingsBody.setVisibility(githubSettingsBody.getVisibility() == View.VISIBLE ? View.GONE : View.VISIBLE);
        }
    }

    private String readSecretPreference(SharedPreferences preferences, String key) {
        try {
            String value = AndroidSecretStore.readAndMigrate(preferences, key);
            credentialStorageError = "";
            return value;
        } catch (Exception error) {
            credentialStorageError = error.getMessage() == null ? error.getClass().getSimpleName() : error.getMessage();
            return "";
        }
    }

    private boolean writeSecretPreference(SharedPreferences preferences, String key, String value) {
        try {
            AndroidSecretStore.write(preferences, key, value);
            credentialStorageError = "";
            return true;
        } catch (Exception error) {
            credentialStorageError = error.getMessage() == null ? error.getClass().getSimpleName() : error.getMessage();
            setStatusText("Credential storage error: " + credentialStorageError);
            return false;
        }
    }

    private void saveGitHubSyncSettings() {
        final String token = githubTokenEditor == null ? "" : githubTokenEditor.getText().toString().trim();
        final String repository = githubRepositoryEditor == null ? "" : githubRepositoryEditor.getText().toString().trim();
        final String branchValue = githubBranchEditor == null ? "" : githubBranchEditor.getText().toString().trim();
        final String branch = branchValue.isEmpty() ? "main" : branchValue;
        if (token.isEmpty() || repository.indexOf('/') <= 0 || repository.endsWith("/")) {
            setStatusText("GitHub sync settings need a token and owner/repository");
            return;
        }
        if (!WorkshopConnectivity.hasUsableNetwork(this)) {
            setStatusText("GitHub settings need a usable network for authenticated validation");
            return;
        }
        if (!beginGitHubOperation("validate", "GitHub sync: validating repository and branch")) return;
        githubSyncExecutor.submit(new Runnable() {
            @Override public void run() {
                try {
                    new WorkshopGitHubApi(token, repository).validateTarget(branch);
                    runOnUiThread(new Runnable() {
                        @Override public void run() {
                            SharedPreferences preferences = getSharedPreferences(GITHUB_PREFS, MODE_PRIVATE);
                            String previousRepository = readGitHubProjectPreference(
                                    preferences, GITHUB_PREF_REPOSITORY, "");
                            String previousBranch = readGitHubProjectPreference(
                                    preferences, GITHUB_PREF_BRANCH, "main");
                            if (!writeSecretPreference(preferences, GITHUB_PREF_TOKEN, token)) {
                                postGitHubOperationState("", "error",
                                        "GitHub sync: credential storage failed after validation");
                                return;
                            }
                            SharedPreferences.Editor editor = preferences.edit()
                                    .putString(githubProjectPreferenceKey(GITHUB_PREF_REPOSITORY), repository)
                                    .putString(githubProjectPreferenceKey(GITHUB_PREF_BRANCH), branch)
                                    .putString(githubProjectPreferenceKey(GITHUB_PREF_VALIDATED_TARGET),
                                            WorkshopGitHubSyncPolicy.targetIdentity(repository, branch));
                            if (!repository.equals(previousRepository) || !branch.equals(previousBranch)) {
                                editor.remove(githubProjectPreferenceKey(GITHUB_PREF_REMOTE_STATE));
                                editor.remove(githubProjectPreferenceKey(GITHUB_PREF_LAST_SYNC_FINGERPRINT));
                                editor.remove(githubProjectPreferenceKey(GITHUB_PREF_REVIEW_FINGERPRINT));
                                reviewedGitHubChangeFingerprint = "";
                            }
                            editor.apply();
                            postGitHubOperationState("", "complete",
                                    "GitHub sync: authenticated target ready for " + repository + ":" + branch);
                            requestGitHubAutoSync();
                        }
                    });
                } catch (Exception error) {
                    postGitHubOperationState("", "error",
                            "GitHub validation error: " + error.getMessage());
                }
            }
        });
    }

    private void refreshGitHubSyncStatus() {
        if (githubSyncStatus == null) {
            return;
        }
        SharedPreferences prefs = getSharedPreferences(GITHUB_PREFS, MODE_PRIVATE);
        String repository = readGitHubProjectPreference(prefs, GITHUB_PREF_REPOSITORY, "").trim();
        String token = readSecretPreference(prefs, GITHUB_PREF_TOKEN).trim();
        if (!credentialStorageError.isEmpty()) {
            githubSyncStatus.setText("GitHub sync: credential storage error");
            return;
        }
        if (token.isEmpty() || repository.indexOf('/') <= 0) {
            githubSyncStatus.setText("GitHub sync: not configured");
            return;
        }
        String branch = readGitHubProjectPreference(prefs, GITHUB_PREF_BRANCH, "main").trim();
        if (!githubTargetValidated(prefs, repository, branch)) {
            githubSyncStatus.setText("GitHub sync: save settings to authenticate this target");
            return;
        }
        String operation = prefs.getString(githubProjectPreferenceKey(GITHUB_PREF_OPERATION), "");
        String state = prefs.getString(githubProjectPreferenceKey(GITHUB_PREF_OPERATION_STATE), "");
        String detail = prefs.getString(githubProjectPreferenceKey(GITHUB_PREF_OPERATION_DETAIL), "");
        boolean inProcessOperationActive = WorkshopLongWorkCoordinator.isGitHubActive();
        if (("queued".equals(state) || "running".equals(state)) && !operation.isEmpty()) {
            if (inProcessOperationActive) {
                githubSyncStatus.setText("GitHub sync: continues in background");
                return;
            }
            if (WorkshopGitHubSyncPolicy.shouldMarkInterrupted(
                    operation, state, inProcessOperationActive)) {
                persistGitHubOperationState(operation, "interrupted", "app stopped before completion");
                githubSyncStatus.setText("GitHub sync: interrupted; retry available");
                return;
            }
        }
        if (("waiting_network".equals(state) || "deferred".equals(state)) && !operation.isEmpty()) {
            githubSyncStatus.setText(detail.isEmpty() ? "GitHub sync: waiting to retry" : detail);
            return;
        }
        if (("error".equals(state) || "interrupted".equals(state)) && !operation.isEmpty()) {
            githubSyncStatus.setText("GitHub sync: retry available" + (detail.isEmpty() ? "" : " - " + detail));
            return;
        }
        githubSyncStatus.setText("GitHub sync: ready for " + repository);
    }

    private void queueGitHubSync() {
        queueGitHubSync(true);
    }

    private void queueGitHubSync(boolean userInitiated) {
        if (audioRecordingActive) {
            setStatusText("Finish or cancel audio recording before GitHub sync");
            return;
        }
        final SharedPreferences prefs = getSharedPreferences(GITHUB_PREFS, MODE_PRIVATE);
        final String token = readSecretPreference(prefs, GITHUB_PREF_TOKEN).trim();
        final String repository = readGitHubProjectPreference(prefs, GITHUB_PREF_REPOSITORY, "").trim();
        final String branch = readGitHubProjectPreference(prefs, GITHUB_PREF_BRANCH, "main").trim();
        if (token.isEmpty() || repository.indexOf('/') <= 0) {
            setStatusText("GitHub sync needs configured settings");
            return;
        }
        if (!githubTargetValidated(prefs, repository, branch)) {
            setStatusText("GitHub sync needs authenticated settings; save them again");
            return;
        }
        WorkshopBackgroundWorkPolicy.Decision background = WorkshopBackgroundWorkPolicy.decide(
                userInitiated, WorkshopConnectivity.hasUsableNetwork(this),
                batterySaverEnabled(), deviceCharging());
        if (background == WorkshopBackgroundWorkPolicy.Decision.WAIT_FOR_NETWORK) {
            persistGitHubSyncOperationState("waiting_network",
                    "GitHub sync: waiting for a usable network", !userInitiated);
            refreshGitHubSyncStatus();
            return;
        }
        if (background == WorkshopBackgroundWorkPolicy.Decision.DEFER_FOR_BATTERY) {
            persistGitHubSyncOperationState("deferred",
                    "GitHub sync: automatic backup deferred by battery saver", !userInitiated);
            refreshGitHubSyncStatus();
            return;
        }
        final String remoteStateKey = githubProjectPreferenceKey(GITHUB_PREF_REMOTE_STATE);
        final String fingerprintKey = githubProjectPreferenceKey(GITHUB_PREF_LAST_SYNC_FINGERPRINT);
        if (!beginGitHubOperation("sync", "GitHub sync: queued", !userInitiated)) return;
        githubSyncExecutor.submit(new Runnable() {
            @Override public void run() {
                try {
                    Map<String, byte[]> files = githubBackupFiles();
                    SharedPreferences preferences = getSharedPreferences(GITHUB_PREFS, MODE_PRIVATE);
                    Map<String, String> remoteState = WorkshopGitHubSyncPolicy.decodeRemoteState(
                            preferences.getString(remoteStateKey, ""));
                    List<WorkshopGitHubSyncPolicy.Change> plan =
                            WorkshopGitHubSyncPolicy.backupPlan(files, remoteState);
                    WorkshopGitHubApi api = new WorkshopGitHubApi(token, repository);
                    int completed = 0;
                    int deleted = 0;
                    for (WorkshopGitHubSyncPolicy.Change change : plan) {
                        completed += 1;
                        postGitHubOperationState("sync", "running",
                                "GitHub sync: " + completed + "/" + plan.size());
                        String remoteSha = api.applyFileChange(
                                branch, change.path, change.content, remoteState.get(change.path));
                        if (change.deletesRemoteFile()) {
                            remoteState.remove(change.path);
                            deleted += 1;
                        } else {
                            remoteState.put(change.path, remoteSha);
                        }
                        preferences.edit().putString(remoteStateKey,
                                WorkshopGitHubSyncPolicy.encodeRemoteState(remoteState)).apply();
                    }
                    preferences.edit()
                            .putString(fingerprintKey, WorkshopGitHubSyncPolicy.fingerprint(files))
                            .putString(remoteStateKey,
                                    WorkshopGitHubSyncPolicy.encodeRemoteState(remoteState))
                            .apply();
                    postGitHubOperationState("", "complete", plan.isEmpty()
                            ? "GitHub sync: no project files"
                            : "GitHub sync: complete (" + (completed - deleted)
                                    + " uploaded, " + deleted + " deleted)");
                } catch (final Exception error) {
                    postGitHubOperationFailure("sync", "GitHub sync", error);
                }
            }
        });
    }

    private void requestGitHubAutoSync() {
        gameLoopHandler.removeCallbacks(githubAutoSyncRequest);
        gameLoopHandler.postDelayed(githubAutoSyncRequest, GITHUB_AUTO_SYNC_DEBOUNCE_MS);
    }

    private void scheduleGitHubAutoSync() {
        if (activityDestroyed || WorkshopLongWorkCoordinator.isAnyActive()
                || audioRecordingActive || hasPendingSourceEdit() || !compileReady) return;
        SharedPreferences preferences = getSharedPreferences(GITHUB_PREFS, MODE_PRIVATE);
        boolean enabled = preferences.getBoolean(
                githubProjectPreferenceKey(GITHUB_PREF_AUTO_SYNC), false);
        String repository = readGitHubProjectPreference(
                preferences, GITHUB_PREF_REPOSITORY, "").trim();
        String branch = readGitHubProjectPreference(
                preferences, GITHUB_PREF_BRANCH, "main").trim();
        if (!githubTargetValidated(preferences, repository, branch)) return;
        String operation = preferences.getString(
                githubProjectPreferenceKey(GITHUB_PREF_OPERATION), "");
        String state = preferences.getString(
                githubProjectPreferenceKey(GITHUB_PREF_OPERATION_STATE), "");
        if (!operation.isEmpty() && ("error".equals(state) || "interrupted".equals(state))
                && !"sync".equals(operation)) return;
        try {
            Map<String, byte[]> files = githubBackupFiles();
            String current = WorkshopGitHubSyncPolicy.fingerprint(files);
            String previous = preferences.getString(
                    githubProjectPreferenceKey(GITHUB_PREF_LAST_SYNC_FINGERPRINT), "");
            WorkshopGitHubSyncPolicy.ScheduleDecision decision =
                    WorkshopGitHubSyncPolicy.automaticSchedule(enabled, !current.equals(previous),
                            WorkshopConnectivity.hasUsableNetwork(this),
                            batterySaverEnabled(), deviceCharging());
            if (decision == WorkshopGitHubSyncPolicy.ScheduleDecision.RUN) {
                queueGitHubSync(false);
            } else if (decision == WorkshopGitHubSyncPolicy.ScheduleDecision.WAIT_FOR_NETWORK) {
                persistGitHubSyncOperationState("waiting_network",
                        "GitHub sync: automatic backup waiting for a usable network", true);
                refreshGitHubSyncStatus();
            } else if (decision == WorkshopGitHubSyncPolicy.ScheduleDecision.DEFER_FOR_BATTERY) {
                persistGitHubSyncOperationState("deferred",
                        "GitHub sync: automatic backup deferred by battery saver", true);
                refreshGitHubSyncStatus();
            }
        } catch (Exception error) {
            persistGitHubOperationState("sync", "error",
                    "GitHub automatic backup error: " + error.getMessage());
            refreshGitHubSyncStatus();
        }
    }

    private void revokeGitHubCredential() {
        if (isGitHubOperationActive()) {
            setStatusText("GitHub token revocation blocked until the active operation finishes");
            return;
        }
        new AlertDialog.Builder(this)
                .setTitle("Revoke GitHub Token?")
                .setMessage("The encrypted credential is removed. Repository/branch settings and project files remain.")
                .setPositiveButton("Revoke", new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        SharedPreferences preferences = getSharedPreferences(GITHUB_PREFS, MODE_PRIVATE);
                        if (!writeSecretPreference(preferences, GITHUB_PREF_TOKEN, "")) return;
                        if (githubTokenEditor != null) githubTokenEditor.setText("");
                        refreshGitHubSyncStatus();
                        setStatusText("GitHub token revoked from encrypted storage");
                    }
                })
                .setNegativeButton("Cancel", null)
                .show();
    }

    private void requestSupportBundleExport() {
        if (isGitHubOperationActive() || projectIoActive) {
            setStatusText("Support export blocked while background work is active");
            return;
        }
        new AlertDialog.Builder(this)
                .setTitle("Export Redacted Support Bundle?")
                .setMessage("Includes app/device versions, project file counts, compile/reload state, operation states, "
                        + "prior redacted crash type/class-method frames. Excludes credentials, source, "
                        + "file/media names and bytes, repository names, and absolute paths.")
                .setPositiveButton("Choose Destination", new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        Intent intent = new Intent(Intent.ACTION_CREATE_DOCUMENT);
                        intent.addCategory(Intent.CATEGORY_OPENABLE);
                        intent.setType("application/json");
                        intent.putExtra(Intent.EXTRA_TITLE, "stasis-android-support-redacted.json");
                        intent.addFlags(Intent.FLAG_GRANT_WRITE_URI_PERMISSION);
                        try {
                            startActivityForResult(intent, EXPORT_SUPPORT_BUNDLE_REQUEST);
                        } catch (Exception error) {
                            setStatusText("Support export picker failed: " + error.getMessage());
                        }
                    }
                })
                .setNegativeButton("Cancel", null)
                .show();
    }

    private void completeSupportBundleExport(int resultCode, Intent data) {
        if (resultCode != RESULT_OK || data == null || data.getData() == null) {
            setStatusText("Support export cancelled");
            return;
        }
        final Uri destination = data.getData();
        final WorkshopProjectRegistry.ProjectInfo project = activeProject;
        final File root = projectRoot();
        final String compile = lastCompileResult;
        SharedPreferences github = getSharedPreferences(GITHUB_PREFS, MODE_PRIVATE);
        final String operation = github.getString(githubProjectPreferenceKey(GITHUB_PREF_OPERATION), "");
        final String state = github.getString(githubProjectPreferenceKey(GITHUB_PREF_OPERATION_STATE), "");
        if (!beginProjectIoWork("Building a redacted support bundle")) return;
        setStatusText("Building redacted support bundle");
        projectIoExecutor.submit(new Runnable() {
            @Override public void run() {
                try {
                    String bundle = AndroidSupportBundle.build(MainActivity.this, project, root, compile,
                            operation, state);
                    OutputStream output = getContentResolver().openOutputStream(destination, "w");
                    if (output == null) throw new IOException("document provider did not open the destination");
                    try {
                        output.write(bundle.getBytes(StandardCharsets.UTF_8));
                    } finally {
                        output.close();
                    }
                    runOnUiThread(new Runnable() {
                        @Override public void run() {
                            setStatusText("Redacted support bundle exported without credentials, source, prompts, or media");
                        }
                    });
                } catch (final Exception error) {
                    runOnUiThread(new Runnable() {
                        @Override public void run() { setStatusText("Support export failed: " + error.getMessage()); }
                    });
                } finally {
                    finishProjectIoWork();
                }
            }
        });
    }

    private void confirmDeleteActiveProject() {
        if (activeProject == null || WorkshopProjectRegistry.LEGACY_PROJECT_DIR.equals(activeProject.directoryName)) {
            setStatusText("Bundled Workshop cannot be deleted");
            return;
        }
        if (isGitHubOperationActive() || projectIoActive || audioRecordingActive) {
            setStatusText("Project deletion blocked while background work or recording is active");
            return;
        }
        if (hasPendingSourceEdit()) {
            setStatusText("Apply or Reset the pending source edit before deleting this project");
            return;
        }
        final WorkshopProjectRegistry.ProjectInfo target = activeProject;
        final EditText confirmation = new EditText(this);
        confirmation.setHint("Type " + target.name + " to confirm");
        confirmation.setSingleLine(true);
        new AlertDialog.Builder(this)
                .setTitle("Delete Project Permanently?")
                .setMessage("Export a project archive first if it may be needed. This deletes the project, accepted assets, "
                        + "trash, baseline, draft/recovery journal, and project-scoped GitHub state. The bundled project and credentials remain.")
                .setView(confirmation)
                .setPositiveButton("Delete Project", new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        if (!target.name.equals(confirmation.getText().toString())) {
                            setStatusText("Project deletion cancelled: confirmation name did not match exactly");
                            return;
                        }
                        deleteActiveProject(target);
                    }
                })
                .setNegativeButton("Cancel", null)
                .show();
    }

    private void deleteActiveProject(WorkshopProjectRegistry.ProjectInfo target) {
        try {
            WorkshopProjectRegistry.ProjectInfo bundled = null;
            for (WorkshopProjectRegistry.ProjectInfo project : WorkshopProjectRegistry.list(this)) {
                if (WorkshopProjectRegistry.LEGACY_PROJECT_DIR.equals(project.directoryName)) {
                    bundled = project;
                    break;
                }
            }
            if (bundled == null) throw new IOException("Bundled Workshop recovery target is unavailable");
            File baseline = new File(new File(getFilesDir(), PROJECT_BASELINES_DIR), target.id);
            if (!activateProject(bundled)) throw new IOException("could not switch to Bundled Workshop before deletion");
            WorkshopProjectRegistry.deleteProject(this, target);
            deleteBaselineDirectory(baseline);
            if (baseline.exists()) throw new IOException("project baseline deletion did not complete");
            AndroidDraftStore.clear(this, target.id);
            AndroidEditRecoveryStore.clearProject(this, target.id);
            clearDeletedProjectPreferences(target);
            refreshProjectControls();
            setStatusText("Deleted project and scoped private data: " + target.name + "; Bundled Workshop is active");
        } catch (Exception error) {
            refreshProjectControls();
            setStatusText("Project deletion stopped with recovery context preserved where possible: " + error.getMessage());
        }
    }

    private void clearDeletedProjectPreferences(WorkshopProjectRegistry.ProjectInfo project) {
        SharedPreferences github = getSharedPreferences(GITHUB_PREFS, MODE_PRIVATE);
        SharedPreferences.Editor editor = github.edit();
        String suffix = "_" + project.id;
        for (String key : github.getAll().keySet()) if (key.endsWith(suffix)) editor.remove(key);
        editor.apply();
    }

    private void showOnboardingGuide(boolean firstRun) {
        final WorkshopOnboardingPolicy.Progress progress = onboardingProgress();
        new AlertDialog.Builder(this)
                .setTitle("Welcome to Stasis Workshop")
                .setMessage(WorkshopOnboardingPolicy.checklist(progress))
                .setPositiveButton(progress.isComplete() ? "Restart Tutorial" : "Resume Tutorial",
                        new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        startManualTutorial();
                    }
                })
                .setNegativeButton(firstRun ? "Remind Me Later" : "Close",
                        new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        if (firstRun) {
                            if (persistOnboardingProgress(WorkshopOnboardingPolicy.defer(progress))) {
                                setStatusText("Manual tutorial deferred; resume it anytime under Help & Onboarding");
                            }
                        }
                    }
                })
                .show();
    }

    private WorkshopOnboardingPolicy.Progress onboardingProgress() {
        if (onboardingState != null) return onboardingState;
        onboardingState = WorkshopOnboardingStore.load(
                getSharedPreferences(ONBOARDING_PREFS, MODE_PRIVATE));
        return onboardingState;
    }

    private boolean persistOnboardingProgress(WorkshopOnboardingPolicy.Progress progress) {
        boolean stored = WorkshopOnboardingStore.save(
                getSharedPreferences(ONBOARDING_PREFS, MODE_PRIVATE), progress);
        if (!stored) {
            setStatusText("Tutorial progress could not be saved; the current step remains active");
            return false;
        }
        onboardingState = progress;
        refreshOnboardingSummary();
        return true;
    }

    private void refreshOnboardingSummary() {
        if (onboardingSummary != null) {
            onboardingSummary.setText(WorkshopOnboardingPolicy.checklist(onboardingProgress()));
        }
    }

    private void persistOnboardingAdvance(WorkshopOnboardingPolicy.Progress before,
            WorkshopOnboardingPolicy.Progress after) {
        if (after == before) return;
        if (!persistOnboardingProgress(after)) return;
        String message = after.isComplete()
                ? "Manual tutorial complete; Help & Onboarding can restart it anytime"
                : "Tutorial progress saved. Next: " + after.nextStep().instruction;
        Toast.makeText(this, message, Toast.LENGTH_LONG).show();
    }

    private void recordOnboardingProjectOpened(WorkshopProjectRegistry.ProjectInfo project) {
        if (project != null && WorkshopTemplateCatalog.isKnown(project.templateId)) {
            WorkshopOnboardingPolicy.Progress before = onboardingProgress();
            persistOnboardingAdvance(before,
                    WorkshopOnboardingPolicy.recordProjectOpened(before, project.id));
        }
    }

    private void recordOnboardingProjectStep(WorkshopOnboardingPolicy.Step event) {
        WorkshopOnboardingPolicy.Progress before = onboardingProgress();
        String projectId = activeProject == null ? "" : activeProject.id;
        persistOnboardingAdvance(before,
                WorkshopOnboardingPolicy.recordProjectStep(before, event, projectId));
    }

    private void recordOnboardingChangeApplied(SymbolEntry symbol, String source) {
        WorkshopOnboardingPolicy.Progress before = onboardingProgress();
        String projectId = activeProject == null ? "" : activeProject.id;
        persistOnboardingAdvance(before, WorkshopOnboardingPolicy.recordChangeApplied(
                before, projectId, symbol.kind, symbol.identityKey(), onboardingSourceHash(source)));
    }

    private void recordOnboardingTrackedChangeStep(WorkshopOnboardingPolicy.Step event,
            ProjectSnapshot currentProject) {
        WorkshopOnboardingPolicy.Progress before = onboardingProgress();
        SymbolEntry tracked = findSymbolByIdentityKey(currentProject, before.changeId);
        if (tracked == null) return;
        String projectId = activeProject == null ? "" : activeProject.id;
        persistOnboardingAdvance(before, WorkshopOnboardingPolicy.recordChangeStep(
                before, event, projectId, tracked.identityKey(), onboardingSourceHash(tracked.source)));
    }

    private void recordOnboardingRevert(String changeId, String changeHash) {
        WorkshopOnboardingPolicy.Progress before = onboardingProgress();
        String projectId = activeProject == null ? "" : activeProject.id;
        persistOnboardingAdvance(before, WorkshopOnboardingPolicy.recordChangeStep(
                before, WorkshopOnboardingPolicy.Step.CHANGE_REVERTED,
                projectId, changeId, changeHash));
    }

    private static String onboardingSourceHash(String source) {
        try {
            return sha256Bytes((source == null ? "" : source.trim()).getBytes(StandardCharsets.UTF_8));
        } catch (IOException error) {
            return "";
        }
    }

    private void startManualTutorial() {
        WorkshopOnboardingPolicy.Progress progress = onboardingProgress();
        if (progress.isComplete()) progress = WorkshopOnboardingPolicy.restart();
        progress = WorkshopOnboardingPolicy.resume(progress);
        if (!persistOnboardingProgress(progress)) return;
        if (progress.nextStep() == WorkshopOnboardingPolicy.Step.WELCOME) {
            progress = WorkshopOnboardingPolicy.recordWelcome(progress);
            if (!persistOnboardingProgress(progress)) return;
        }
        if (progress.nextStep() == WorkshopOnboardingPolicy.Step.PROJECT_OPENED) {
            setStatusText("Tutorial: choose Open for the current template, or New to create from another template");
            showProjectChooser();
            return;
        }
        if (progress.nextStep() == WorkshopOnboardingPolicy.Step.PROJECT_RAN) {
            setStatusText("Tutorial: watch the selected project run; the first successful frame completes this step");
            return;
        }
        if (editorPanel != null && editorPanel.getVisibility() != View.VISIBLE) toggleEditorPanel();
        if (manualEditBody != null) manualEditBody.setVisibility(View.VISIBLE);
        if (onboardingBody != null) onboardingBody.setVisibility(View.VISIBLE);
        if (selectedSymbol == null) {
            ProjectSnapshot project = loadBundledProject();
            if (project.firstSymbol != null) showSymbol(project.firstSymbol);
        }
        if (progress.nextStep() == WorkshopOnboardingPolicy.Step.CHANGE_APPLIED) {
            setStatusText("Tutorial: select a function, make a small function-body edit, then Apply; no API key is required");
        } else if (progress.nextStep() == WorkshopOnboardingPolicy.Step.TESTS_PASSED) {
            setStatusText("Tutorial: choose Run Tests and continue when every runnable test passes");
        } else if (progress.nextStep() == WorkshopOnboardingPolicy.Step.CHANGES_REVIEWED) {
            if (diagnosticBody != null) diagnosticBody.setVisibility(View.VISIBLE);
            setStatusText("Tutorial: choose Changes or Raw Diffs and inspect the saved edit");
        } else if (progress.nextStep() == WorkshopOnboardingPolicy.Step.CHANGE_REVERTED) {
            setStatusText("Tutorial: keep the changed baseline symbol selected and choose Revert Saved");
        } else {
            setStatusText("Manual tutorial complete; Help & Onboarding remains available");
        }
        if (editorPanel != null && sourceEditor != null) {
            editorPanel.post(new Runnable() {
                @Override public void run() { editorPanel.smoothScrollTo(0, sourceEditor.getTop()); }
            });
        }
    }

    private void restartManualTutorial() {
        if (!persistOnboardingProgress(WorkshopOnboardingPolicy.restart())) return;
        setStatusText("Manual tutorial restarted with previous project/change context cleared");
        startManualTutorial();
    }

    private void persistPendingDraft() {
        if (selectedSymbol == null || sourceEditor == null) return;
        try {
            String draft = sourceEditor.getText().toString();
            if (draft.trim().equals(selectedSymbol.source.trim())) {
                AndroidDraftStore.clearIfMatches(this, activeRecoveryProjectId(), selectedSymbol.file,
                        selectedSymbol.kind, selectedSymbol.name, selectedSymbol.owner);
                return;
            }
            AndroidDraftStore.save(this, activeRecoveryProjectId(), selectedSymbol.file, selectedSymbol.kind,
                    selectedSymbol.name, selectedSymbol.owner, selectedSymbol.source, draft);
        } catch (Exception error) {
            setStatusText("Draft autosave failed: " + error.getMessage());
        }
    }

    private void restorePendingDraft() {
        try {
            AndroidDraftStore.Entry draft = AndroidDraftStore.load(this, activeRecoveryProjectId());
            if (draft == null) return;
            SymbolEntry target = null;
            for (SymbolSection section : loadBundledProject().sections) {
                for (SymbolGroup group : section.groups) {
                    for (SymbolEntry symbol : group.symbols) {
                        if (symbol.file.equals(draft.path) && symbol.kind.equals(draft.kind)
                                && symbol.name.equals(draft.name) && symbol.owner.equals(draft.owner)) {
                            target = symbol;
                            break;
                        }
                    }
                    if (target != null) break;
                }
                if (target != null) break;
            }
            if (target == null) {
                setStatusText("Unsaved draft retained but its symbol no longer exists");
                return;
            }
            if (!AndroidDraftStore.matchesBase(draft, target.source)) {
                setStatusText("Unsaved draft retained but source changed; recovery will not overwrite newer code");
                return;
            }
            showSymbol(target);
            sourceEditor.setText(draft.draftSource);
            if (manualEditBody != null) manualEditBody.setVisibility(View.VISIBLE);
            setStatusText("Recovered unsaved source draft after app interruption");
        } catch (Exception error) {
            setStatusText("Draft recovery unavailable: " + error.getMessage());
        }
    }

    private void restoreWorkshopUiState(Bundle state) {
        if (state == null) return;
        SymbolEntry restoredSymbol = findSymbolByIdentity(loadBundledProject(),
                state.getString("selected_kind", ""), state.getString("selected_file", ""),
                state.getString("selected_owner", ""), state.getString("selected_name", ""));
        if (restoredSymbol != null) showSymbol(restoredSymbol);
        restoreVisibility(manualEditBody, state.getBoolean("manual_open", false));
        restoreVisibility(diagnosticBody, state.getBoolean("diagnostics_open", false));
        restoreVisibility(moreToolsBody, state.getBoolean("more_tools_open", false));
        restoreVisibility(projectSettingsBody, state.getBoolean("projects_open", false));
        restoreVisibility(githubSettingsBody, state.getBoolean("github_settings_open", false));
        restoreVisibility(privacySettingsBody, state.getBoolean("privacy_open", false));
        restoreVisibility(onboardingBody, state.getBoolean("onboarding_open", false));
        if (state.getBoolean("editor_open", false) && editorPanel != null
                && editorPanel.getVisibility() != View.VISIBLE) {
            toggleEditorPanel();
        }
        final int scrollY = Math.max(0, state.getInt("editor_scroll_y", 0));
        if (editorPanel != null) {
            editorPanel.post(new Runnable() {
                @Override public void run() { editorPanel.scrollTo(0, scrollY); }
            });
        }
    }

    private static void restoreVisibility(View view, boolean visible) {
        if (view != null) view.setVisibility(visible ? View.VISIBLE : View.GONE);
    }


    private void clearPendingDraft() {
        if (selectedSymbol == null) return;
        try {
            AndroidDraftStore.clearIfMatches(this, activeRecoveryProjectId(), selectedSymbol.file,
                    selectedSymbol.kind, selectedSymbol.name, selectedSymbol.owner);
        } catch (Exception error) {
            setStatusText("Draft cleanup failed: " + error.getMessage());
        }
    }

    private Map<String, byte[]> githubBackupFiles() throws IOException {
        Map<String, byte[]> files = new LinkedHashMap<>();
        int totalBytes = 0;
        for (Map.Entry<String, String> source : sourcesByFile(loadBundledProject()).entrySet()) {
            byte[] content = source.getValue().getBytes(StandardCharsets.UTF_8);
            totalBytes = checkedGitHubBackupSize(totalBytes, content.length);
            files.put(source.getKey(), content);
        }
        if (activeProject != null) {
            byte[] assetManifest = WorkshopAssetManifest.readForSync(activeProject.root);
            if (assetManifest != null) {
                totalBytes = checkedGitHubBackupSize(totalBytes, assetManifest.length);
                files.put(WorkshopAssetManifest.RELATIVE_PATH, assetManifest);
            }
            for (WorkshopImageAssets.AssetInfo asset : WorkshopImageAssets.list(activeProject.root)) {
                byte[] content = WorkshopImageAssets.readForSync(asset);
                totalBytes = checkedGitHubBackupSize(totalBytes, content.length);
                files.put(asset.relativePath, content);
            }
            for (WorkshopAudioAssets.AssetInfo asset : WorkshopAudioAssets.list(activeProject.root)) {
                byte[] content = WorkshopAudioAssets.readForSync(asset);
                totalBytes = checkedGitHubBackupSize(totalBytes, content.length);
                files.put(asset.relativePath, content);
            }
        }
        return files;
    }

    private static int checkedGitHubBackupSize(int current, int additional) throws IOException {
        if (additional > MAX_GITHUB_BACKUP_BYTES - current) {
            throw new IOException("project exceeds the 32 MiB direct backup limit");
        }
        return current + additional;
    }

    private static Map<String, String> changedProjectSources(ProjectSnapshot baseline, ProjectSnapshot current) {
        return WorkshopGitHubSyncPolicy.changedTextFiles(
                sourcesByFile(baseline), sourcesByFile(current));
    }

    private void reviewGitHubPullRequestChanges() {
        try {
            ProjectSnapshot baseline = loadProjectBaselineSnapshot();
            ProjectSnapshot current = loadBundledProject();
            Map<String, String> changes = changedProjectSources(baseline, current);
            if (changes.isEmpty()) {
                reviewedGitHubChangeFingerprint = "";
                githubSyncStatus.setText("GitHub review: no local changes");
                return;
            }
            reviewedGitHubChangeFingerprint = githubChangeFingerprint(changes);
            getSharedPreferences(GITHUB_PREFS, MODE_PRIVATE).edit()
                    .putString(githubProjectPreferenceKey(GITHUB_PREF_REVIEW_FINGERPRINT), reviewedGitHubChangeFingerprint).apply();
            changeSummary.setText(formatChangeSummary(baseline, current)
                    + "\n\n" + formatRawFileDiffs(baseline, current));
            githubSyncStatus.setText("GitHub review: ready (" + changes.size() + " files)");
        } catch (IOException error) {
            reviewedGitHubChangeFingerprint = "";
            githubSyncStatus.setText("GitHub review error: " + error.getMessage());
        }
    }

    private void queueGitHubPullRequest() {
        if (audioRecordingActive) {
            setStatusText("Finish or cancel audio recording before GitHub pull request work");
            return;
        }
        final SharedPreferences prefs = getSharedPreferences(GITHUB_PREFS, MODE_PRIVATE);
        final String token = readSecretPreference(prefs, GITHUB_PREF_TOKEN).trim();
        final String repository = readGitHubProjectPreference(prefs, GITHUB_PREF_REPOSITORY, "").trim();
        final String baseBranch = readGitHubProjectPreference(prefs, GITHUB_PREF_BRANCH, "main").trim();
        if (token.isEmpty() || repository.indexOf('/') <= 0) {
            setStatusText("GitHub pull request needs configured settings");
            return;
        }
        if (!githubTargetValidated(prefs, repository, baseBranch)) {
            setStatusText("GitHub pull request needs authenticated settings; save them again");
            return;
        }
        if (!WorkshopConnectivity.hasUsableNetwork(this)) {
            persistGitHubOperationState("pull_request", "waiting_network",
                    "GitHub pull request: waiting for a usable network");
            refreshGitHubSyncStatus();
            return;
        }
        final Map<String, String> changes;
        try {
            changes = changedProjectSources(loadProjectBaselineSnapshot(), loadBundledProject());
        } catch (IOException error) {
            githubSyncStatus.setText("GitHub pull request error: unable to read local changes");
            return;
        }
        if (changes.isEmpty()) {
            githubSyncStatus.setText("GitHub pull request: no local changes");
            return;
        }
        if (reviewedGitHubChangeFingerprint.isEmpty()) {
            reviewedGitHubChangeFingerprint = prefs.getString(githubProjectPreferenceKey(GITHUB_PREF_REVIEW_FINGERPRINT), "");
        }
        if (!githubChangeFingerprint(changes).equals(reviewedGitHubChangeFingerprint)) {
            githubSyncStatus.setText("GitHub pull request: review current changes first");
            return;
        }
        final String reviewBranch = githubReviewBranchName();
        if (!beginGitHubOperation("pull_request", "GitHub pull request: queued")) return;
        githubSyncExecutor.submit(new Runnable() {
            @Override public void run() {
                try {
                    WorkshopGitHubApi api = new WorkshopGitHubApi(token, repository);
                    api.ensureReviewBranch(baseBranch, reviewBranch);
                    int completed = 0;
                    for (Map.Entry<String, String> entry : changes.entrySet()) {
                        completed += 1;
                        postGitHubOperationState("pull_request", "running", "GitHub pull request: uploading " + completed + "/" + changes.size());
                        api.applyFileChange(reviewBranch, entry.getKey(),
                                entry.getValue() == null ? null
                                        : entry.getValue().getBytes(StandardCharsets.UTF_8), null);
                    }
                    String url = api.createOrFindPullRequest(
                            baseBranch, reviewBranch, formatGitHubPullRequestBody(changes));
                    postGitHubOperationState("", "complete", "GitHub pull request: ready " + url);
                } catch (Exception error) {
                    postGitHubOperationFailure("pull_request", "GitHub pull request", error);
                }
            }
        });
    }

    private static String githubChangeFingerprint(Map<String, String> changes) {
        return WorkshopGitHubSyncPolicy.reviewFingerprint(changes);
    }

    private String githubReviewBranchName() {
        String identity = activeProject == null
                ? Integer.toHexString(projectRootPath().hashCode()) : activeProject.id;
        return "stasis-workshop-" + identity;
    }

    private static String formatGitHubPullRequestBody(Map<String, String> changes) {
        StringBuilder body = new StringBuilder("Updated from Stasis Workshop for Android.\n\nChanged files:");
        for (Map.Entry<String, String> change : changes.entrySet()) {
            body.append("\n- `").append(change.getKey()).append('`');
            if (change.getValue() == null) body.append(" (deleted)");
        }
        return body.toString();
    }

    private void retryGitHubOperation() {
        String operation = getSharedPreferences(GITHUB_PREFS, MODE_PRIVATE)
                .getString(githubProjectPreferenceKey(GITHUB_PREF_OPERATION), "");
        if ("sync".equals(operation)) {
            queueGitHubSync();
        } else if ("pull_request".equals(operation)) {
            queueGitHubPullRequest();
        } else {
            githubSyncStatus.setText("GitHub sync: no retryable operation");
        }
    }

    private void resumeGitHubAfterNetworkChange() {
        if (isGitHubOperationActive() || !WorkshopConnectivity.hasUsableNetwork(this)) return;
        SharedPreferences preferences = getSharedPreferences(GITHUB_PREFS, MODE_PRIVATE);
        String state = preferences.getString(
                githubProjectPreferenceKey(GITHUB_PREF_OPERATION_STATE), "");
        String operation = preferences.getString(
                githubProjectPreferenceKey(GITHUB_PREF_OPERATION), "");
        boolean automatic = preferences.getBoolean(
                githubProjectPreferenceKey(GITHUB_PREF_OPERATION_AUTOMATIC), false);
        WorkshopGitHubSyncPolicy.NetworkResumeDecision decision =
                WorkshopGitHubSyncPolicy.networkResume(
                        operation, state, WorkshopConnectivity.hasUsableNetwork(this), automatic);
        if (decision == WorkshopGitHubSyncPolicy.NetworkResumeDecision.RECHECK_AUTOMATIC_SYNC) {
            scheduleGitHubAutoSync();
        } else if (decision == WorkshopGitHubSyncPolicy.NetworkResumeDecision.RETRY_USER_SYNC) {
            queueGitHubSync(true);
        } else if (decision == WorkshopGitHubSyncPolicy.NetworkResumeDecision.RETRY_PULL_REQUEST) {
            queueGitHubPullRequest();
        }
    }

    private boolean batterySaverEnabled() {
        PowerManager manager = (PowerManager)getSystemService(POWER_SERVICE);
        return manager != null && manager.isPowerSaveMode();
    }

    private boolean deviceCharging() {
        Intent battery = registerReceiver(null, new IntentFilter(Intent.ACTION_BATTERY_CHANGED));
        if (battery == null) return false;
        int status = battery.getIntExtra(BatteryManager.EXTRA_STATUS, -1);
        return status == BatteryManager.BATTERY_STATUS_CHARGING
                || status == BatteryManager.BATTERY_STATUS_FULL;
    }

    private void persistGitHubOperationState(String operation, String state, String detail) {
        getSharedPreferences(GITHUB_PREFS, MODE_PRIVATE).edit()
                .putString(githubProjectPreferenceKey(GITHUB_PREF_OPERATION), operation)
                .putString(githubProjectPreferenceKey(GITHUB_PREF_OPERATION_STATE), state)
                .putString(githubProjectPreferenceKey(GITHUB_PREF_OPERATION_DETAIL), detail)
                .apply();
    }

    private void persistGitHubSyncOperationState(String state, String detail, boolean automatic) {
        getSharedPreferences(GITHUB_PREFS, MODE_PRIVATE).edit()
                .putString(githubProjectPreferenceKey(GITHUB_PREF_OPERATION), "sync")
                .putString(githubProjectPreferenceKey(GITHUB_PREF_OPERATION_STATE), state)
                .putString(githubProjectPreferenceKey(GITHUB_PREF_OPERATION_DETAIL), detail)
                .putBoolean(githubProjectPreferenceKey(GITHUB_PREF_OPERATION_AUTOMATIC), automatic)
                .apply();
    }

    private String githubProjectPreferenceKey(String base) {
        String identity = activeProject == null
                ? Integer.toHexString(projectRootPath().hashCode()) : activeProject.id;
        return base + "_" + identity;
    }

    private String readGitHubProjectPreference(SharedPreferences preferences, String base, String fallback) {
        String scopedKey = githubProjectPreferenceKey(base);
        if (preferences.contains(scopedKey)) return preferences.getString(scopedKey, fallback);
        if (!preferences.contains(base)) return fallback;
        String legacy = preferences.getString(base, fallback);
        preferences.edit().putString(scopedKey, legacy).remove(base).apply();
        return legacy;
    }

    private boolean githubTargetValidated(SharedPreferences preferences,
            String repository, String branch) {
        if (repository.isEmpty() || branch.isEmpty()) return false;
        return WorkshopGitHubSyncPolicy.targetIdentity(repository, branch).equals(
                preferences.getString(
                        githubProjectPreferenceKey(GITHUB_PREF_VALIDATED_TARGET), ""));
    }

    private synchronized boolean beginProjectIoWork(String detail) {
        if (projectIoActive || !WorkshopLongWorkCoordinator.beginProjectIo(this, detail)) {
            setStatusText("Project operation blocked while another foreground operation is active");
            return false;
        }
        projectIoActive = true;
        return true;
    }

    private void finishProjectIoWork() {
        projectIoActive = false;
        WorkshopLongWorkCoordinator.finishProjectIo(this);
        if (activityDestroyed) projectIoExecutor.shutdown();
        requestGitHubAutoSync();
    }

    private boolean isGitHubOperationActive() {
        return WorkshopLongWorkCoordinator.isGitHubActive();
    }

    private synchronized boolean beginGitHubOperation(String operation, String status) {
        return beginGitHubOperation(operation, status, null);
    }

    private synchronized boolean beginGitHubOperation(
            String operation, String status, Boolean automaticSync) {
        if (WorkshopLongWorkCoordinator.isGitHubActive()) {
            githubSyncStatus.setText("GitHub sync: another operation is already queued or running");
            return false;
        }
        String detail = "validate".equals(operation) ? "Validating the GitHub backup target"
                : ("pull_request".equals(operation) ? "Publishing reviewed project files"
                        : "Backing up project files to GitHub");
        if (!WorkshopLongWorkCoordinator.beginGitHub(this, detail)) {
            githubSyncStatus.setText("GitHub sync: another foreground operation is active");
            return false;
        }
        if (automaticSync == null) {
            postGitHubOperationState(operation, "queued", status);
        } else {
            persistGitHubSyncOperationState("queued", status, automaticSync.booleanValue());
            if (githubSyncStatus != null) githubSyncStatus.setText(status);
        }
        return true;
    }

    private void postGitHubOperationState(final String operation, final String state, final String status) {
        persistGitHubOperationState(operation, state, status);
        if ("complete".equals(state) || "error".equals(state)
                || "waiting_network".equals(state) || "deferred".equals(state)) {
            WorkshopLongWorkCoordinator.finishGitHub(this);
            if (activityDestroyed) githubSyncExecutor.shutdown();
        }
        runOnUiThread(new Runnable() {
            @Override public void run() {
                if (githubSyncStatus != null) githubSyncStatus.setText(status);
                if ("complete".equals(state)) requestGitHubAutoSync();
            }
        });
    }

    private void registerNetworkMonitoring() {
        connectivityManager = (ConnectivityManager)getSystemService(CONNECTIVITY_SERVICE);
        if (connectivityManager == null || networkCallbackRegistered) return;
        networkCallback = new ConnectivityManager.NetworkCallback() {
            @Override public void onAvailable(Network network) { resumeBackgroundWorkAfterNetworkChange(); }
            @Override public void onCapabilitiesChanged(Network network, NetworkCapabilities capabilities) {
                resumeBackgroundWorkAfterNetworkChange();
            }
        };
        try {
            connectivityManager.registerDefaultNetworkCallback(networkCallback);
            networkCallbackRegistered = true;
        } catch (RuntimeException error) {
            networkCallback = null;
        }
    }

    private void postGitHubOperationFailure(String operation, String label, Exception error) {
        boolean networkAvailable = WorkshopConnectivity.hasUsableNetwork(this);
        String state = WorkshopGitHubSyncPolicy.failureState(networkAvailable);
        String status = networkAvailable
                ? label + " error: " + error.getMessage()
                : label + ": network lost; waiting to retry";
        postGitHubOperationState(operation, state, status);
    }

    private void resumeBackgroundWorkAfterNetworkChange() {
        if (!WorkshopConnectivity.hasUsableNetwork(this)) return;
        runOnUiThread(new Runnable() {
            @Override public void run() {
                resumeGitHubAfterNetworkChange();
                requestGitHubAutoSync();
            }
        });
    }

    private void unregisterNetworkMonitoring() {
        if (!networkCallbackRegistered || connectivityManager == null || networkCallback == null) return;
        try {
            connectivityManager.unregisterNetworkCallback(networkCallback);
        } catch (RuntimeException ignored) {
            // Android may already have removed the callback during process teardown.
        }
        networkCallbackRegistered = false;
        networkCallback = null;
    }

    private void registerPowerMonitoring() {
        if (powerReceiverRegistered) return;
        powerReceiver = new BroadcastReceiver() {
            @Override public void onReceive(Context context, Intent intent) {
                requestGitHubAutoSync();
            }
        };
        IntentFilter filter = new IntentFilter();
        filter.addAction(Intent.ACTION_POWER_CONNECTED);
        filter.addAction(Intent.ACTION_POWER_DISCONNECTED);
        filter.addAction(PowerManager.ACTION_POWER_SAVE_MODE_CHANGED);
        try {
            registerReceiver(powerReceiver, filter);
            powerReceiverRegistered = true;
        } catch (RuntimeException error) {
            powerReceiver = null;
        }
    }

    private void unregisterPowerMonitoring() {
        if (!powerReceiverRegistered || powerReceiver == null) return;
        try {
            unregisterReceiver(powerReceiver);
        } catch (RuntimeException ignored) {
            // Android may already have removed the receiver during process teardown.
        }
        powerReceiverRegistered = false;
        powerReceiver = null;
    }

    private LinearLayout createEditControls() {
        LinearLayout controls = new LinearLayout(this);
        controls.setOrientation(LinearLayout.VERTICAL);
        controls.setPadding(0, dp(8), 0, 0);

        LinearLayout editRow = new LinearLayout(this);
        editRow.setOrientation(LinearLayout.HORIZONTAL);

        Button apply = new Button(this);
        apply.setText("Apply");
        apply.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                applySelectedEdit();
            }
        });
        editRow.addView(apply, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1.0f));

        Button reset = new Button(this);
        reset.setText("Reset");
        reset.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                resetSelectedEdit();
            }
        });
        editRow.addView(reset, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1.0f));
        controls.addView(editRow, fullWidth());

        Button revertSaved = new Button(this);
        revertSaved.setText("Revert Saved");
        revertSaved.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                revertSelectedToBundled();
            }
        });
        controls.addView(revertSaved, fullWidth());

        Button refreshChanges = new Button(this);
        refreshChanges.setText("Changes");
        refreshChanges.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                ProjectSnapshot current = loadBundledProject();
                boolean reviewedChange = refreshChangeSummary(current);
                if (diagnosticBody != null) diagnosticBody.setVisibility(View.VISIBLE);
                if (reviewedChange) {
                    recordOnboardingTrackedChangeStep(
                            WorkshopOnboardingPolicy.Step.CHANGES_REVIEWED, current);
                }
            }
        });
        controls.addView(refreshChanges, fullWidth());

        Button rawDiffs = new Button(this);
        rawDiffs.setText("Raw Diffs");
        rawDiffs.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                showRawDiffReview();
            }
        });
        controls.addView(rawDiffs, fullWidth());

        Button newTest = new Button(this);
        newTest.setText("New Test");
        newTest.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                createManualTest();
            }
        });
        controls.addView(newTest, fullWidth());

        Button deleteTest = new Button(this);
        deleteTest.setText("Delete Test");
        deleteTest.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                deleteSelectedManualTest();
            }
        });
        controls.addView(deleteTest, fullWidth());

        Button newHelper = new Button(this);
        newHelper.setText("New Helper");
        newHelper.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                createManualHelper();
            }
        });
        controls.addView(newHelper, fullWidth());

        Button deleteHelper = new Button(this);
        deleteHelper.setText("Delete Helper");
        deleteHelper.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                deleteSelectedManualHelper();
            }
        });
        controls.addView(deleteHelper, fullWidth());

        Button resetProject = new Button(this);
        resetProject.setText("Reset Project");
        resetProject.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                resetProjectFiles();
            }
        });
        controls.addView(resetProject, fullWidth());

        Button runTests = new Button(this);
        runTests.setText("Run Tests");
        runTests.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                runNativeTests();
            }
        });
        controls.addView(runTests, fullWidth());

        Button runTick = new Button(this);
        runTick.setText("Run Tick");
        runTick.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                runNativeTick();
            }
        });
        controls.addView(runTick, fullWidth());
        return controls;
    }
    private void runNativeCompile() {
        String compileResult = nativeCompileProject(projectRootPath());
        lastCompileResult = compileResult;
        compileReady = isRunnableCompile(compileResult);
        compileAttempted = true;
        setStatusText(compileResult);
    }

    private void runNativeTick() {
        int touchX = gamePreview == null ? 0 : gamePreview.touchX();
        int touchY = gamePreview == null ? 0 : gamePreview.touchY();
        int touchActive = gamePreview == null ? 0 : gamePreview.touchActive();
        int screenWidth = gamePreview == null ? 0 : gamePreview.getWidth();
        int screenHeight = gamePreview == null ? 0 : gamePreview.getHeight();
        int frameStatus = gamePreview == null ? -1 : gamePreview.runNativeFrame(
                projectRootPath(), touchX, touchY, touchActive, screenWidth, screenHeight,
                nativeFrameValues);
        long tickEndNanos = System.nanoTime();
        tickMetric.add(tickEndNanos,
                gamePreview == null ? 0L : gamePreview.lastNativeFrameDurationNanos());
        syncMetric.add(tickEndNanos,
                gamePreview == null ? 0L : gamePreview.lastRendererSyncWaitNanos());
        if (frameStatus != 0 || nativeFrameValues[0] != StasisPreviewRenderer.RENDER_MAGIC
                || nativeFrameValues[1] != StasisPreviewRenderer.RENDER_VERSION) {
            compileReady = false;
            compileAttempted = true;
            gameRuntimeActive = false;
            String frameError = "RunError: " + nativeLastFrameError();
            setStatusText(frameError);
            if (gameStatus != null) gameStatus.setText(frameError);
            android.util.Log.e("StasisWorkshop", frameError);
            return;
        }
        gameRuntimeActive = true;
        recordOnboardingProjectStep(WorkshopOnboardingPolicy.Step.PROJECT_RAN);
        updateGameDebugText();
    }

    // Acceptance-only bridge for the fixed IT-027 gesture. The frame still
    // enters through GamePreviewView and the normal JNI direct-buffer path;
    // this method only packages evidence after GLES has consumed its token.
    String runIt027Frame(String projectRoot, int logicalX, int logicalY, int action,
            int touchActive, int sequence) {
        if (gamePreview == null) {
            return "{\"status\":\"failed\",\"error\":\"preview unavailable\"}";
        }
        int screenWidth = Math.max(1, gamePreview.getWidth());
        int screenHeight = Math.max(1, gamePreview.getHeight());
        float scale = Math.min(screenWidth / 640.0f, screenHeight / 360.0f);
        int viewportWidth = Math.max(1, Math.round(640.0f * scale));
        int viewportHeight = Math.max(1, Math.round(360.0f * scale));
        float viewportX = (screenWidth - viewportWidth) / 2.0f;
        float viewportY = (screenHeight - viewportHeight) / 2.0f;
        float nativeX = viewportX + logicalX * scale;
        float nativeY = viewportY + logicalY * scale;
        try {
            gamePreview.dispatchAcceptanceTouch(action, nativeX, nativeY);
            int viewTouchActive = gamePreview.touchActive();
            if (viewTouchActive != touchActive) {
                return "{\"status\":\"failed\",\"error\":\"view touch state mismatch\"}";
            }
            GamePreviewView.AcceptanceFrameSubmission submission =
                    gamePreview.runNativeAcceptanceFrame(projectRoot, gamePreview.touchX(),
                    gamePreview.touchY(), viewTouchActive, screenWidth, screenHeight,
                    nativeFrameValues);
            int status = submission.status();
            if (status != 0) {
                return "{\"status\":\"failed\",\"error\":"
                        + JSONObject.quote(nativeLastFrameError()) + "}";
            }
            int token = submission.frameToken();
            long trace = Integer.toUnsignedLong(submission.trace());
            boolean presented = gamePreview.awaitPresentedFrame(submission, 5_000L);
            if (!presented) {
                return "{\"status\":\"failed\",\"error\":\"GLES token timeout\"}";
            }
            JSONObject guest = new JSONObject();
            String[] names = {"x", "y", "dx", "dy", "x_norm_x1000", "y_norm_x1000",
                    "active", "down_edge", "up_edge", "marker_active", "checksum"};
            String[] paths = {"seam_touch_x", "seam_touch_y", "seam_touch_dx", "seam_touch_dy",
                    "seam_touch_x_norm_x1000", "seam_touch_y_norm_x1000", "seam_touch_active",
                    "seam_touch_down_edge", "seam_touch_up_edge", "seam_touch_marker_active",
                    "seam_touch_checksum"};
            for (int index = 0; index < names.length; index += 1) {
                String state = nativeGetRuntimeI32(projectRoot, paths[index]);
                guest.put(names[index], extractIntField(state, "value", Integer.MIN_VALUE));
            }
            JSONObject marker = new JSONObject();
            int rectCount = gamePreview.rectCount();
            boolean markerActive = rectCount >= 2;
            marker.put("active", markerActive);
            if (markerActive) {
                int base = StasisPreviewRenderer.F_RECT_REVERSE_BASE - 8;
                marker.put("x", gamePreview.acceptanceFrameF32(base));
                marker.put("y", gamePreview.acceptanceFrameF32(base + 1));
                marker.put("w", gamePreview.acceptanceFrameF32(base + 2));
                marker.put("h", gamePreview.acceptanceFrameF32(base + 3));
                marker.put("r", gamePreview.acceptanceFrameF32(base + 4));
                marker.put("g", gamePreview.acceptanceFrameF32(base + 5));
                marker.put("b", gamePreview.acceptanceFrameF32(base + 6));
                marker.put("a", gamePreview.acceptanceFrameF32(base + 7));
            }
            return new JSONObject().put("schema", "stasis.workshop_touch_roundtrip.v1")
                    .put("test_id", "IT-027").put("event", "case")
                    .put("status", "passed").put("sequence", sequence)
                    .put("phase", action == MotionEvent.ACTION_DOWN ? "down"
                            : action == MotionEvent.ACTION_MOVE ? "move" : "up")
                    .put("input", new JSONObject().put("x", logicalX).put("y", logicalY)
                            .put("active", touchActive).put("action", action))
                    .put("guest", guest)
                    .put("render", new JSONObject().put("frame_token", token)
                            .put("trace", trace)
                            .put("rect_count", rectCount)
                            .put("marker", marker))
                    .put("gles_presented", true).put("gles_frame_token", token)
                    .put("java_only", false).toString();
        } catch (Exception error) {
            return "{\"status\":\"failed\",\"error\":"
                    + JSONObject.quote(error.getMessage() == null ? error.getClass().getSimpleName()
                            : error.getMessage()) + "}";
        }
    }

    String runIt028Frame(String projectRoot, String phase, int sequence) {
        if (gamePreview == null) {
            return "{\"status\":\"failed\",\"error\":\"preview unavailable\"}";
        }
        int screenWidth = Math.max(1, gamePreview.getWidth());
        int screenHeight = Math.max(1, gamePreview.getHeight());
        try {
            GamePreviewView.AcceptanceFrameSubmission submission =
                    gamePreview.runNativeAcceptanceFrame(projectRoot, 0, 0, 0,
                    screenWidth, screenHeight, nativeFrameValues);
            int status = submission.status();
            if (status != 0) {
                return "{\"status\":\"failed\",\"error\":"
                        + JSONObject.quote(nativeLastFrameError()) + "}";
            }
            int token = submission.frameToken();
            long trace = Integer.toUnsignedLong(submission.trace());
            if (!gamePreview.awaitPresentedFrame(submission, 5_000L)) {
                return "{\"status\":\"failed\",\"error\":\"GLES token timeout\"}";
            }
            JSONObject runtime = new JSONObject(nativeInspectRuntimeState(projectRoot));
            JSONObject guest = new JSONObject();
            String[] names = {"tick_revision", "render_revision", "state_counter"};
            String[] paths = {"seam_it028_tick_marker", "seam_it028_render_marker",
                    "seam_it028_state_counter"};
            for (int index = 0; index < names.length; index += 1) {
                String state = nativeGetRuntimeI32(projectRoot, paths[index]);
                guest.put(names[index], extractIntField(state, "value", Integer.MIN_VALUE));
            }
            JSONObject marker = new JSONObject();
            int rectCount = gamePreview.rectCount();
            boolean markerActive = rectCount >= 2;
            marker.put("active", markerActive);
            if (markerActive) {
                int base = StasisPreviewRenderer.F_RECT_REVERSE_BASE - 8;
                marker.put("x", gamePreview.acceptanceFrameF32(base));
                marker.put("y", gamePreview.acceptanceFrameF32(base + 1));
                marker.put("w", gamePreview.acceptanceFrameF32(base + 2));
                marker.put("h", gamePreview.acceptanceFrameF32(base + 3));
                marker.put("r", gamePreview.acceptanceFrameF32(base + 4));
                marker.put("g", gamePreview.acceptanceFrameF32(base + 5));
                marker.put("b", gamePreview.acceptanceFrameF32(base + 6));
                marker.put("a", gamePreview.acceptanceFrameF32(base + 7));
            }
            return new JSONObject().put("schema", "stasis.workshop_hot_edit.v1")
                    .put("test_id", "IT-028").put("event", "case")
                    .put("status", "passed").put("phase", phase).put("sequence", sequence)
                    .put("runtime", runtime).put("guest", guest)
                    .put("render", new JSONObject().put("frame_token", token)
                            .put("trace", trace).put("rect_count", rectCount)
                            .put("marker", marker))
                    .put("gles_presented", true).put("gles_frame_token", token)
                    .put("java_only", false).put("fallback", 0).put("stub", 0).toString();
        } catch (Exception error) {
            return "{\"status\":\"failed\",\"error\":"
                    + JSONObject.quote(error.getMessage() == null ? error.getClass().getSimpleName()
                            : error.getMessage()) + "}";
        }
    }

    JSONObject runIt029Frame(String projectRoot, String phase, int sequence) throws Exception {
        if (!BuildConfig.STASIS_RENDER_ACCEPTANCE || gamePreview == null) {
            throw new IllegalStateException("IT-029 preview unavailable");
        }
        GamePreviewView.AcceptanceFrameSubmission submission =
                gamePreview.runNativeAcceptanceFrame(projectRoot, 0, 0, 0,
                Math.max(1, gamePreview.getWidth()), Math.max(1, gamePreview.getHeight()),
                nativeFrameValues);
        int status = submission.status();
        if (status != 0) throw new IllegalStateException(nativeLastFrameError());
        int token = submission.frameToken();
        long commandTrace = Integer.toUnsignedLong(submission.trace());
        if (!gamePreview.awaitPresentedFrame(submission, 5_000L)) {
            throw new IllegalStateException("IT-029 GLES token timeout");
        }
        final Bitmap[] captured = new Bitmap[1];
        final String[] captureError = new String[1];
        final StasisPreviewRenderer.LogicalFrameSnapshot[] logical =
                new StasisPreviewRenderer.LogicalFrameSnapshot[1];
        CountDownLatch captureReady = new CountDownLatch(1);
        gamePreview.captureFrame((bitmap, error, frame) -> {
            captured[0] = bitmap;
            captureError[0] = error;
            logical[0] = frame;
            captureReady.countDown();
        });
        long captureDeadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5L);
        while (captureReady.getCount() != 0L && System.nanoTime() < captureDeadline) {
            gamePreview.requestRender();
            long remaining = captureDeadline - System.nanoTime();
            if (remaining > 0L) {
                captureReady.await(Math.min(remaining, TimeUnit.MILLISECONDS.toNanos(100L)),
                        TimeUnit.NANOSECONDS);
            }
        }
        if (captureReady.getCount() != 0L || captured[0] == null) {
            throw new IllegalStateException("IT-029 capture failed: "
                    + (captureError[0] == null ? "timeout" : captureError[0]));
        }
        byte[] png;
        try {
            png = encodeBitmapPng(captured[0]);
        } finally {
            captured[0].recycle();
        }
        File captureDirectory = new File(getExternalFilesDir(null), "it029");
        if (!captureDirectory.isDirectory() && !captureDirectory.mkdirs()) {
            throw new IOException("IT-029 capture directory could not be created");
        }
        File capture = new File(captureDirectory, phase + ".png");
        FileOutputStream output = new FileOutputStream(capture);
        try {
            output.write(png);
            output.getFD().sync();
        } finally {
            output.close();
        }
        StasisPreviewRenderer.LogicalFrameSnapshot frame = logical[0];
        JSONArray sprites = new JSONArray();
        for (int index = 0; index + 2 < frame.sprites.length; index += 3) {
            sprites.put(frame.sprites[index]);
        }
        JSONArray fonts = new JSONArray();
        JSONArray cachedText = new JSONArray();
        for (int index = 0; index + 2 < frame.textMetadata.length; index += 3) {
            fonts.put(frame.textMetadata[index]);
            if (frame.textMetadata[index + 1] < 0) {
                cachedText.put(0 - frame.textMetadata[index + 1]);
            }
        }
        JSONObject resources = gamePreview.resourceScopeSnapshot();
        return new JSONObject()
                .put("schema", "stasis.workshop_resource_scope.v1")
                .put("test_id", "IT-029").put("event", "case")
                .put("status", "passed").put("phase", phase).put("sequence", sequence)
                .put("project_root", new File(projectRoot).getCanonicalPath())
                .put("frame_token", token).put("gles_presented", true)
                .put("command_trace", commandTrace)
                .put("sprite_handles", sprites).put("font_handles", fonts)
                .put("cached_text_handles", cachedText)
                .put("direct_text_sha256", sha256Bytes(frame.textBytes))
                .put("capture_path", capture.getAbsolutePath())
                .put("capture_sha256", sha256Bytes(png))
                .put("resources", resources)
                .put("java_only", false).put("fallback", 0).put("stub", 0);
    }

    void resetIt029ResourceMetrics() {
        if (gamePreview != null) gamePreview.resetResourceScopeMetrics();
    }

    boolean recreateIt029Surface() {
        return gamePreview != null && gamePreview.recreateEglContextForAcceptance(5_000L);
    }

    String acceptanceReadSource(String projectRoot) throws IOException {
        return readTextFile(new File(projectRoot, "src/main.stasis"));
    }

    void acceptanceReplaceSource(String projectRoot, String source) throws IOException {
        replaceTextFileAtomically(new File(projectRoot, "src/main.stasis"), source);
    }

    String acceptanceCompile(String projectRoot) {
        return nativeCompileProject(projectRoot);
    }

    JSONObject acceptanceRuntimeState(String projectRoot) throws Exception {
        return new JSONObject(nativeInspectRuntimeState(projectRoot));
    }

    JSONObject acceptanceActivateRuntime(String projectRoot) throws Exception {
        if (gamePreview == null) {
            throw new IllegalStateException("IT-030 runtime activation requires the preview");
        }
        GamePreviewView.AcceptanceFrameSubmission submission =
                gamePreview.runNativeAcceptanceFrame(projectRoot, gamePreview.touchX(),
                gamePreview.touchY(), gamePreview.touchActive(),
                Math.max(1, gamePreview.getWidth()), Math.max(1, gamePreview.getHeight()),
                nativeFrameValues);
        int status = submission.status();
        if (status != 0) {
            throw new IllegalStateException("IT-030 runtime activation failed: "
                    + nativeLastFrameError());
        }
        if (!gamePreview.awaitPresentedFrame(submission, 5_000L)) {
            throw new IllegalStateException("IT-030 GLES token timeout");
        }
        JSONObject runtime = acceptanceRuntimeState(projectRoot);
        if (runtime.optBoolean("pending_candidate", true)) {
            throw new IllegalStateException("IT-030 runtime activation left a pending candidate");
        }
        runtime.put("activation", "native_frame");
        return runtime;
    }

    String acceptanceRunTests(String projectRoot) {
        return nativeRunTests(projectRoot);
    }

    void materializeIt029Project(WorkshopProjectRegistry.ProjectInfo project) throws IOException {
        if (!BuildConfig.STASIS_RENDER_ACCEPTANCE) {
            throw new IllegalStateException("IT-029 project materialization is acceptance-only");
        }
        WorkshopTemplateCatalog.Template template = WorkshopTemplateCatalog.require(
                project.templateId);
        materializeTemplateProject(getAssets(), template, project.root, false);
    }

    WorkshopProjectSnapshot.Snapshot acceptanceCaptureProject(String projectRoot)
            throws Exception {
        return WorkshopProjectSnapshot.capture(new File(projectRoot));
    }

    void acceptanceRestoreProject(String projectRoot,
            WorkshopProjectSnapshot.Snapshot snapshot) throws Exception {
        WorkshopProjectSnapshot.restore(new File(projectRoot), snapshot);
    }

    String acceptanceProjectFingerprint(WorkshopProjectSnapshot.Snapshot snapshot)
            throws Exception {
        return WorkshopProjectSnapshot.fingerprint(snapshot);
    }

    void acceptanceWriteTest(String projectRoot, String relativePath, String source)
            throws Exception {
        String normalized = relativePath.replace('\\', '/');
        if (!normalized.startsWith("tests/") || !normalized.endsWith(".test.stasis")
                || normalized.contains("..")) {
            throw new IllegalArgumentException("IT-030 test path is invalid");
        }
        File root = new File(projectRoot).getCanonicalFile();
        File target = new File(root, normalized.replace('/', File.separatorChar)).getCanonicalFile();
        if (!target.getPath().startsWith(root.getPath() + File.separator)) {
            throw new IllegalArgumentException("IT-030 test path escaped project");
        }
        replaceTextFileAtomically(target, source);
    }

    String acceptanceSetRuntimeI32(String projectRoot, String path, int value) {
        return nativeSetRuntimeI32(projectRoot, path, value);
    }

    int acceptanceRuntimeI32(String projectRoot, String path) {
        return extractIntField(nativeGetRuntimeI32(projectRoot, path),
                "value", Integer.MIN_VALUE);
    }

    JSONObject acceptanceCompileDiagnostic(String compileResult) throws Exception {
        return compileResultToJson(compileResult);
    }

    WorkshopNativeDiagnostic acceptanceNativeDiagnostic(String nativeMessage) {
        return WorkshopNativeDiagnostic.fromNative(nativeMessage);
    }

    JSONObject acceptanceDisplayDiagnostic(String nativeMessage) throws Exception {
        setStatusText(nativeMessage);
        String displayedText = reloadStatus == null
                ? compactStatusText(nativeMessage)
                : reloadStatus.getText().toString();
        WorkshopNativeDiagnostic displayed = WorkshopNativeDiagnostic.fromNative(displayedText);
        return new JSONObject().put("diagnostic", displayed == null ? JSONObject.NULL
                : displayed.toJson()).put("displayed_text", displayedText);
    }

    String runIt031Frame(String projectRoot) {
        if (gamePreview == null) return "";
        GamePreviewView.AcceptanceFrameSubmission submission =
                gamePreview.runNativeAcceptanceFrame(projectRoot, gamePreview.touchX(),
                gamePreview.touchY(), gamePreview.touchActive(),
                Math.max(1, gamePreview.getWidth()), Math.max(1, gamePreview.getHeight()),
                nativeFrameValues);
        int status = submission.status();
        if (status != 0) return "RunError: " + nativeLastFrameError();
        if (!gamePreview.awaitPresentedFrame(submission, 5_000L)) {
            return "RunError: GLES token timeout";
        }
        return "passed";
    }

    JSONObject runIt032Frame(String projectRoot, int sequence) throws Exception {
        if (!BuildConfig.STASIS_RENDER_ACCEPTANCE || gamePreview == null) {
            throw new IllegalStateException("IT-032 preview unavailable");
        }
        GamePreviewView.AcceptanceFrameSubmission submission =
                gamePreview.runNativeAcceptanceFrame(projectRoot, 0, 0, 0,
                Math.max(1, gamePreview.getWidth()), Math.max(1, gamePreview.getHeight()),
                nativeFrameValues);
        int status = submission.status();
        if (status != 0) throw new IllegalStateException(nativeLastFrameError());
        int token = submission.frameToken();
        long trace = Integer.toUnsignedLong(submission.trace());
        if (!gamePreview.awaitPresentedFrame(submission, 5_000L)) {
            throw new IllegalStateException("IT-032 GLES token timeout at frame " + sequence);
        }
        JSONObject guest = new JSONObject();
        String[] names = {"tick_revision", "render_revision", "state_counter"};
        String[] paths = {"seam_it028_tick_marker", "seam_it028_render_marker",
                "seam_it028_state_counter"};
        for (int index = 0; index < names.length; index += 1) {
            String state = nativeGetRuntimeI32(projectRoot, paths[index]);
            guest.put(names[index], extractIntField(state, "value", Integer.MIN_VALUE));
        }
        return new JSONObject().put("sequence", sequence).put("frame_token", token)
                .put("command_trace", trace).put("runtime", acceptanceRuntimeState(projectRoot))
                .put("guest", guest).put("buffers", gamePreview.acceptanceBufferSnapshot())
                .put("resources", gamePreview.resourceScopeSnapshot())
                .put("presentation", gamePreview.workshopSoakPresentationSnapshot())
                .put("gles_presented", true).put("java_only", false)
                .put("fallback", 0).put("stub", 0);
    }

    boolean recreateIt032Surface() {
        return gamePreview != null && gamePreview.recreateEglContextForAcceptance(5_000L);
    }

    void setIt032Active(boolean active) {
        if (gamePreview != null) gamePreview.setWorkshopSoakAcceptanceActive(active);
    }

    JSONObject acceptanceRecoverAfterHealthyFrame(String compileResult) throws Exception {
        if (!BuildConfig.STASIS_RENDER_ACCEPTANCE || !isRunnableCompile(compileResult)
                || gamePreview == null
                || nativeFrameValues[0] != StasisPreviewRenderer.RENDER_MAGIC
                || nativeFrameValues[1] != StasisPreviewRenderer.RENDER_VERSION) {
            throw new IllegalStateException("IT-031 UI recovery requires a healthy compile and frame");
        }
        compileReady = true;
        compileAttempted = true;
        gameRuntimeActive = true;
        lastCompileResult = compileResult;
        setStatusText(compileResult);
        String displayedStatus = reloadStatus == null
                ? compactStatusText(compileResult) : reloadStatus.getText().toString();
        boolean blockingVisible = blockingErrorPanel != null
                && blockingErrorPanel.getVisibility() == View.VISIBLE;
        boolean statusHealthy = !blockingVisible && !displayedStatus.startsWith("CompileError")
                && !displayedStatus.startsWith("RunError");
        return new JSONObject().put("blocking_error_visible", blockingVisible)
                .put("status_healthy", statusHealthy)
                .put("displayed_status", displayedStatus)
                .put("compile_ready", compileReady)
                .put("compile_attempted", compileAttempted)
                .put("game_runtime_active", gameRuntimeActive);
    }

    private static int extractIntField(String text, String key, int fallback) {
        String marker = key + "=";
        int start = text.indexOf(marker);
        if (start < 0) {
            return fallback;
        }
        start += marker.length();
        int end = start;
        if (end < text.length() && text.charAt(end) == '-') {
            end += 1;
        }
        while (end < text.length() && Character.isDigit(text.charAt(end))) {
            end += 1;
        }
        if (end == start || (end == start + 1 && text.charAt(start) == '-')) {
            return fallback;
        }
        return Integer.parseInt(text.substring(start, end));
    }

    private static String sha256Bytes(byte[] bytes) throws IOException {
        try {
            byte[] digest = MessageDigest.getInstance("SHA-256").digest(bytes);
            StringBuilder hex = new StringBuilder(digest.length * 2);
            String digits = "0123456789abcdef";
            for (byte value : digest) {
                int unsigned = value & 0xff;
                hex.append(digits.charAt(unsigned >>> 4)).append(digits.charAt(unsigned & 0x0f));
            }
            return hex.toString();
        } catch (NoSuchAlgorithmException error) {
            throw new IOException("SHA-256 is unavailable", error);
        }
    }

    private static byte[] encodeBitmapPng(Bitmap bitmap) throws IOException {
        ByteArrayOutputStream encoded = new ByteArrayOutputStream();
        if (!bitmap.compress(Bitmap.CompressFormat.PNG, 100, encoded)) {
            throw new IOException("could not encode captured preview pixels");
        }
        return encoded.toByteArray();
    }

    private static String testSummaryText(JSONObject testRun) {
        if (testRun == null) {
            return "tests unavailable";
        }
        if ("error".equals(testRun.optString("status", ""))) {
            return "tests error=" + testRun.optString("error", "unknown");
        }
        return "tests passed=" + testRun.optInt("passed", 0)
                + " failed=" + testRun.optInt("failed", 0)
                + " pending=" + testRun.optInt("pending", 0);
    }
    private void appendProjectFunction(ProjectSnapshot project, String file, String newSource) throws Exception {
        SourceFile sourceFile = findProjectFile(project, file);
        String separator = sourceFile.source.endsWith("\n") ? "\n" : "\n\n";
        sourceFile.source = sourceFile.source + separator + newSource.trim() + "\n";
        writeTextFile(sourceFile.diskFile, sourceFile.source);
    }

    private void runNativeTests() {
        try {
            String compiled = nativeCompileProject(projectRootPath());
            lastCompileResult = compiled;
            compileReady = isRunnableCompile(compiled);
            compileAttempted = true;
            JSONObject run = new JSONObject(nativeRunTests(projectRootPath()));
            JSONObject result = new JSONObject()
                    .put("compile", compileResultToJson(compiled))
                    .put("passed", run.optInt("passed", 0))
                    .put("failed", run.optInt("failed", 0))
                    .put("pending", run.optInt("pending", 0))
                    .put("stasis_test_files", new JSONArray().put(run));
            result.put("all_runnable_tests_passed", compileReady
                    && result.optInt("passed", 0) > 0 && result.optInt("failed", 0) == 0);
            captureFirstTestFailureDiagnostic(result);
            if (result.optBoolean("all_runnable_tests_passed", false)) {
                recordExplorationLesson(WorkshopExplorationLessonPolicy.PASSED_TESTS);
                recordOnboardingTrackedChangeStep(
                        WorkshopOnboardingPolicy.Step.TESTS_PASSED, loadBundledProject());
            }
            setStatusText(testSummaryText(result));
        } catch (Exception error) {
            setStatusText("Tests failed: " + error.getMessage());
        }
    }

    private static JSONObject compileResultToJson(String compileResult) throws Exception {
        String result = compileResult == null || compileResult.isEmpty() ? "CompileNotRun" : compileResult;
        JSONObject json = new JSONObject()
                .put("ok", isRunnableCompile(result))
                .put("raw", result)
                .put("kind", result.startsWith("CompileError") ? "compile_error" : "compile_result");
        WorkshopSourceDiagnostic diagnostic = WorkshopSourceDiagnostic.fromCompileResult(result);
        if (diagnostic != null) {
            json.put("diagnostic", new JSONObject()
                    .put("file", diagnostic.file)
                    .put("line", diagnostic.line)
                    .put("column", diagnostic.column)
                    .put("end_line", diagnostic.endLine)
                    .put("end_column", diagnostic.endColumn)
                    .put("symbol", diagnostic.symbol)
                    .put("message", diagnostic.message));
        }
        return json;
    }


    private static SourceFile findProjectFile(ProjectSnapshot project, String file) throws Exception {
        for (SourceFile sourceFile : project.files) {
            if (sourceFile.path.equals(file)) {
                return sourceFile;
            }
        }
        throw new IOException("Project file not found: " + file);
    }

    private void applySelectedEdit() {
        if (selectedSymbol == null) {
            return;
        }

        SymbolEntry editedSymbol = selectedSymbol;
        String editedSource = sourceEditor.getText().toString().trim();
        boolean sourceChanged = !editedSource.equals(editedSymbol.source.trim());
        String beforeFileSource = editedSymbol.sourceFile.source;
        String reload = classifySelectedReload(editedSymbol, editedSource);
        try {
            persistSelectedEdit(editedSymbol, editedSource);
            clearPendingDraft();
            ProjectSnapshot refreshedProject = loadBundledProject();
            rebuildSymbolList(refreshedProject);
            SymbolEntry refreshedSymbol = findMatchingSymbol(refreshedProject, editedSymbol);
            if (refreshedSymbol != null) {
                showSymbol(refreshedSymbol);
            } else if (refreshedProject.firstSymbol != null) {
                showSymbol(refreshedProject.firstSymbol);
            }
            String compileResult = nativeCompileProject(projectRootPath());
            lastCompileResult = compileResult;
            compileReady = isRunnableCompile(compileResult);
            compileAttempted = true;
            if (compileReady) {
                diagnosticFile = "";
                diagnosticSymbol = "";
                diagnosticLine = 0;
                diagnosticColumn = 0;
                diagnosticEndLine = 0;
                diagnosticEndColumn = 0;
                diagnosticStatus.setText("Compile passed - " + reload);
                recordExplorationLesson(WorkshopExplorationLessonPolicy.APPLIED_EDIT);
                if (sourceChanged && refreshedSymbol != null) {
                    recordOnboardingChangeApplied(refreshedSymbol, refreshedSymbol.source);
                }
                setStatusText("Saved to .stasis file - " + reload + " - " + compileResult);
            } else {
                WorkshopSourceDiagnostic location = WorkshopSourceDiagnostic.fromCompileResult(compileResult);
                if (location == null) {
                    location = new WorkshopSourceDiagnostic(editedSymbol.file, 0, 0, 0, 0,
                            editedSymbol.name, compileResult);
                }
                applySourceDiagnostic(location, "Compile failed");
                selectedRecoveryEntry = AndroidEditRecoveryStore.record(this, activeRecoveryProjectId(), editedSymbol.file,
                        editedSymbol.name, beforeFileSource, editedSymbol.sourceFile.source, compileResult, location);
                setStatusText("Saved edit failed compile; use Go to Diagnostic or Undo Failed Apply");
            }
        } catch (IOException error) {
            setStatusText("Save failed: " + error.getMessage());
        } catch (Exception error) {
            setStatusText("Recovery journal failed: " + error.getMessage());
        }
    }

    private String activeRecoveryProjectId() {
        return activeProject == null ? Integer.toHexString(projectRootPath().hashCode()) : activeProject.id;
    }

    private void refreshRecoveryStatus() {
        if (diagnosticStatus == null) return;
        try {
            AndroidEditRecoveryStore.Entry[] entries = AndroidEditRecoveryStore.list(this, activeRecoveryProjectId());
            if (entries.length == 0) {
                selectedRecoveryEntry = null;
                diagnosticStatus.setText("Diagnostics: no failed manual applies");
                return;
            }
            AndroidEditRecoveryStore.Entry entry = selectedRecoveryEntry;
            int selectedIndex = -1;
            for (int index = 0; index < entries.length; index += 1) {
                if (entry != null && entries[index].file.equals(entry.file)) selectedIndex = index;
            }
            if (selectedIndex < 0) {
                selectedIndex = 0;
                entry = entries[0];
            }
            selectedRecoveryEntry = entry;
            diagnosticFile = entry.diagnosticPath;
            diagnosticSymbol = entry.diagnosticSymbol;
            diagnosticLine = entry.diagnosticLine;
            diagnosticColumn = entry.diagnosticColumn;
            diagnosticEndLine = entry.diagnosticEndLine;
            diagnosticEndColumn = entry.diagnosticEndColumn;
            diagnosticStatus.setText("Recoverable failed apply history: " + entries.length + " entries\nselected="
                    + (selectedIndex + 1) + "/" + entries.length + "\nfile=" + entry.path
                    + "\ndiagnostic_file=" + entry.diagnosticPath
                    + (entry.diagnosticLine > 0 ? ":" + entry.diagnosticLine : "")
                    + "\nsymbol=" + entry.diagnosticSymbol + "\n" + entry.diagnostic);
        } catch (Exception error) {
            diagnosticStatus.setText("Recovery history unavailable: " + error.getMessage());
        }
    }

    private void requestImageImport() {
        if (activeProject == null) {
            setStatusText("Image import needs a registered active project");
            return;
        }
        if (isGitHubOperationActive() || projectIoActive || hasPendingSourceEdit()) {
            setStatusText("Image import blocked by active work or a pending source edit");
            return;
        }
        Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        intent.setType("image/*");
        intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
        try {
            startActivityForResult(intent, IMPORT_IMAGE_REQUEST);
        } catch (Exception error) {
            setStatusText("Image picker failed: " + error.getMessage());
        }
    }

    private void requestAudioImport() {
        if (!canModifyAudioAssets()) return;
        Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        intent.setType("audio/*");
        intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
        try {
            startActivityForResult(intent, IMPORT_AUDIO_REQUEST);
        } catch (Exception error) {
            setStatusText("Audio picker failed: " + error.getMessage());
        }
    }

    private void requestAudioRecording() {
        if (!canModifyAudioAssets()) return;
        if (audioRecordingActive) {
            setStatusText("Audio recording is already active; use Stop & Save or Cancel Recording");
            return;
        }
        String requestedName = audioRecordingNameEditor == null
                ? "" : audioRecordingNameEditor.getText().toString().trim();
        if (!requestedName.matches("[A-Za-z0-9][A-Za-z0-9_-]{0,63}")) {
            setStatusText("Recording name must use 1-64 letters, numbers, underscores, or hyphens");
            return;
        }
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            requestPermissions(new String[] {Manifest.permission.RECORD_AUDIO}, AUDIO_RECORD_PERMISSION_REQUEST);
            setStatusText("Audio recording is waiting for microphone permission");
            return;
        }
        startAudioRecording();
    }

    private void startAudioRecording() {
        if (activeProject == null || audioRecordingActive) return;
        stopAudioPreview();
        File temporary = null;
        MediaRecorder recorder = null;
        try {
            temporary = WorkshopAudioAssets.createRecordingFile(activeProject.root);
            recorder = new MediaRecorder();
            recorder.setAudioSource(MediaRecorder.AudioSource.MIC);
            recorder.setOutputFormat(MediaRecorder.OutputFormat.MPEG_4);
            recorder.setAudioEncoder(MediaRecorder.AudioEncoder.AAC);
            recorder.setAudioEncodingBitRate(128_000);
            recorder.setAudioSamplingRate(44_100);
            recorder.setMaxDuration((int)WorkshopAudioAssets.MAX_DURATION_MS);
            recorder.setMaxFileSize(WorkshopAudioAssets.MAX_AUDIO_BYTES);
            recorder.setOutputFile(temporary.getAbsolutePath());
            recorder.setOnInfoListener(new MediaRecorder.OnInfoListener() {
                @Override public void onInfo(MediaRecorder ignored, int what, int extra) {
                    if (what == MediaRecorder.MEDIA_RECORDER_INFO_MAX_DURATION_REACHED
                            || what == MediaRecorder.MEDIA_RECORDER_INFO_MAX_FILESIZE_REACHED) {
                        finishAudioRecording(true);
                    }
                }
            });
            recorder.prepare();
            recorder.start();
            activeAudioRecordingFile = temporary;
            activeAudioRecorder = recorder;
            audioRecordingActive = true;
            setStatusText("Audio recording active: bounded to five minutes and 16 MiB; use Stop & Save or Cancel");
        } catch (Exception error) {
            if (recorder != null) recorder.release();
            if (temporary != null) {
                try {
                    WorkshopAudioAssets.discardRecording(temporary, activeProject.root);
                } catch (Exception ignored) {
                }
            }
            setStatusText("Audio recording failed to start: " + error.getMessage());
        }
    }

    private void finishAudioRecording(boolean save) {
        if (!audioRecordingActive || activeAudioRecorder == null || activeAudioRecordingFile == null) {
            if (save) setStatusText("No audio recording is active");
            return;
        }
        MediaRecorder recorder = activeAudioRecorder;
        File temporary = activeAudioRecordingFile;
        File project = activeProject == null ? projectRoot() : activeProject.root;
        activeAudioRecorder = null;
        activeAudioRecordingFile = null;
        audioRecordingActive = false;
        try {
            recorder.stop();
        } catch (RuntimeException stopError) {
            recorder.release();
            try {
                WorkshopAudioAssets.discardRecording(temporary, project);
            } catch (Exception cleanupError) {
                stopError.addSuppressed(cleanupError);
            }
            setStatusText("Audio recording discarded after stop failure: " + stopError.getMessage());
            return;
        }
        recorder.release();
        try {
            if (!save) {
                WorkshopAudioAssets.discardRecording(temporary, project);
                setStatusText("Audio recording cancelled; project assets unchanged");
                return;
            }
            String name = audioRecordingNameEditor == null
                    ? "recorded_audio" : audioRecordingNameEditor.getText().toString().trim();
            WorkshopAudioAssets.AssetInfo recorded = WorkshopAudioAssets.publishRecording(temporary, project, name);
            refreshAudioAssetList();
            setStatusText("Audio recording saved: " + recorded.relativePath + " - "
                    + formatDuration(recorded.durationMs));
        } catch (Exception error) {
            try {
                WorkshopAudioAssets.discardRecording(temporary, project);
            } catch (Exception cleanupError) {
                error.addSuppressed(cleanupError);
            }
            setStatusText("Audio recording discarded after stop/validation failure: " + error.getMessage());
        }
    }

    private void cancelAudioRecording(boolean report) {
        if (!audioRecordingActive) return;
        finishAudioRecording(false);
        if (!report && reloadStatus != null) reloadStatus.setText("Audio recording cancelled on app pause");
    }

    private void refreshAudioAssetList() {
        requestGitHubAutoSync();
        if (audioAssetList == null) return;
        audioAssetList.removeAllViews();
        if (activeProject == null) return;
        try {
            List<WorkshopAudioAssets.AssetInfo> assets = WorkshopAudioAssets.list(activeProject.root);
            if (assets.isEmpty()) {
                TextView empty = new TextView(this);
                empty.setText("No imported audio");
                empty.setTextSize(12.0f);
                empty.setTextColor(Color.rgb(73, 84, 100));
                audioAssetList.addView(empty, fullWidth());
                return;
            }
            for (final WorkshopAudioAssets.AssetInfo asset : assets) {
                Button actions = new Button(this);
                actions.setAllCaps(false);
                actions.setText(asset.relativePath + "\n" + formatDuration(asset.durationMs)
                        + " - " + asset.sampleRate + " Hz / " + asset.channels + " ch - "
                        + asset.bytes + " bytes");
                actions.setContentDescription("Audio asset " + asset.relativePath + ", duration "
                        + formatDuration(asset.durationMs) + ". Tap for preview and actions.");
                actions.setOnClickListener(new View.OnClickListener() {
                    @Override public void onClick(View view) { showAudioAssetActions(asset); }
                });
                audioAssetList.addView(actions, fullWidth());
            }
        } catch (Exception error) {
            TextView failure = new TextView(this);
            failure.setText("Audio library unavailable: " + error.getMessage());
            failure.setTextColor(Color.rgb(164, 45, 45));
            audioAssetList.addView(failure, fullWidth());
        }
    }

    private void showAudioAssetActions(final WorkshopAudioAssets.AssetInfo asset) {
        new AlertDialog.Builder(this)
                .setTitle(asset.relativePath)
                .setItems(new String[] {"Preview", "Rename", "Delete"},
                        new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        if (which == 0) previewAudioAsset(asset);
                        else if (which == 1) requestAudioRename(asset);
                        else if (which == 2) requestAudioDelete(asset);
                    }
                })
                .setNegativeButton("Cancel", null)
                .show();
    }

    private void previewAudioAsset(final WorkshopAudioAssets.AssetInfo asset) {
        stopAudioPreview();
        try {
            MediaPlayer player = new MediaPlayer();
            player.setDataSource(asset.file.getAbsolutePath());
            player.setOnCompletionListener(new MediaPlayer.OnCompletionListener() {
                @Override public void onCompletion(MediaPlayer completed) {
                    completed.release();
                    if (activeAudioPreview == completed) activeAudioPreview = null;
                    setStatusText("Audio preview complete: " + asset.relativePath);
                }
            });
            player.prepare();
            activeAudioPreview = player;
            player.start();
            setStatusText("Audio preview playing: " + asset.relativePath + " - " + formatDuration(asset.durationMs));
        } catch (Exception error) {
            stopAudioPreview();
            setStatusText("Audio preview failed: " + error.getMessage());
        }
    }

    private void stopAudioPreview() {
        MediaPlayer player = activeAudioPreview;
        activeAudioPreview = null;
        if (player == null) return;
        try {
            if (player.isPlaying()) player.stop();
        } catch (RuntimeException ignored) {
        }
        player.release();
    }

    private void requestAudioRename(final WorkshopAudioAssets.AssetInfo asset) {
        if (!canModifyAudioAssets()) return;
        List<String> references = audioReferences(asset);
        if (!references.isEmpty()) {
            setStatusText("Audio rename blocked: referenced by " + joinPaths(references));
            return;
        }
        final EditText name = new EditText(this);
        String current = asset.file.getName();
        int dot = current.lastIndexOf('.');
        name.setText(dot > 0 ? current.substring(0, dot) : current);
        name.setSingleLine(true);
        new AlertDialog.Builder(this)
                .setTitle("Rename Audio")
                .setView(name)
                .setPositiveButton("Rename", new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        try {
                            stopAudioPreview();
                            WorkshopAudioAssets.AssetInfo renamed = WorkshopAudioAssets.rename(
                                    asset, activeProject.root, name.getText().toString());
                            refreshAudioAssetList();
                            setStatusText("Audio renamed: " + renamed.relativePath);
                        } catch (Exception error) {
                            setStatusText("Audio rename failed: " + error.getMessage());
                        }
                    }
                })
                .setNegativeButton("Cancel", null)
                .show();
    }

    private void requestAudioDelete(final WorkshopAudioAssets.AssetInfo asset) {
        if (!canModifyAudioAssets()) return;
        List<String> references = audioReferences(asset);
        if (!references.isEmpty()) {
            setStatusText("Audio delete blocked: referenced by " + joinPaths(references));
            return;
        }
        new AlertDialog.Builder(this)
                .setTitle("Delete Audio?")
                .setMessage(asset.relativePath + " will move to bounded project recovery.")
                .setPositiveButton("Delete", new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        try {
                            stopAudioPreview();
                            WorkshopAudioAssets.moveToTrash(asset, activeProject.root);
                            refreshAudioAssetList();
                            setStatusText("Audio moved to recovery: " + asset.relativePath);
                        } catch (Exception error) {
                            setStatusText("Audio delete failed: " + error.getMessage());
                        }
                    }
                })
                .setNegativeButton("Cancel", null)
                .show();
    }

    private void restoreLastDeletedAudio() {
        if (!canModifyAudioAssets()) return;
        try {
            WorkshopAudioAssets.AssetInfo restored = WorkshopAudioAssets.restoreLatest(activeProject.root);
            refreshAudioAssetList();
            setStatusText("Audio restored: " + restored.relativePath);
        } catch (Exception error) {
            setStatusText("Audio restore failed: " + error.getMessage());
        }
    }

    private boolean canModifyAudioAssets() {
        if (activeProject == null) {
            setStatusText("Audio changes need a registered active project");
            return false;
        }
        if (isGitHubOperationActive() || projectIoActive || hasPendingSourceEdit()) {
            setStatusText("Audio change blocked by active work or a pending source edit");
            return false;
        }
        return true;
    }

    private List<String> audioReferences(WorkshopAudioAssets.AssetInfo asset) {
        ArrayList<String> references = new ArrayList<>();
        for (SourceFile source : loadBundledProject().files) {
            if (source.source.contains(asset.relativePath) || source.source.contains(asset.file.getName())) {
                references.add(source.path);
            }
        }
        return references;
    }

    private static String formatDuration(long durationMs) {
        long seconds = durationMs / 1000L;
        long minutes = seconds / 60L;
        long remainder = seconds % 60L;
        return minutes + ":" + (remainder < 10L ? "0" : "") + remainder;
    }

    private void refreshImageAssetList() {
        requestGitHubAutoSync();
        if (imageAssetList == null) return;
        imageAssetList.removeAllViews();
        if (activeProject == null) return;
        try {
            List<WorkshopImageAssets.AssetInfo> assets = WorkshopImageAssets.list(activeProject.root);
            if (assets.isEmpty()) {
                TextView empty = new TextView(this);
                empty.setText("No imported images");
                empty.setTextSize(12.0f);
                empty.setTextColor(Color.rgb(73, 84, 100));
                imageAssetList.addView(empty, fullWidth());
                return;
            }
            for (final WorkshopImageAssets.AssetInfo asset : assets) {
                Button preview = new Button(this);
                preview.setAllCaps(false);
                preview.setText(asset.relativePath + "\n" + asset.width + "x" + asset.height
                        + " - " + asset.bytes + " bytes");
                preview.setContentDescription("Image asset " + asset.relativePath + ", "
                        + asset.width + " by " + asset.height + " pixels. Tap for actions.");
                preview.setOnClickListener(new View.OnClickListener() {
                    @Override public void onClick(View view) { showImageAssetActions(asset); }
                });
                imageAssetList.addView(preview, fullWidth());
            }
        } catch (Exception error) {
            TextView failure = new TextView(this);
            failure.setText("Image library unavailable: " + error.getMessage());
            failure.setTextColor(Color.rgb(164, 45, 45));
            imageAssetList.addView(failure, fullWidth());
        }
    }

    private void showImageAssetActions(final WorkshopImageAssets.AssetInfo asset) {
        String[] actions = new String[] {"Preview",
                "Paint as Copy", "Rename", "Delete"};
        new AlertDialog.Builder(this)
                .setTitle(asset.relativePath)
                .setItems(actions, new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        if (which == 0) showImagePreview(asset);
                        else if (which == 1) openPaintEditor(asset);
                        else if (which == 2) requestImageRename(asset);
                        else if (which == 3) requestImageDelete(asset);
                    }
                })
                .setNegativeButton("Cancel", null)
                .show();
    }

    private void restoreRetainedPaintSession() {
        Object retained = getLastNonConfigurationInstance();
        if (!(retained instanceof RetainedPaintSession)) return;
        RetainedPaintSession session = (RetainedPaintSession)retained;
        try {
            showPaintEditor(session.bitmap.getWidth(), session.bitmap.getHeight(), session.bitmap,
                    session.name, session);
        } finally {
            if (!session.bitmap.isRecycled()) session.bitmap.recycle();
        }
    }

    private void requestNewPaintedImage() {
        if (!canModifyImageAssets()) return;
        WorkshopAdaptiveLayout.Profile layout = adaptiveLayoutProfile();
        final EditText width = new EditText(this);
        width.setHint("Width");
        width.setContentDescription("Canvas width in pixels");
        width.setInputType(InputType.TYPE_CLASS_NUMBER);
        width.setText("256");
        final EditText height = new EditText(this);
        height.setHint("Height");
        height.setContentDescription("Canvas height in pixels");
        height.setInputType(InputType.TYPE_CLASS_NUMBER);
        height.setText("256");
        LinearLayout dimensions = new LinearLayout(this);
        configureActionRow(dimensions, layout);
        dimensions.addView(width, actionWidth(layout));
        dimensions.addView(height, actionWidth(layout));
        new AlertDialog.Builder(this)
                .setTitle("New Paint Canvas")
                .setMessage("Canvas dimensions must be 16-1024 pixels.")
                .setView(dimensions)
                .setPositiveButton("Create", new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        try {
                            int requestedWidth = Integer.parseInt(width.getText().toString());
                            int requestedHeight = Integer.parseInt(height.getText().toString());
                            showPaintEditor(requestedWidth, requestedHeight, null, "painted_image");
                        } catch (Exception error) {
                            setStatusText("Paint canvas failed: " + error.getMessage());
                        }
                    }
                })
                .setNegativeButton("Cancel", null)
                .show();
    }

    private void openPaintEditor(WorkshopImageAssets.AssetInfo asset) {
        if (!canModifyImageAssets()) return;
        try {
            Bitmap initial = WorkshopImageAssets.decodeForPaint(asset);
            String name = asset.file.getName();
            int dot = name.lastIndexOf('.');
            if (dot > 0) name = name.substring(0, dot);
            showPaintEditor(asset.width, asset.height, initial, name + "_edit");
            initial.recycle();
        } catch (Exception error) {
            setStatusText("Paint editor failed: " + error.getMessage());
        }
    }

    private void showPaintEditor(int width, int height, Bitmap initial, String defaultName) {
        showPaintEditor(width, height, initial, defaultName, null);
    }

    private void showPaintEditor(int width, int height, Bitmap initial, String defaultName,
                                 RetainedPaintSession retained) {
        final WorkshopAdaptiveLayout.Profile layout = adaptiveLayoutProfile();
        final WorkshopPaintView paint = new WorkshopPaintView(this, width, height, initial);
        if (retained != null) {
            paint.setBrushColor(retained.brushColor);
            paint.setBrushSize(retained.brushSize);
            paint.setEraser(retained.erasing);
        }
        final ArrayList<View> paintTraversal = new ArrayList<>();
        paintTraversal.add(paint);
        LinearLayout content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        content.setPadding(dp(8), dp(8), dp(8), dp(8));
        content.addView(paint, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT, dp(layout.paintCanvasHeightDp)));

        LinearLayout tools = new LinearLayout(this);
        configureActionRow(tools, layout);
        Button brush = compactButton("Brush");
        Button eraser = compactButton("Eraser");
        Button undo = compactButton("Undo");
        Button redo = compactButton("Redo");
        brush.setSelected(!paint.isErasing());
        eraser.setSelected(paint.isErasing());
        tools.addView(brush, actionWidth(layout));
        tools.addView(eraser, actionWidth(layout));
        tools.addView(undo, actionWidth(layout));
        tools.addView(redo, actionWidth(layout));
        paintTraversal.add(brush);
        paintTraversal.add(eraser);
        paintTraversal.add(undo);
        paintTraversal.add(redo);
        content.addView(tools, fullWidth());
        brush.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) {
                paint.setEraser(false);
                brush.setSelected(true);
                eraser.setSelected(false);
                paint.announceForAccessibility("Brush selected");
                setStatusText("Paint tool: brush");
            }
        });
        eraser.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) {
                paint.setEraser(true);
                brush.setSelected(false);
                eraser.setSelected(true);
                paint.announceForAccessibility("Eraser selected");
                setStatusText("Paint tool: eraser");
            }
        });
        undo.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { paint.undo(); }
        });
        redo.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { paint.redo(); }
        });

        LinearLayout sizes = new LinearLayout(this);
        configureActionRow(sizes, layout);
        final ArrayList<Button> sizeChoices = new ArrayList<>();
        for (final int size : new int[] {2, 8, 24, 64}) {
            final Button choice = compactButton(Integer.toString(size) + "px");
            choice.setSelected(Math.round(paint.brushSize()) == size);
            choice.setOnClickListener(new View.OnClickListener() {
                @Override public void onClick(View view) {
                    paint.setBrushSize(size);
                    selectOnly(sizeChoices, choice);
                    paint.announceForAccessibility("Brush size " + size + " pixels selected");
                }
            });
            sizes.addView(choice, actionWidth(layout));
            sizeChoices.add(choice);
            paintTraversal.add(choice);
        }
        content.addView(sizes, fullWidth());

        LinearLayout palette = new LinearLayout(this);
        configureActionRow(palette, layout);
        final int[] colors = new int[] {Color.BLACK, Color.WHITE, Color.RED, Color.GREEN, Color.BLUE};
        final String[] colorNames = new String[] {"Black", "White", "Red", "Green", "Blue"};
        final ArrayList<Button> colorChoices = new ArrayList<>();
        for (int index = 0; index < colors.length; index++) {
            final int color = colors[index];
            final String colorName = colorNames[index];
            final Button choice = compactButton(colorName);
            choice.setSelected(paint.brushColor() == color);
            choice.setOnClickListener(new View.OnClickListener() {
                @Override public void onClick(View view) {
                    paint.setBrushColor(color);
                    brush.setSelected(true);
                    eraser.setSelected(false);
                    selectOnly(colorChoices, choice);
                    paint.announceForAccessibility(colorName + " paint color selected");
                }
            });
            palette.addView(choice, actionWidth(layout));
            colorChoices.add(choice);
            paintTraversal.add(choice);
        }
        content.addView(palette, fullWidth());

        LinearLayout customColor = new LinearLayout(this);
        configureActionRow(customColor, layout);
        final EditText hex = new EditText(this);
        hex.setHint("#RRGGBB or #AARRGGBB");
        hex.setSingleLine(true);
        Button applyColor = compactButton("Set Color");
        customColor.addView(hex, actionWidth(layout));
        customColor.addView(applyColor, actionWidth(layout));
        paintTraversal.add(hex);
        paintTraversal.add(applyColor);
        content.addView(customColor, fullWidth());
        applyColor.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) {
                try {
                    paint.setBrushColor(Color.parseColor(hex.getText().toString().trim()));
                    brush.setSelected(true);
                    eraser.setSelected(false);
                    selectOnly(colorChoices, null);
                    paint.announceForAccessibility("Custom paint color selected");
                    setStatusText("Paint color applied");
                } catch (Exception error) {
                    setStatusText("Paint color needs #RRGGBB or #AARRGGBB");
                }
            }
        });

        LinearLayout canvasActions = new LinearLayout(this);
        configureActionRow(canvasActions, layout);
        Button resize = compactButton("Resize / Crop");
        Button clear = compactButton("Clear");
        canvasActions.addView(resize, actionWidth(layout));
        canvasActions.addView(clear, actionWidth(layout));
        paintTraversal.add(resize);
        paintTraversal.add(clear);
        content.addView(canvasActions, fullWidth());
        resize.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { requestPaintResize(paint); }
        });
        clear.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { paint.clearCanvas(); }
        });

        final EditText name = new EditText(this);
        name.setHint("Save-as image name");
        name.setSingleLine(true);
        name.setText(defaultName);
        content.addView(name, fullWidth());
        paintTraversal.add(name);
        LinearLayout finish = new LinearLayout(this);
        configureActionRow(finish, layout);
        Button save = compactButton("Save as PNG");
        Button cancel = compactButton("Cancel");
        finish.addView(save, actionWidth(layout));
        finish.addView(cancel, actionWidth(layout));
        paintTraversal.add(save);
        paintTraversal.add(cancel);
        content.addView(finish, fullWidth());

        ScrollView editorScroll = new ScrollView(this);
        editorScroll.addView(content, fullWidth());
        final AlertDialog dialog = new AlertDialog.Builder(this)
                .setTitle("Mini Paint - " + width + "x" + height)
                .setView(editorScroll)
                .create();
        activePaintView = paint;
        activePaintDialog = dialog;
        activePaintName = name;
        save.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) {
                savePaintedImage(paint, name.getText().toString(), dialog);
            }
        });
        cancel.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) {
                setStatusText("Paint cancelled; project assets unchanged");
                dialog.dismiss();
            }
        });
        dialog.setOnDismissListener(new android.content.DialogInterface.OnDismissListener() {
            @Override public void onDismiss(android.content.DialogInterface ignored) {
                paint.dispose();
                if (activePaintDialog == dialog) {
                    activePaintView = null;
                    activePaintDialog = null;
                    activePaintName = null;
                }
            }
        });
        dialog.show();
        chainAccessibilityTraversal(paintTraversal.toArray(new View[paintTraversal.size()]));
    }

    private void savePaintedImage(WorkshopPaintView paint, String name, AlertDialog dialog) {
        Bitmap snapshot = paint.snapshot();
        try {
            WorkshopImageAssets.AssetInfo saved = WorkshopImageAssets.savePainted(
                    snapshot, activeProject.root, name);
            refreshImageAssetList();
            setStatusText("Painted image saved as copy: " + saved.relativePath);
            dialog.dismiss();
        } catch (Exception error) {
            setStatusText("Paint save failed: " + error.getMessage());
        } finally {
            snapshot.recycle();
        }
    }

    private void requestPaintResize(final WorkshopPaintView paint) {
        WorkshopAdaptiveLayout.Profile layout = adaptiveLayoutProfile();
        final EditText width = new EditText(this);
        width.setInputType(InputType.TYPE_CLASS_NUMBER);
        width.setContentDescription("Canvas width in pixels");
        width.setText(Integer.toString(paint.canvasWidth()));
        final EditText height = new EditText(this);
        height.setInputType(InputType.TYPE_CLASS_NUMBER);
        height.setContentDescription("Canvas height in pixels");
        height.setText(Integer.toString(paint.canvasHeight()));
        LinearLayout dimensions = new LinearLayout(this);
        configureActionRow(dimensions, layout);
        dimensions.addView(width, actionWidth(layout));
        dimensions.addView(height, actionWidth(layout));
        new AlertDialog.Builder(this)
                .setTitle("Resize / Crop Canvas")
                .setMessage("Pixels outside the new bottom/right edges are cropped; new space is transparent.")
                .setView(dimensions)
                .setPositiveButton("Apply", new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        try {
                            paint.resizeCanvas(Integer.parseInt(width.getText().toString()),
                                    Integer.parseInt(height.getText().toString()));
                        } catch (Exception error) {
                            setStatusText("Paint resize failed: " + error.getMessage());
                        }
                    }
                })
                .setNegativeButton("Cancel", null)
                .show();
    }

    private Button compactButton(String label) {
        Button button = new Button(this);
        button.setText(label);
        button.setAllCaps(false);
        button.setTextSize(14.0f);
        button.setMinHeight(dp(48));
        button.setPadding(dp(2), 0, dp(2), 0);
        return button;
    }

    private static LinearLayout.LayoutParams weightedWidth() {
        return new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1.0f);
    }

    private void requestImageRename(final WorkshopImageAssets.AssetInfo asset) {
        if (!canModifyImageAssets()) return;
        List<String> references = imageReferences(asset);
        if (!references.isEmpty()) {
            setStatusText("Rename blocked: image is referenced by " + joinPaths(references));
            return;
        }
        final EditText name = new EditText(this);
        String current = asset.file.getName();
        int dot = current.lastIndexOf('.');
        name.setText(dot > 0 ? current.substring(0, dot) : current);
        name.setSingleLine(true);
        new AlertDialog.Builder(this)
                .setTitle("Rename Image")
                .setMessage("References are checked before the file is renamed.")
                .setView(name)
                .setPositiveButton("Rename", new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        try {
                            WorkshopImageAssets.AssetInfo renamed = WorkshopImageAssets.rename(
                                    asset, activeProject.root, name.getText().toString());
                            refreshImageAssetList();
                            setStatusText("Image renamed: " + renamed.relativePath);
                        } catch (Exception error) {
                            setStatusText("Image rename failed: " + error.getMessage());
                        }
                    }
                })
                .setNegativeButton("Cancel", null)
                .show();
    }

    private void requestImageDelete(final WorkshopImageAssets.AssetInfo asset) {
        if (!canModifyImageAssets()) return;
        List<String> references = imageReferences(asset);
        if (!references.isEmpty()) {
            setStatusText("Delete blocked: image is referenced by " + joinPaths(references));
            return;
        }
        new AlertDialog.Builder(this)
                .setTitle("Delete Image?")
                .setMessage(asset.relativePath + " will move to bounded project recovery.")
                .setPositiveButton("Delete", new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        try {
                            WorkshopImageAssets.moveToTrash(asset, activeProject.root);
                            refreshImageAssetList();
                            setStatusText("Image moved to recovery: " + asset.relativePath);
                        } catch (Exception error) {
                            setStatusText("Image delete failed: " + error.getMessage());
                        }
                    }
                })
                .setNegativeButton("Cancel", null)
                .show();
    }

    private void restoreLastDeletedImage() {
        if (!canModifyImageAssets()) return;
        try {
            WorkshopImageAssets.AssetInfo restored = WorkshopImageAssets.restoreLatest(activeProject.root);
            refreshImageAssetList();
            setStatusText("Image restored: " + restored.relativePath);
        } catch (Exception error) {
            setStatusText("Image restore failed: " + error.getMessage());
        }
    }

    private boolean canModifyImageAssets() {
        if (activeProject == null) {
            setStatusText("Image changes need a registered active project");
            return false;
        }
        if (isGitHubOperationActive() || projectIoActive || hasPendingSourceEdit()) {
            setStatusText("Image change blocked by active work or a pending source edit");
            return false;
        }
        return true;
    }

    private List<String> imageReferences(WorkshopImageAssets.AssetInfo asset) {
        ArrayList<String> references = new ArrayList<>();
        ProjectSnapshot project = loadBundledProject();
        for (SourceFile source : project.files) {
            if (source.source.contains(asset.relativePath) || source.source.contains(asset.file.getName())) {
                references.add(source.path);
            }
        }
        return references;
    }

    private static String joinPaths(List<String> paths) {
        StringBuilder joined = new StringBuilder();
        for (String path : paths) {
            if (joined.length() > 0) joined.append(", ");
            joined.append(path);
        }
        return joined.toString();
    }

    private void showImagePreview(WorkshopImageAssets.AssetInfo asset) {
        try {
            Bitmap bitmap = WorkshopImageAssets.decodePreview(asset);
            ImageView preview = new ImageView(this);
            preview.setAdjustViewBounds(true);
            preview.setScaleType(ImageView.ScaleType.FIT_CENTER);
            preview.setPadding(dp(12), dp(12), dp(12), dp(12));
            preview.setImageBitmap(bitmap);
            new AlertDialog.Builder(this)
                    .setTitle(asset.relativePath)
                    .setView(preview)
                    .setMessage(asset.width + "x" + asset.height + " - " + asset.bytes + " bytes")
                    .setPositiveButton("Close", null)
                    .show();
        } catch (Exception error) {
            setStatusText("Image preview failed: " + error.getMessage());
        }
    }

    private void showRecoveryHistory() {
        try {
            final AndroidEditRecoveryStore.Entry[] entries =
                    AndroidEditRecoveryStore.list(this, activeRecoveryProjectId());
            if (entries.length == 0) {
                setStatusText("No failed manual apply history is available");
                return;
            }
            String[] labels = new String[entries.length];
            for (int index = 0; index < entries.length; index += 1) {
                AndroidEditRecoveryStore.Entry entry = entries[index];
                String when = java.text.DateFormat.getDateTimeInstance(
                        java.text.DateFormat.SHORT, java.text.DateFormat.SHORT)
                        .format(new java.util.Date(entry.timestampMs));
                labels[index] = when + " - " + entry.path
                        + (entry.symbol.isEmpty() ? "" : " - " + entry.symbol);
            }
            new AlertDialog.Builder(this)
                    .setTitle("Failed Apply History")
                    .setItems(labels, new android.content.DialogInterface.OnClickListener() {
                        @Override public void onClick(android.content.DialogInterface dialog, int which) {
                            selectedRecoveryEntry = entries[which];
                            diagnosticFile = selectedRecoveryEntry.diagnosticPath;
                            diagnosticSymbol = selectedRecoveryEntry.diagnosticSymbol;
                            diagnosticLine = selectedRecoveryEntry.diagnosticLine;
                            diagnosticColumn = selectedRecoveryEntry.diagnosticColumn;
                            diagnosticEndLine = selectedRecoveryEntry.diagnosticEndLine;
                            diagnosticEndColumn = selectedRecoveryEntry.diagnosticEndColumn;
                            diagnosticStatus.setText("Recovery history selection " + (which + 1) + "/"
                                    + entries.length + "\nfile=" + diagnosticFile + "\nsymbol="
                                    + diagnosticSymbol + "\n" + selectedRecoveryEntry.diagnostic);
                            setStatusText("Selected failed apply " + (which + 1) + " of " + entries.length);
                        }
                    })
                    .setNegativeButton("Cancel", null)
                    .show();
        } catch (Exception error) {
            setStatusText("Recovery history unavailable: " + error.getMessage());
        }
    }

    private void captureFirstTestFailureDiagnostic(JSONObject testRun) {
        JSONObject compile = testRun.optJSONObject("compile");
        WorkshopSourceDiagnostic compileDiagnostic = compile == null ? null
                : WorkshopSourceDiagnostic.fromCompileResult(compile.optString("raw", ""));
        if (compileDiagnostic != null) applySourceDiagnostic(compileDiagnostic, "Compile failure");
        JSONArray runs = testRun.optJSONArray("stasis_test_files");
        if (runs == null) return;
        for (int runIndex = 0; runIndex < runs.length(); runIndex += 1) {
            JSONObject run = runs.optJSONObject(runIndex);
            JSONArray results = run == null ? null : run.optJSONArray("results");
            if (results == null) continue;
            for (int resultIndex = 0; resultIndex < results.length(); resultIndex += 1) {
                JSONObject result = results.optJSONObject(resultIndex);
                if (result == null || result.optBoolean("passed", false)) continue;
                String error = result.optString("error", "");
                WorkshopSourceDiagnostic diagnostic = WorkshopSourceDiagnostic.fromTestFailure(
                        result.optString("file", ""), result.optInt("line", 0),
                        result.optInt("column", 1),
                        result.optString("name", ""), error);
                if (diagnostic != null) applySourceDiagnostic(diagnostic, "Test failure");
                return;
            }
        }
    }

    private void applySourceDiagnostic(WorkshopSourceDiagnostic diagnostic, String kind) {
        diagnosticFile = diagnostic.file;
        diagnosticSymbol = diagnostic.symbol;
        diagnosticLine = diagnostic.line;
        diagnosticColumn = diagnostic.column;
        diagnosticEndLine = diagnostic.endLine;
        diagnosticEndColumn = diagnostic.endColumn;
        diagnosticStatus.setText(diagnostic.displayText(kind));
    }

    private void goToDiagnosticSource() {
        if (diagnosticFile.isEmpty()) {
            setStatusText("No source diagnostic is available");
            return;
        }
        ProjectSnapshot project = loadBundledProject();
        SymbolEntry fileFallback = null;
        for (SymbolSection section : project.sections) {
            for (SymbolGroup group : section.groups) {
                for (SymbolEntry symbol : group.symbols) {
                    if (!symbol.file.equals(diagnosticFile)) continue;
                    if (fileFallback == null || (diagnosticLine > 0
                            && symbol.start <= WorkshopSourceDiagnostic.sourceOffset(
                                    symbol.sourceFile.source, diagnosticLine, diagnosticColumn)
                            && symbol.end >= WorkshopSourceDiagnostic.sourceOffset(
                                    symbol.sourceFile.source, diagnosticLine, diagnosticColumn))) {
                        fileFallback = symbol;
                    }
                    if (diagnosticSymbol.isEmpty() || symbol.name.equals(diagnosticSymbol)) {
                        showSymbol(symbol);
                        if (diagnosticLine > 0) {
                            int absoluteOffset = WorkshopSourceDiagnostic.sourceOffset(
                                    symbol.sourceFile.source, diagnosticLine, diagnosticColumn);
                            int absoluteEnd = WorkshopSourceDiagnostic.sourceOffset(
                                    symbol.sourceFile.source, diagnosticEndLine, diagnosticEndColumn);
                            int symbolOffset = Math.max(0,
                                    Math.min(symbol.source.length(), absoluteOffset - symbol.start));
                            int symbolEnd = Math.max(symbolOffset,
                                    Math.min(symbol.source.length(), absoluteEnd - symbol.start));
                            sourceEditor.setSelection(symbolOffset, symbolEnd);
                        }
                        manualEditBody.setVisibility(View.VISIBLE);
                        setStatusText("Opened diagnostic source " + diagnosticFile
                                + (diagnosticLine > 0 ? ":" + diagnosticLine : "")
                                + " - " + symbol.displayName());
                        return;
                    }
                }
            }
        }
        if (fileFallback != null) {
            showSymbol(fileFallback);
            int absoluteOffset = WorkshopSourceDiagnostic.sourceOffset(
                    fileFallback.sourceFile.source, diagnosticLine, diagnosticColumn);
            int absoluteEnd = WorkshopSourceDiagnostic.sourceOffset(
                    fileFallback.sourceFile.source, diagnosticEndLine, diagnosticEndColumn);
            int start = Math.max(0,
                    Math.min(fileFallback.source.length(), absoluteOffset - fileFallback.start));
            int end = Math.max(start,
                    Math.min(fileFallback.source.length(), absoluteEnd - fileFallback.start));
            sourceEditor.setSelection(start, end);
            manualEditBody.setVisibility(View.VISIBLE);
            setStatusText("Opened diagnostic file " + diagnosticFile
                    + (diagnosticLine > 0 ? ":" + diagnosticLine : ""));
            return;
        }
        for (SourceFile file : project.files) {
            if (!file.path.equals(diagnosticFile)) continue;
            int absoluteOffset = WorkshopSourceDiagnostic.sourceOffset(
                    file.source, diagnosticLine, diagnosticColumn);
            int absoluteEnd = WorkshopSourceDiagnostic.sourceOffset(
                    file.source, diagnosticEndLine, diagnosticEndColumn);
            int lineStart = absoluteOffset;
            while (lineStart > 0 && file.source.charAt(lineStart - 1) != '\n') lineStart -= 1;
            SymbolEntry diagnosticFileEntry = new SymbolEntry("diagnostic",
                    diagnosticSymbol.isEmpty() ? file.diskFile.getName() : diagnosticSymbol,
                    "Diagnostics", "", file, file.path, file.source.substring(lineStart),
                    lineStart, file.source.length());
            showSymbol(diagnosticFileEntry);
            sourceEditor.setSelection(Math.max(0, absoluteOffset - lineStart),
                    Math.max(absoluteOffset - lineStart, absoluteEnd - lineStart));
            manualEditBody.setVisibility(View.VISIBLE);
            setStatusText("Opened diagnostic file " + diagnosticFile
                    + (diagnosticLine > 0 ? ":" + diagnosticLine : ""));
            return;
        }
        setStatusText("Diagnostic file is available but its symbol could not be parsed");
    }

    private void undoSelectedFailedApply() {
        try {
            AndroidEditRecoveryStore.Entry entry = selectedRecoveryEntry;
            if (entry == null || !entry.file.isFile()) {
                entry = AndroidEditRecoveryStore.latest(this, activeRecoveryProjectId());
            }
            if (entry == null) {
                setStatusText("No failed manual apply is available to undo");
                return;
            }
            File target = new File(projectRoot(), entry.path.replace('/', File.separatorChar));
            relativeProjectPath(target);
            String current = readTextFile(target);
            if (!current.equals(entry.failedSource)) {
                setStatusText("Undo blocked: source changed after the failed apply");
                return;
            }
            writeTextFile(target, entry.beforeSource);
            AndroidEditRecoveryStore.consume(entry);
            selectedRecoveryEntry = null;
            ProjectSnapshot restored = loadBundledProject();
            rebuildSymbolList(restored);
            diagnosticFile = entry.path;
            diagnosticSymbol = entry.symbol;
            diagnosticLine = 0;
            diagnosticColumn = 0;
            diagnosticEndLine = 0;
            diagnosticEndColumn = 0;
            goToDiagnosticSource();
            refreshChangeSummary(restored);
            String compileResult = nativeCompileProject(projectRootPath());
            lastCompileResult = compileResult;
            compileReady = isRunnableCompile(compileResult);
            compileAttempted = true;
            diagnosticStatus.setText("Recovered failed apply\nfile=" + entry.path + "\n" + compileResult);
            setStatusText("Failed manual apply restored safely - " + compileResult);
        } catch (Exception error) {
            setStatusText("Undo failed: " + error.getMessage());
        }
    }

    private void persistSelectedEdit(SymbolEntry symbol, String editedSource) throws IOException {
        SourceFile sourceFile = symbol.sourceFile;
        String before = sourceFile.source.substring(0, symbol.start);
        String after = sourceFile.source.substring(symbol.end);
        sourceFile.source = before + editedSource + after;
        symbol.source = editedSource;
        symbol.end = symbol.start + editedSource.length();
        writeTextFile(sourceFile.diskFile, sourceFile.source);
    }

    private boolean refreshChangeSummary(ProjectSnapshot currentProject) {
        requestGitHubAutoSync();
        if (changeSummary == null) {
            return false;
        }
        try {
            ProjectSnapshot baseline = loadProjectBaselineSnapshot();
            changeSummary.setText(formatChangeSummary(baseline, currentProject));
            return !sourcesByFile(baseline).equals(sourcesByFile(currentProject));
        } catch (IOException error) {
            changeSummary.setText("Changed symbols:\n  Unable to read project baseline: " + error.getMessage());
            return false;
        }
    }

    private void showRawDiffReview() {
        if (changeSummary == null) {
            return;
        }
        try {
            ProjectSnapshot baseline = loadProjectBaselineSnapshot();
            ProjectSnapshot current = loadBundledProject();
            changeSummary.setText(formatRawFileDiffs(baseline, current));
            if (diagnosticBody != null) diagnosticBody.setVisibility(View.VISIBLE);
            if (!sourcesByFile(baseline).equals(sourcesByFile(current))) {
                recordOnboardingTrackedChangeStep(
                        WorkshopOnboardingPolicy.Step.CHANGES_REVIEWED, current);
            }
        } catch (IOException error) {
            changeSummary.setText("Raw file diffs:\n  Unable to read project baseline: " + error.getMessage());
        }
    }

    private ProjectSnapshot loadBundledAssetSnapshot() throws IOException {
        List<SourceFile> files = new ArrayList<>();
        AssetManager assets = getAssets();
        File projectRoot = projectRoot();
        WorkshopTemplateCatalog.Template template = activeWorkshopTemplate();
        for (String file : template.sourceFiles) {
            File diskFile = new File(projectRoot, file);
            try {
                files.add(new SourceFile(file, diskFile, readAsset(assets, template.assetRoot + file)));
            } catch (IOException error) {
                files.add(new SourceFile(file, diskFile, "// Unable to load " + file + ": " + error.getMessage()));
            }
        }
        for (String file : template.testFiles) {
            File diskFile = new File(projectRoot, file);
            try {
                files.add(new SourceFile(file, diskFile, readAsset(assets, template.assetRoot + file)));
            } catch (IOException error) {
                files.add(new SourceFile(file, diskFile, "// Unable to load " + file + ": " + error.getMessage()));
            }
        }

        return ProjectSnapshot.from(files);
    }

    private ProjectSnapshot enrichCanonicalSymbolIds(ProjectSnapshot project, String sourceRoot) {
        try {
            JSONArray items = new JSONObject(nativeSourceItems(sourceRoot))
                    .optJSONArray("items");
            if (items == null) return project;
            for (SymbolSection section : project.sections) {
                for (SymbolGroup group : section.groups) {
                    for (SymbolEntry symbol : group.symbols) {
                        for (int index = 0; index < items.length(); index += 1) {
                            JSONObject item = items.getJSONObject(index);
                            JSONArray spans = item.optJSONArray("source_spans");
                            int[] spanStarts = new int[spans == null ? 0 : spans.length()];
                            int[] spanEnds = new int[spanStarts.length];
                            for (int spanIndex = 0; spanIndex < spanStarts.length; spanIndex += 1) {
                                JSONObject span = spans.getJSONObject(spanIndex);
                                spanStarts[spanIndex] = span.optInt("start", -1);
                                spanEnds[spanIndex] = span.optInt("end", -1);
                            }
                            if (CanonicalSymbolIdentity.matchesRustItem(
                                    symbol.kind, symbol.file, symbol.name, symbol.signature,
                                    symbol.start, symbol.end,
                                    item.optString("kind"), item.optString("file"),
                                    item.optString("name"), item.optString("signature"),
                                    spanStarts, spanEnds)) {
                                symbol.canonicalSymbolId = item.optString("symbol_id", "");
                                break;
                            }
                        }
                    }
                }
            }
        } catch (Exception ignored) {
            // Rust semantic lookup remains authoritative when a symbol is edited.
        }
        return project;
    }

    private WorkshopTemplateCatalog.Template activeWorkshopTemplate() throws IOException {
        String templateId = activeProject == null
                ? WorkshopTemplateCatalog.DEFAULT_TEMPLATE_ID : activeProject.templateId;
        try {
            return WorkshopTemplateCatalog.require(templateId);
        } catch (IllegalArgumentException error) {
            throw new IOException("active project template is unavailable: " + templateId, error);
        }
    }

    private File activeProjectBaselineRoot() {
        String identity = activeProject == null
                ? Integer.toHexString(projectRootPath().hashCode()) : activeProject.id;
        return new File(new File(getFilesDir(), PROJECT_BASELINES_DIR), identity);
    }

    private void ensureActiveProjectBaseline(ProjectSnapshot current) throws IOException {
        File baselineRoot = activeProjectBaselineRoot();
        File readyFile = new File(baselineRoot, PROJECT_BASELINE_READY);
        String templateId = activeProject == null ? WorkshopTemplateCatalog.DEFAULT_TEMPLATE_ID
                : activeProject.templateId;
        String expectedReady = "format=3\ntemplate_id=" + templateId
                + "\nrenderer=gfx_cmd\nrenderer_schema=8\n";
        boolean readyExists = readyFile.isFile();
        boolean readyMatches = readyExists && expectedReady.equals(readTextFile(readyFile));
        WorkshopProjectBaselinePolicy.Action action = WorkshopProjectBaselinePolicy.requiredAction(
                activeProject != null && "import".equals(activeProject.origin), readyExists, readyMatches);
        if (action == WorkshopProjectBaselinePolicy.Action.KEEP) return;
        if (action == WorkshopProjectBaselinePolicy.Action.UPDATE_MARKER) {
            writeTextFile(readyFile, expectedReady);
            return;
        }
        ProjectSnapshot baseline = activeProject != null && "import".equals(activeProject.origin)
                ? current : loadBundledAssetSnapshot();
        deleteBaselineDirectory(baselineRoot);
        if (!baselineRoot.isDirectory() && !baselineRoot.mkdirs()) {
            throw new IOException("unable to create project baseline directory");
        }
        String canonicalRoot = baselineRoot.getCanonicalPath();
        for (SourceFile source : baseline.files) {
            File target = new File(baselineRoot, source.path.replace('/', File.separatorChar));
            String canonicalTarget = target.getCanonicalPath();
            if (!canonicalTarget.startsWith(canonicalRoot + File.separator)) {
                throw new IOException("baseline path escaped baseline root");
            }
            File parent = target.getParentFile();
            if (!parent.isDirectory() && !parent.mkdirs()) throw new IOException("unable to create baseline source directory");
            writeTextFile(target, source.source);
        }
        writeTextFile(readyFile, expectedReady);
    }

    private ProjectSnapshot loadProjectBaselineSnapshot() throws IOException {
        ensureActiveProjectBaseline(loadBundledProject());
        File baselineRoot = activeProjectBaselineRoot();
        List<SourceFile> files = new ArrayList<>();
        collectBaselineStasisFiles(baselineRoot, baselineRoot, files);
        return enrichCanonicalSymbolIds(ProjectSnapshot.from(files), baselineRoot.getAbsolutePath());
    }

    private void collectBaselineStasisFiles(File baselineRoot, File file, List<SourceFile> files) throws IOException {
        if (!file.exists()) return;
        if (file.isDirectory()) {
            File[] children = file.listFiles();
            if (children == null) throw new IOException("unable to list project baseline");
            for (File child : children) collectBaselineStasisFiles(baselineRoot, child, files);
            return;
        }
        if (!file.getName().endsWith(".stasis")) return;
        String rootPath = baselineRoot.getCanonicalPath();
        String filePath = file.getCanonicalPath();
        if (!filePath.startsWith(rootPath + File.separator)) throw new IOException("baseline source escaped baseline root");
        String relative = filePath.substring(rootPath.length() + 1).replace(File.separatorChar, '/');
        files.add(new SourceFile(relative, new File(projectRoot(), relative.replace('/', File.separatorChar)), readTextFile(file)));
    }

    private void deleteBaselineDirectory(File file) {
        if (!file.exists()) return;
        if (file.isDirectory()) {
            File[] children = file.listFiles();
            if (children != null) for (File child : children) deleteBaselineDirectory(child);
        }
        file.delete();
    }

    private static String formatChangeSummary(ProjectSnapshot baseline, ProjectSnapshot current) {
        Map<String, SymbolEntry> baselineSymbols = symbolsByIdentity(baseline);
        Map<String, SymbolEntry> currentSymbols = symbolsByIdentity(current);
        TreeSet<String> changedFiles = new TreeSet<>();
        Map<String, List<String>> changedByGroup = new LinkedHashMap<>();
        for (SymbolEntry symbol : currentSymbols.values()) {
            SymbolEntry before = baselineSymbols.get(symbol.identityKey());
            String change = null;
            if (before == null) {
                change = "added";
            } else if (!before.source.equals(symbol.source)) {
                change = "modified";
            }
            if (change != null) {
                addChangedSymbol(changedByGroup, symbol.owner, change + " " + symbol.displayName());
                changedFiles.add(symbol.file);
            }
        }
        for (SymbolEntry symbol : baselineSymbols.values()) {
            if (!currentSymbols.containsKey(symbol.identityKey())) {
                addChangedSymbol(changedByGroup, symbol.owner, "removed " + symbol.displayName());
                changedFiles.add(symbol.file);
            }
        }

        StringBuilder builder = new StringBuilder();
        builder.append("Changed symbols:");
        if (changedByGroup.isEmpty()) {
            builder.append(" none");
        } else {
            for (Map.Entry<String, List<String>> group : changedByGroup.entrySet()) {
                builder.append('\n').append("  ").append(group.getKey());
                for (String line : group.getValue()) {
                    builder.append('\n').append("    ").append(line);
                }
            }
        }
        builder.append('\n').append("Changed files:");
        if (changedFiles.isEmpty()) {
            builder.append(" none");
        } else {
            for (String file : changedFiles) {
                builder.append('\n').append("  ").append(file);
            }
        }
        return builder.toString();
    }

    private static String formatRawFileDiffs(ProjectSnapshot baseline, ProjectSnapshot current) {
        Map<String, String> baselineFiles = sourcesByFile(baseline);
        Map<String, String> currentFiles = sourcesByFile(current);
        TreeSet<String> files = new TreeSet<>();
        files.addAll(baselineFiles.keySet());
        files.addAll(currentFiles.keySet());

        StringBuilder builder = new StringBuilder("Raw file diffs:");
        boolean found = false;
        for (String file : files) {
            String before = baselineFiles.containsKey(file) ? baselineFiles.get(file) : "";
            String after = currentFiles.containsKey(file) ? currentFiles.get(file) : "";
            if (before.equals(after)) {
                continue;
            }
            found = true;
            appendUnifiedFileDiff(builder, file, before, after);
        }
        if (!found) {
            builder.append(" none");
        }
        return builder.toString();
    }

    private static Map<String, String> sourcesByFile(ProjectSnapshot project) {
        Map<String, String> sources = new LinkedHashMap<>();
        for (SourceFile file : project.files) {
            sources.put(file.path, file.source);
        }
        return sources;
    }

    private static void appendUnifiedFileDiff(StringBuilder builder, String file, String before, String after) {
        String[] beforeLines = splitSourceLines(before);
        String[] afterLines = splitSourceLines(after);
        int prefix = 0;
        while (prefix < beforeLines.length && prefix < afterLines.length
                && beforeLines[prefix].equals(afterLines[prefix])) {
            prefix += 1;
        }
        int beforeEnd = beforeLines.length;
        int afterEnd = afterLines.length;
        while (beforeEnd > prefix && afterEnd > prefix
                && beforeLines[beforeEnd - 1].equals(afterLines[afterEnd - 1])) {
            beforeEnd -= 1;
            afterEnd -= 1;
        }

        builder.append("\n\ndiff --stasis ").append(file);
        builder.append("\n--- a/").append(file);
        builder.append("\n+++ b/").append(file);
        builder.append("\n@@ -").append(prefix + 1).append(',').append(beforeEnd - prefix);
        builder.append(" +").append(prefix + 1).append(',').append(afterEnd - prefix).append(" @@");
        for (int index = prefix; index < beforeEnd; index += 1) {
            builder.append('\n').append('-').append(beforeLines[index]);
        }
        for (int index = prefix; index < afterEnd; index += 1) {
            builder.append('\n').append('+').append(afterLines[index]);
        }
    }

    private static String[] splitSourceLines(String source) {
        return source.isEmpty() ? new String[0] : source.split("\\n", -1);
    }

    private static void addChangedSymbol(Map<String, List<String>> changedByGroup, String group, String line) {
        List<String> lines = changedByGroup.get(group);
        if (lines == null) {
            lines = new ArrayList<>();
            changedByGroup.put(group, lines);
        }
        lines.add(line);
    }

    private static Map<String, SymbolEntry> symbolsByIdentity(ProjectSnapshot project) {
        Map<String, SymbolEntry> symbols = new LinkedHashMap<>();
        for (SymbolSection section : project.sections) {
            for (SymbolGroup group : section.groups) {
                for (SymbolEntry symbol : group.symbols) {
                    symbols.put(symbol.identityKey(), symbol);
                }
            }
        }
        return symbols;
    }
    private void resetProjectFiles() {
        try {
            boolean imported = activeProject != null && "import".equals(activeProject.origin);
            ProjectSnapshot project;
            if (imported) {
                restoreImportedProjectSourceBaseline();
                project = loadBundledProject();
            } else {
                project = loadBundledProject(true);
            }
            rebuildSymbolList(project);
            if (project.firstSymbol != null) showSymbol(project.firstSymbol);
            refreshChangeSummary(project);
            compileReady = false;
            compileAttempted = false;
            setStatusText(imported ? "Reset imported project source baseline" : "Reset project from bundled sample");
        } catch (IOException error) {
            setStatusText("Reset project failed: " + error.getMessage());
        }
    }

    private void restoreImportedProjectSourceBaseline() throws IOException {
        ProjectSnapshot baseline = loadProjectBaselineSnapshot();
        ProjectSnapshot current = loadBundledProject();
        for (SourceFile source : current.files) {
            if (source.diskFile.isFile() && !source.diskFile.delete()) {
                throw new IOException("unable to remove current source " + source.path);
            }
        }
        deleteProjectDirectory(new File(projectRoot(), "build"));
        for (SourceFile source : baseline.files) {
            File target = new File(projectRoot(), source.path.replace('/', File.separatorChar));
            relativeProjectPath(target);
            File parent = target.getParentFile();
            if (!parent.isDirectory() && !parent.mkdirs()) throw new IOException("unable to restore source directory");
            writeTextFile(target, source.source);
        }
    }

    private void createManualTest() {
        try {
            int number = 1;
            File file;
            do {
                file = projectTestFile("tests/manual_test_" + number + ".test.stasis");
                number += 1;
            } while (file.exists());

            String name = "manual test " + (number - 1);
            String source = "import \"../src/main.stasis\";\n\n"
                    + "test `" + name + "`(): bool {\n"
                    + "    return false;\n"
                    + "}\n";
            File parent = file.getParentFile();
            if (parent != null && !parent.isDirectory() && !parent.mkdirs()) {
                throw new IOException("failed to create " + parent.getAbsolutePath());
            }
            writeTextFile(file, source);

            ProjectSnapshot project = loadBundledProject();
            rebuildSymbolList(project);
            SymbolEntry created = findSymbolByIdentity(project, "test", relativeProjectPath(file), "Tests", name);
            if (created != null) {
                showSymbol(created);
            }
            refreshChangeSummary(project);
            setStatusText("Created failing test template; edit it, then Run Tests");
        } catch (IOException error) {
            setStatusText("Create test failed: " + error.getMessage());
        }
    }

    private void deleteSelectedManualTest() {
        if (selectedSymbol == null || !"test".equals(selectedSymbol.kind)) {
            setStatusText("Delete Test unavailable: select a user-created test first");
            return;
        }
        try {
            if (findMatchingSymbol(loadProjectBaselineSnapshot(), selectedSymbol) != null) {
                setStatusText("Delete Test unavailable: baseline tests can be reverted, not deleted");
                return;
            }
            File file = selectedSymbol.sourceFile.diskFile;
            if (!file.delete()) {
                throw new IOException("failed to delete " + file.getAbsolutePath());
            }
            ProjectSnapshot project = loadBundledProject();
            rebuildSymbolList(project);
            if (project.firstSymbol != null) {
                showSymbol(project.firstSymbol);
            }
            refreshChangeSummary(project);
            setStatusText("Deleted user-created test");
        } catch (IOException error) {
            setStatusText("Delete Test failed: " + error.getMessage());
        }
    }

    private void createManualHelper() {
        try {
            ProjectSnapshot project = loadBundledProject();
            int number = 1;
            String name;
            do {
                name = "manual_helper_" + number;
                number += 1;
            } while (findSymbolByIdentity(project, "function", "src/root.stasis", "Root", name) != null);

            String source = "function " + name + "(): void {\n}\n";
            SourceFile rootFile = findProjectFile(project, "src/root.stasis");
            String originalSource = rootFile.source;
            appendProjectFunction(project, "src/root.stasis", source);
            String compileResult = nativeCompileProject(projectRootPath());
            if (!isRunnableCompile(compileResult)) {
                rootFile.source = originalSource;
                writeTextFile(rootFile.diskFile, originalSource);
                throw new IOException("new helper compile failed: " + compileResult);
            }

            lastCompileResult = compileResult;
            compileReady = true;
            compileAttempted = true;
            ProjectSnapshot refreshedProject = loadBundledProject();
            rebuildSymbolList(refreshedProject);
            SymbolEntry created = findSymbolByIdentity(refreshedProject, "function", "src/root.stasis", "Root", name);
            if (created != null) {
                showSymbol(created);
            }
            refreshChangeSummary(refreshedProject);
            setStatusText("Created root helper - " + compileResult);
        } catch (Exception error) {
            setStatusText("Create helper failed: " + error.getMessage());
        }
    }

    private void deleteSelectedManualHelper() {
        if (selectedSymbol == null || !"function".equals(selectedSymbol.kind)
                || !"Root".equals(selectedSymbol.owner) || !"src/root.stasis".equals(selectedSymbol.file)) {
            setStatusText("Delete Helper unavailable: select a user-created root helper first");
            return;
        }
        try {
            if (findMatchingSymbol(loadProjectBaselineSnapshot(), selectedSymbol) != null) {
                setStatusText("Delete Helper unavailable: baseline helpers can be reverted, not deleted");
                return;
            }
            SourceFile sourceFile = selectedSymbol.sourceFile;
            String originalSource = sourceFile.source;
            sourceFile.source = originalSource.substring(0, selectedSymbol.start)
                    + originalSource.substring(selectedSymbol.end);
            writeTextFile(sourceFile.diskFile, sourceFile.source);
            String compileResult = nativeCompileProject(projectRootPath());
            if (!isRunnableCompile(compileResult)) {
                sourceFile.source = originalSource;
                writeTextFile(sourceFile.diskFile, originalSource);
                throw new IOException("delete helper compile failed: " + compileResult);
            }

            lastCompileResult = compileResult;
            compileReady = true;
            compileAttempted = true;
            ProjectSnapshot project = loadBundledProject();
            rebuildSymbolList(project);
            if (project.firstSymbol != null) {
                showSymbol(project.firstSymbol);
            }
            refreshChangeSummary(project);
            setStatusText("Deleted user-created root helper - " + compileResult);
        } catch (IOException error) {
            setStatusText("Delete Helper failed: " + error.getMessage());
        }
    }

    private void resetSelectedEdit() {
        if (selectedSymbol == null) {
            return;
        }

        sourceEditor.setText(selectedSymbol.source.trim());
        clearPendingDraft();
        setStatusText("Reset editor to selected symbol");
    }

    private void revertSelectedToBundled() {
        if (selectedSymbol == null) {
            setStatusText("Revert unavailable: select a baseline symbol first");
            return;
        }
        try {
            SymbolEntry baseline = findMatchingSymbol(loadProjectBaselineSnapshot(), selectedSymbol);
            if (baseline == null) {
                setStatusText("Revert unavailable: selected symbol is not in the project baseline");
                return;
            }
            boolean sourceChanged = !selectedSymbol.source.trim().equals(baseline.source.trim());
            String revertedChangeId = selectedSymbol.identityKey();
            String revertedChangeHash = onboardingSourceHash(selectedSymbol.source);
            persistSelectedEdit(selectedSymbol, baseline.source);
            clearPendingDraft();
            ProjectSnapshot refreshedProject = loadBundledProject();
            rebuildSymbolList(refreshedProject);
            SymbolEntry refreshedSymbol = findMatchingSymbol(refreshedProject, baseline);
            if (refreshedSymbol != null) {
                showSymbol(refreshedSymbol);
            }
            refreshChangeSummary(refreshedProject);
            String compileResult = nativeCompileProject(projectRootPath());
            lastCompileResult = compileResult;
            compileReady = isRunnableCompile(compileResult);
            compileAttempted = true;
            if (compileReady && sourceChanged) {
                recordOnboardingRevert(revertedChangeId, revertedChangeHash);
            }
            setStatusText("Reverted saved symbol to project baseline - " + compileResult);
        } catch (IOException error) {
            setStatusText("Revert failed: " + error.getMessage());
        }
    }

    private String classifySelectedReload(SymbolEntry symbol, String editedSource) {
        if ("test".equals(symbol.kind)) {
            return "TestUpdated: run tests to validate";
        }
        if (!"function".equals(symbol.kind)) {
            return "ResetRequired: struct or layout source changed";
        }

        String editedSignature = functionSignature(editedSource);
        if (symbol.signature.equals(editedSignature)) {
            return "FastReload: function signature unchanged";
        }
        return "ResetRequired: function signature changed";
    }

    private static String functionSignature(String source) {
        String trimmed = source.trim();
        if (!trimmed.startsWith("function ")) {
            return "";
        }

        int bodyStart = trimmed.indexOf('{');
        if (bodyStart < 0) {
            return "";
        }
        return trimmed.substring("function ".length(), bodyStart).trim();
    }

    File projectRoot() {
        return projectRootFile;
    }

    private String relativeProjectPath(File file) throws IOException {
        String root = projectRoot().getCanonicalPath();
        String path = file.getCanonicalPath();
        if (!path.equals(root) && !path.startsWith(root + File.separator)) {
            throw new IOException("path is outside project root: " + path);
        }
        if (path.equals(root)) {
            return "";
        }
        return path.substring(root.length() + 1).replace(File.separatorChar, '/');
    }

    private File projectTestFile(String path) throws IOException {
        String normalized = path == null ? "" : path.replace('\\', '/').trim();
        if (!normalized.startsWith("tests/") || normalized.contains("..")) {
            throw new IOException("Test files must live under tests/: " + normalized);
        }
        if (!normalized.endsWith(".test.stasis")) {
            throw new IOException("Test files must end with .test.stasis: " + normalized);
        }
        File file = new File(projectRoot(), normalized.replace('/', File.separatorChar));
        relativeProjectPath(file);
        return file;
    }



    private void collectProjectStasisFiles(File file, List<SourceFile> out, TreeSet<String> seen) throws IOException {
        if (!file.exists()) {
            return;
        }
        if (file.isDirectory()) {
            if (!file.equals(projectRoot()) && "build".equals(file.getName())) return;
            File[] children = file.listFiles();
            if (children != null) {
                Arrays.sort(children, new Comparator<File>() {
                    @Override public int compare(File left, File right) {
                        return left.getName().compareTo(right.getName());
                    }
                });
                for (File child : children) {
                    collectProjectStasisFiles(child, out, seen);
                }
            }
            return;
        }
        String path = relativeProjectPath(file);
        if (path.endsWith(".stasis") && !seen.contains(path)) {
            seen.add(path);
            out.add(new SourceFile(path, file, readTextFile(file)));
        }
    }
    String projectRootPath() {
        return projectRootPath;
    }
    private ProjectSnapshot loadBundledProject() {
        return loadBundledProject(false);
    }

    private ProjectSnapshot loadBundledProject(boolean resetProject) {
        List<SourceFile> files = new ArrayList<>();
        AssetManager assets = getAssets();
        File projectRoot = projectRoot();
        if (resetProject) {
            deleteProjectDirectory(projectRoot);
        }

        boolean sampleProject = activeProject == null || "sample".equals(activeProject.origin);
        if (sampleProject) {
            try {
                WorkshopTemplateCatalog.Template template = activeWorkshopTemplate();
                materializeTemplateProject(assets, template, projectRoot, true);
                refreshCompilerOwnedLibrary(assets, template, projectRoot);
            } catch (IOException ignored) {
                // Registry validation normally prevents an unknown template from reaching this path.
            }
        }
        try {
            TreeSet<String> seen = new TreeSet<>();
            collectProjectStasisFiles(projectRoot, files, seen);
        } catch (IOException ignored) {
            // One unreadable project file should not crash the workshop surface.
        }

        return enrichCanonicalSymbolIds(ProjectSnapshot.from(files), projectRootPath());
    }

    private void materializeTemplateProject(AssetManager assets,
            WorkshopTemplateCatalog.Template template, File root, boolean bestEffort)
            throws IOException {
        for (String file : template.sourceFiles) {
            materializeTemplateFile(assets, template.assetRoot + file, new File(root, file),
                    template.replaceExistingFiles, bestEffort);
        }
        for (String file : template.testFiles) {
            materializeTemplateFile(assets, template.assetRoot + file, new File(root, file),
                    template.replaceExistingFiles, bestEffort);
        }
        for (WorkshopTemplateCatalog.DirectoryMount mount : template.directoryMounts) {
            try {
                ensureProjectDirectory(assets, mount.assetDirectory,
                        new File(root, mount.projectDirectory), mount.replaceExisting);
            } catch (IOException error) {
                if (!bestEffort) throw error;
            }
        }
        for (String file : template.auxiliaryFiles) {
            materializeTemplateFile(assets, template.assetRoot + file, new File(root, file),
                    template.replaceExistingFiles, bestEffort);
        }
    }

    private void refreshCompilerOwnedLibrary(AssetManager assets,
            WorkshopTemplateCatalog.Template template, File root) throws IOException {
        for (WorkshopTemplateCatalog.DirectoryMount mount : template.directoryMounts) {
            if (!"stasis_stdlib".equals(mount.assetDirectory)) continue;
            for (String relativePath : WorkshopCompilerOwnedLibrary.refreshedFiles()) {
                ensureProjectFile(assets, mount.assetDirectory + "/" + relativePath,
                        new File(root, mount.projectDirectory + "/" + relativePath), true);
            }
        }
    }

    private boolean migrateUnmodifiedBundledTemplateSources() throws IOException {
        if (activeProject == null || !"sample".equals(activeProject.origin)) return false;
        File baselineRoot = activeProjectBaselineRoot();
        if (!new File(baselineRoot, PROJECT_BASELINE_READY).isFile()) return false;

        WorkshopTemplateCatalog.Template template = activeWorkshopTemplate();
        ArrayList<String> paths = new ArrayList<>();
        paths.addAll(Arrays.asList(template.sourceFiles));
        paths.addAll(Arrays.asList(template.testFiles));
        boolean changed = false;
        for (String path : paths) {
            File projectFile = new File(projectRoot(), path.replace('/', File.separatorChar));
            File baselineFile = new File(baselineRoot, path.replace('/', File.separatorChar));
            if (!projectFile.isFile() || !baselineFile.isFile()) continue;
            if (!WorkshopBundledSourceUpgrade.shouldReplace(
                    readTextFile(projectFile), readTextFile(baselineFile))) continue;
            String packaged = readAsset(getAssets(), template.assetRoot + path);
            if (packaged.equals(readTextFile(projectFile))) continue;
            writeTextFile(projectFile, packaged);
            changed = true;
        }
        return changed;
    }

    private ProjectSnapshot loadAndMigrateActiveBundledProject() throws IOException {
        ProjectSnapshot project = loadBundledProject();
        if (migrateUnmodifiedBundledTemplateSources()) project = loadBundledProject();
        if (migrateBundledPongBallSpeed()) project = loadBundledProject();
        if (migrateBundledPongProductionRenderer()) project = loadBundledProject();
        return project;
    }

    private void materializeTemplateFile(AssetManager assets, String assetPath, File file,
            boolean replaceExisting, boolean bestEffort) throws IOException {
        try {
            ensureProjectFile(assets, assetPath, file, replaceExisting);
        } catch (IOException error) {
            if (!bestEffort) throw error;
        }
    }

    private boolean migrateBundledPongBallSpeed() throws IOException {
        if (activeProject == null || !"sample".equals(activeProject.origin)
                || !WorkshopTemplateCatalog.LEGACY_TEMPLATE_ID.equals(activeProject.templateId)) {
            return false;
        }
        SharedPreferences preferences = getSharedPreferences(SAMPLE_MIGRATION_PREFS, MODE_PRIVATE);
        String key = activeProject.id + ":" + PONG_SLOW_BALL_MIGRATION;
        if (preferences.getBoolean(key, false)) return false;

        File sourceFile = new File(projectRoot(), "src/main.stasis");
        if (!sourceFile.isFile()) return false;
        String before = readTextFile(sourceFile);
        String after = before
                .replace("GameState.ball_vx = 5;", "GameState.ball_vx = 3;")
                .replace("GameState.ball_vx = -5;", "GameState.ball_vx = -3;")
                .replace("GameState.ball_vy = 4;", "GameState.ball_vy = 3;");
        if (!after.equals(before)) writeTextFile(sourceFile, after);
        if (!preferences.edit().putBoolean(key, true).commit()) {
            throw new IOException("unable to record bundled Pong speed migration");
        }
        return !after.equals(before);
    }

    private boolean migrateBundledPongProductionRenderer() throws IOException {
        if (activeProject == null || !"bundled-workshop".equals(activeProject.id)
                || !"sample".equals(activeProject.origin)
                || !WorkshopTemplateCatalog.LEGACY_TEMPLATE_ID.equals(activeProject.templateId)) {
            return false;
        }
        SharedPreferences preferences = getSharedPreferences(SAMPLE_MIGRATION_PREFS, MODE_PRIVATE);
        String key = activeProject.id + ":" + PONG_GFX_CMD_MIGRATION;
        if (preferences.getBoolean(key, false)) return false;

        File sourceFile = new File(projectRoot(), "src/main.stasis");
        if (!sourceFile.isFile()) return false;
        String before = readTextFile(sourceFile);
        String after = before;
        boolean sourceChanged = !WorkshopPongRendererMigration.isProductionSource(before);
        if (sourceChanged) {
            try {
                after = WorkshopPongRendererMigration.migrateSource(before);
            } catch (IllegalArgumentException error) {
                throw new IOException("bundled Pong lifecycle could not be migrated safely", error);
            }
        }
        File adapterFile = new File(projectRoot(), "src/preview_adapter.stasis");
        boolean adapterExisted = adapterFile.isFile();
        String previousAdapter = adapterExisted ? readTextFile(adapterFile) : "";
        String packagedAdapter = readAsset(getAssets(),
                "workshop_sample/src/preview_adapter.stasis");
        boolean adapterChanged = !packagedAdapter.equals(previousAdapter);
        File manifestFile = new File(projectRoot(), "assets/manifest.json");
        boolean manifestExisted = manifestFile.isFile();
        String previousManifest = manifestExisted ? readTextFile(manifestFile) : "";
        String packagedManifest = readAsset(getAssets(),
                "workshop_sample/assets/manifest.json");
        String migratedManifest;
        try {
            migratedManifest = manifestExisted
                    ? WorkshopPongAssetManifestMigration.mergeRequiredSprites(
                            previousManifest, packagedManifest)
                    : packagedManifest;
        } catch (org.json.JSONException error) {
            throw new IOException("bundled Pong asset manifest could not be migrated safely", error);
        }
        boolean manifestChanged = !migratedManifest.equals(previousManifest);
        if (!sourceChanged && !adapterChanged && !manifestChanged) {
            if (!preferences.edit().putBoolean(key, true).commit()) {
                throw new IOException("unable to record bundled Pong renderer migration");
            }
            return false;
        }

        File backup = new File(projectRoot(),
                "build/migrations/pong_gfx_cmd_v6/main.stasis");
        File adapterBackup = new File(projectRoot(),
                "build/migrations/pong_gfx_cmd_v6/preview_adapter.stasis");
        File manifestBackup = new File(projectRoot(),
                "build/migrations/pong_gfx_cmd_v6/manifest.json");
        if (sourceChanged && !backup.isFile()) writeSyncedTextFile(backup, before);
        if (adapterChanged && adapterExisted && !adapterBackup.isFile()) {
            writeSyncedTextFile(adapterBackup, previousAdapter);
        }
        if (manifestChanged && manifestExisted && !manifestBackup.isFile()) {
            writeSyncedTextFile(manifestBackup, previousManifest);
        }
        try {
            if (sourceChanged) replaceTextFileAtomically(sourceFile, after);
            if (adapterChanged) replaceTextFileAtomically(adapterFile, packagedAdapter);
            if (manifestChanged) replaceTextFileAtomically(manifestFile, migratedManifest);
            if (!preferences.edit().putBoolean(key, true).commit()) {
                throw new IOException("unable to record bundled Pong renderer migration");
            }
        } catch (IOException error) {
            try {
                if (sourceChanged) replaceTextFileAtomically(sourceFile, before);
                if (adapterChanged) {
                    if (adapterExisted) replaceTextFileAtomically(adapterFile, previousAdapter);
                    else if (!adapterFile.delete() && adapterFile.exists()) {
                        throw new IOException("unable to remove migrated Pong renderer adapter");
                    }
                }
                if (manifestChanged) {
                    if (manifestExisted) replaceTextFileAtomically(manifestFile, previousManifest);
                    else if (!manifestFile.delete() && manifestFile.exists()) {
                        throw new IOException("unable to remove migrated Pong asset manifest");
                    }
                }
            } catch (IOException rollback) {
                error.addSuppressed(rollback);
            }
            throw error;
        }
        return true;
    }

    private void ensureProjectFile(AssetManager assets, String assetPath, File diskFile) throws IOException {
        ensureProjectFile(assets, assetPath, diskFile, false);
    }

    private void ensureProjectFile(AssetManager assets, String assetPath, File diskFile,
            boolean replaceExisting) throws IOException {
        if (!replaceExisting && diskFile.isFile()) {
            return;
        }
        File parent = diskFile.getParentFile();
        if (parent != null && !parent.isDirectory() && !parent.mkdirs()) {
            throw new IOException("failed to create " + parent.getAbsolutePath());
        }
        File temporary = new File(parent, diskFile.getName() + ".asset.tmp");
        try (InputStream input = assets.open(assetPath);
                FileOutputStream output = new FileOutputStream(temporary, false)) {
            byte[] buffer = new byte[8192];
            int count;
            while ((count = input.read(buffer)) != -1) output.write(buffer, 0, count);
            output.getFD().sync();
        }
        try {
            try {
                Files.move(temporary.toPath(), diskFile.toPath(),
                        StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING);
            } catch (AtomicMoveNotSupportedException unsupported) {
                Files.move(temporary.toPath(), diskFile.toPath(), StandardCopyOption.REPLACE_EXISTING);
            }
        } finally {
            if (temporary.exists()) temporary.delete();
        }
    }

    private void ensureProjectDirectory(AssetManager assets, String assetPath, File diskDirectory)
            throws IOException {
        ensureProjectDirectory(assets, assetPath, diskDirectory, false);
    }

    private void ensureProjectDirectory(AssetManager assets, String assetPath, File diskDirectory,
            boolean replaceExisting) throws IOException {
        if (diskDirectory.exists() && !diskDirectory.isDirectory()) {
            throw new IOException("project directory path is a file: "
                    + diskDirectory.getAbsolutePath());
        }
        if (!diskDirectory.isDirectory() && !diskDirectory.mkdirs()) {
            throw new IOException("failed to create " + diskDirectory.getAbsolutePath());
        }
        String[] children = assets.list(assetPath);
        if (children == null || children.length == 0) {
            throw new IOException("packaged asset directory is empty: " + assetPath);
        }
        Arrays.sort(children);
        for (String child : children) {
            String childAssetPath = assetPath + "/" + child;
            File childDiskPath = new File(diskDirectory, child);
            String[] grandchildren = assets.list(childAssetPath);
            if (grandchildren != null && grandchildren.length > 0) {
                ensureProjectDirectory(assets, childAssetPath, childDiskPath, replaceExisting);
            } else {
                ensureProjectFile(assets, childAssetPath, childDiskPath, replaceExisting);
            }
        }
    }

    private void deleteProjectDirectory(File file) {
        if (!file.exists()) {
            return;
        }
        if (WorkshopProjectRegistry.METADATA_FILE.equals(file.getName())) {
            return;
        }
        if (file.isDirectory()) {
            File[] children = file.listFiles();
            if (children != null) {
                for (File child : children) {
                    deleteProjectDirectory(child);
                }
            }
        }
        if (file.equals(projectRoot())) {
            return;
        }
        if (!file.delete() && file.exists()) {
            setStatusText("Unable to refresh bundled project file: " + file.getAbsolutePath());
        }
    }

    private String readTextFile(File file) throws IOException {
        FileInputStream input = new FileInputStream(file);
        try {
            return readStream(input);
        } finally {
            input.close();
        }
    }

    private void writeTextFile(File file, String source) throws IOException {
        File parent = file.getParentFile();
        if (parent != null && !parent.isDirectory() && !parent.mkdirs()) {
            throw new IOException("failed to create " + parent.getAbsolutePath());
        }

        FileOutputStream output = new FileOutputStream(file, false);
        try {
            output.write(source.getBytes(StandardCharsets.UTF_8));
        } finally {
            output.close();
        }
    }

    private void writeSyncedTextFile(File file, String source) throws IOException {
        File parent = file.getParentFile();
        if (parent != null && !parent.isDirectory() && !parent.mkdirs()) {
            throw new IOException("failed to create " + parent.getAbsolutePath());
        }
        FileOutputStream output = new FileOutputStream(file, false);
        try {
            output.write(source.getBytes(StandardCharsets.UTF_8));
            output.getFD().sync();
        } finally {
            output.close();
        }
    }

    private void replaceTextFileAtomically(File file, String source) throws IOException {
        File temporary = new File(file.getParentFile(), file.getName() + ".gfx-cmd.tmp");
        writeSyncedTextFile(temporary, source);
        try {
            try {
                Files.move(temporary.toPath(), file.toPath(), StandardCopyOption.ATOMIC_MOVE,
                        StandardCopyOption.REPLACE_EXISTING);
            } catch (AtomicMoveNotSupportedException unsupported) {
                Files.move(temporary.toPath(), file.toPath(), StandardCopyOption.REPLACE_EXISTING);
            }
        } finally {
            if (temporary.exists()) temporary.delete();
        }
    }

    private String readAsset(AssetManager assets, String path) throws IOException {
        InputStream input = assets.open(path);
        try {
            return readStream(input);
        } finally {
            input.close();
        }
    }

    private String readStream(InputStream input) throws IOException {
        return readStreamStatic(input);
    }

    private static String readStreamStatic(InputStream input) throws IOException {
        ByteArrayOutputStream output = new ByteArrayOutputStream();
        byte[] buffer = new byte[4096];
        int read;
        while ((read = input.read(buffer)) != -1) {
            output.write(buffer, 0, read);
        }
        return new String(output.toByteArray(), StandardCharsets.UTF_8);
    }

    private LinearLayout.LayoutParams fullWidth() {
        return new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT);
    }

    private WorkshopAdaptiveLayout.Profile adaptiveLayoutProfile() {
        Configuration configuration = getResources().getConfiguration();
        return WorkshopAdaptiveLayout.profile(configuration.screenWidthDp,
                configuration.screenHeightDp, configuration.fontScale);
    }

    private static void configureActionRow(LinearLayout row,
            WorkshopAdaptiveLayout.Profile layout) {
        row.setOrientation(layout.stackActions ? LinearLayout.VERTICAL : LinearLayout.HORIZONTAL);
    }

    private LinearLayout.LayoutParams actionWidth(WorkshopAdaptiveLayout.Profile layout) {
        return layout.stackActions ? fullWidth() : weightedWidth();
    }

    private static void chainAccessibilityTraversal(View... views) {
        View previous = null;
        for (View view : views) {
            if (view == null) continue;
            if (view.getId() == View.NO_ID) view.setId(View.generateViewId());
            if (previous != null) {
                view.setAccessibilityTraversalAfter(previous.getId());
                previous.setNextFocusForwardId(view.getId());
            }
            previous = view;
        }
    }

    private GradientDrawable createPanelBackground(int fill, int stroke) {
        GradientDrawable drawable = new GradientDrawable();
        drawable.setColor(fill);
        drawable.setStroke(dp(1), stroke);
        drawable.setCornerRadius(dp(6));
        return drawable;
    }

    private StateListDrawable createFocusableControlBackground() {
        GradientDrawable focused = createPanelBackground(
                WorkshopAccessibilityPolicy.DARK_CONTROL,
                WorkshopAccessibilityPolicy.FOCUS_BORDER);
        focused.setStroke(dp(3), WorkshopAccessibilityPolicy.FOCUS_BORDER);
        GradientDrawable pressed = createPanelBackground(Color.rgb(38, 98, 217),
                WorkshopAccessibilityPolicy.FOCUS_BORDER);
        StateListDrawable background = new StateListDrawable();
        background.addState(new int[] {android.R.attr.state_focused}, focused);
        background.addState(new int[] {android.R.attr.state_pressed}, pressed);
        background.addState(new int[] {}, createPanelBackground(
                WorkshopAccessibilityPolicy.DARK_CONTROL,
                WorkshopAccessibilityPolicy.DARK_CONTROL_BORDER));
        return background;
    }

    private static void selectOnly(List<Button> choices, Button selected) {
        for (Button choice : choices) choice.setSelected(choice == selected);
    }

    private int dp(int value) {
        return Math.round(value * getResources().getDisplayMetrics().density);
    }

    private static final class RetainedPaintSession {
        final Bitmap bitmap;
        final String name;
        final int brushColor;
        final float brushSize;
        final boolean erasing;

        RetainedPaintSession(Bitmap bitmap, String name,
                int brushColor, float brushSize, boolean erasing) {
            this.bitmap = bitmap;
            this.name = name;
            this.brushColor = brushColor;
            this.brushSize = brushSize;
            this.erasing = erasing;
        }
    }

    private static final class ProjectSnapshot {
        final List<SourceFile> files;
        final List<SymbolSection> sections;
        final SymbolEntry firstSymbol;
        final int symbolCount;

        private ProjectSnapshot(List<SourceFile> files, List<SymbolSection> sections, SymbolEntry firstSymbol, int symbolCount) {
            this.files = files;
            this.sections = sections;
            this.firstSymbol = firstSymbol;
            this.symbolCount = symbolCount;
        }

        static ProjectSnapshot from(List<SourceFile> files) {
            TreeSet<String> structs = new TreeSet<>();
            for (SourceFile file : files) {
                structs.addAll(parseStructNames(file.source));
            }

            List<SymbolEntry> symbols = new ArrayList<>();
            for (SourceFile file : files) {
                symbols.addAll(parseSymbols(file, structs));
            }

            List<SymbolSection> sections = groupSymbols(symbols);
            SymbolEntry first = symbols.isEmpty() ? null : symbols.get(0);
            return new ProjectSnapshot(files, sections, first, symbols.size());
        }
    }

    private static SymbolEntry findMatchingSymbol(ProjectSnapshot project, SymbolEntry previous) {
        for (SymbolSection section : project.sections) {
            for (SymbolGroup group : section.groups) {
                for (SymbolEntry symbol : group.symbols) {
                    if (sameSymbolIdentity(symbol, previous)) {
                        return symbol;
                    }
                }
            }
        }
        return null;
    }

    private static SymbolEntry findSymbolByIdentity(ProjectSnapshot project, String kind, String file, String owner, String name) {
        for (SymbolSection section : project.sections) {
            for (SymbolGroup group : section.groups) {
                for (SymbolEntry symbol : group.symbols) {
                    if (symbol.kind.equals(kind) && symbol.file.equals(file)
                            && symbol.owner.equals(owner) && symbol.name.equals(name)) {
                        return symbol;
                    }
                }
            }
        }
        return null;
    }

    private static SymbolEntry findSymbolByIdentityKey(ProjectSnapshot project, String identityKey) {
        for (SymbolSection section : project.sections) {
            for (SymbolGroup group : section.groups) {
                for (SymbolEntry symbol : group.symbols) {
                    if (symbol.identityKey().equals(identityKey)) return symbol;
                }
            }
        }
        return null;
    }

    private static boolean sameSymbolIdentity(SymbolEntry left, SymbolEntry right) {
        return CanonicalSymbolIdentity.sameIdentity(
                left.canonicalSymbolId, left.kind, left.file, left.owner, left.name,
                right.canonicalSymbolId, right.kind, right.file, right.owner, right.name);
    }
    private static List<String> parseStructNames(String source) {
        List<String> names = new ArrayList<>();
        int cursor = 0;
        while (true) {
            int structIndex = source.indexOf("struct ", cursor);
            if (structIndex < 0) {
                return names;
            }

            int nameStart = structIndex + "struct ".length();
            int nameEnd = readIdentifierEnd(source, nameStart);
            if (nameEnd > nameStart) {
                names.add(source.substring(nameStart, nameEnd));
            }
            cursor = nameEnd;
        }
    }

    private static List<SymbolEntry> parseSymbols(SourceFile file, TreeSet<String> structs) {
        List<SymbolEntry> symbols = new ArrayList<>();
        int cursor = 0;
        while (cursor < file.source.length()) {
            int nextStruct = file.source.indexOf("struct ", cursor);
            int nextFunction = file.source.indexOf("function ", cursor);
            int nextGlobal = file.source.indexOf("global ", cursor);
            int nextTest = file.source.indexOf("test ", cursor);
            int next = minPositive(minPositive(nextStruct, nextFunction), minPositive(nextGlobal, nextTest));
            if (next < 0) {
                break;
            }

            if (next == nextStruct) {
                SymbolEntry symbol = parseStruct(file, next);
                if (symbol != null) {
                    symbols.add(symbol);
                    cursor = symbol.end;
                } else {
                    cursor = next + "struct ".length();
                }
            } else if (next == nextGlobal) {
                SymbolEntry symbol = parseGlobal(file, next);
                if (symbol != null) {
                    symbols.add(symbol);
                    cursor = symbol.end;
                } else {
                    cursor = next + "global ".length();
                }
            } else if (next == nextTest) {
                SymbolEntry symbol = parseTest(file, next);
                if (symbol != null) {
                    symbols.add(symbol);
                    cursor = symbol.end;
                } else {
                    cursor = next + "test ".length();
                }
            } else {
                SymbolEntry symbol = parseFunction(file, next, structs);
                if (symbol != null) {
                    symbols.add(symbol);
                    cursor = symbol.end;
                } else {
                    cursor = next + "function ".length();
                }
            }
        }
        return symbols;
    }

    private static SymbolEntry parseStruct(SourceFile file, int start) {
        int nameStart = start + "struct ".length();
        int nameEnd = readIdentifierEnd(file.source, nameStart);
        int bodyStart = file.source.indexOf('{', nameEnd);
        int end = findMatchingBrace(file.source, bodyStart);
        if (nameEnd <= nameStart || bodyStart < 0 || end < 0) {
            return null;
        }

        String name = file.source.substring(nameStart, nameEnd);
        String source = file.source.substring(start, end);
        return new SymbolEntry("struct", name, name, "struct " + name, file, file.path, source, start, end);
    }

    private static SymbolEntry parseGlobal(SourceFile file, int start) {
        int nameStart = start + "global ".length();
        int nameEnd = readIdentifierEnd(file.source, nameStart);
        int bodyStart = file.source.indexOf('{', nameEnd);
        int end = findMatchingBrace(file.source, bodyStart);
        if (nameEnd <= nameStart || bodyStart < 0 || end < 0) {
            return null;
        }

        String name = file.source.substring(nameStart, nameEnd);
        String source = file.source.substring(start, end);
        String backingStructSource = "struct " + name + " " + file.source.substring(bodyStart, end);
        return new SymbolEntry("global", name, "Globals", "global " + name, file, file.path, source, start, end, backingStructSource);
    }
    private static SymbolEntry parseFunction(SourceFile file, int start, TreeSet<String> structs) {
        int signatureStart = start + "function ".length();
        int bodyStart = file.source.indexOf('{', signatureStart);
        int end = findMatchingBrace(file.source, bodyStart);
        if (bodyStart < 0 || end < 0) {
            return null;
        }

        String signature = file.source.substring(signatureStart, bodyStart).trim();
        int paren = signature.indexOf('(');
        String name = paren < 0 ? signature : signature.substring(0, paren).trim();
        String owner = ownerForFunction(file.path, name, signature, structs);
        String source = file.source.substring(start, end);
        return new SymbolEntry("function", name, owner, signature, file, file.path, source, start, end);
    }

    private static SymbolEntry parseTest(SourceFile file, int start) {
        int signatureStart = start + "test ".length();
        int bodyStart = file.source.indexOf('{', signatureStart);
        int end = findMatchingBrace(file.source, bodyStart);
        if (bodyStart < 0 || end < 0) {
            return null;
        }

        String signature = file.source.substring(signatureStart, bodyStart).trim();
        int nameStart = signature.indexOf('`');
        int nameEnd = nameStart < 0 ? -1 : signature.indexOf('`', nameStart + 1);
        if (nameStart < 0 || nameEnd <= nameStart + 1) {
            return null;
        }
        String name = signature.substring(nameStart + 1, nameEnd);
        String source = file.source.substring(start, end);
        return new SymbolEntry("test", name, "Tests", "test " + signature, file, file.path, source, start, end);
    }

    private static String ownerForFunction(String file, String name, String signature, TreeSet<String> structs) {
        if (isLifecycle(name)) {
            return "Main";
        }

        String receiver = receiverType(signature);
        if (receiver != null && structs.contains(receiver)) {
            return receiver;
        }

        String firstParameterType = firstParameterType(signature);
        if (firstParameterType != null && structs.contains(firstParameterType)) {
            return firstParameterType;
        }

        if (file.startsWith("src/systems/")) {
            return titleCase(file.substring(file.lastIndexOf('/') + 1, file.lastIndexOf('.')));
        }

        return "Root";
    }

    private static List<SymbolSection> groupSymbols(List<SymbolEntry> symbols) {
        Map<String, Map<String, List<SymbolEntry>>> sections = new LinkedHashMap<>();
        sections.put("Main", new LinkedHashMap<String, List<SymbolEntry>>());
        sections.put("Structs", new LinkedHashMap<String, List<SymbolEntry>>());
        sections.put("Globals", new LinkedHashMap<String, List<SymbolEntry>>());
        sections.put("Systems", new LinkedHashMap<String, List<SymbolEntry>>());
        sections.put("Root", new LinkedHashMap<String, List<SymbolEntry>>());
        sections.put("Tests", new LinkedHashMap<String, List<SymbolEntry>>());

        for (SymbolEntry symbol : symbols) {
            String section = sectionFor(symbol);
            Map<String, List<SymbolEntry>> groups = sections.get(section);
            if (!groups.containsKey(symbol.owner)) {
                groups.put(symbol.owner, new ArrayList<SymbolEntry>());
            }
            groups.get(symbol.owner).add(symbol);
        }

        List<SymbolSection> out = new ArrayList<>();
        for (Map.Entry<String, Map<String, List<SymbolEntry>>> section : sections.entrySet()) {
            if (section.getValue().isEmpty()) {
                continue;
            }

            List<SymbolGroup> groups = new ArrayList<>();
            for (Map.Entry<String, List<SymbolEntry>> group : sortedGroups(section.getValue())) {
                groups.add(new SymbolGroup(group.getKey(), group.getValue()));
            }
            out.add(new SymbolSection(section.getKey(), groups));
        }
        return out;
    }

    private static String sectionFor(SymbolEntry symbol) {
        if ("test".equals(symbol.kind)) {
            return "Tests";
        }
        if ("Main".equals(symbol.owner)) {
            return "Main";
        }
        if ("Root".equals(symbol.owner)) {
            return "Root";
        }
        if ("global".equals(symbol.kind)) {
            return "Globals";
        }
        if (symbol.file.startsWith("src/systems/")) {
            return "Systems";
        }
        return "Structs";
    }

    private static List<Map.Entry<String, List<SymbolEntry>>> sortedGroups(Map<String, List<SymbolEntry>> groups) {
        List<Map.Entry<String, List<SymbolEntry>>> entries = new ArrayList<>(groups.entrySet());
        Collections.sort(entries, new Comparator<Map.Entry<String, List<SymbolEntry>>>() {
            @Override
            public int compare(Map.Entry<String, List<SymbolEntry>> left, Map.Entry<String, List<SymbolEntry>> right) {
                return left.getKey().compareTo(right.getKey());
            }
        });
        return entries;
    }

    private static boolean isLifecycle(String name) {
        return "main".equals(name)
                || "init".equals(name)
                || "tick".equals(name)
                || "render".equals(name)
                || "on_code_swap".equals(name);
    }

    private static String receiverType(String signature) {
        String first = firstParameter(signature);
        if (first == null || !first.startsWith("self:")) {
            return null;
        }
        return first.substring("self:".length()).trim();
    }

    private static String firstParameterType(String signature) {
        String first = firstParameter(signature);
        if (first == null) {
            return null;
        }
        int colon = first.indexOf(':');
        return colon < 0 ? null : first.substring(colon + 1).trim();
    }

    private static String firstParameter(String signature) {
        int open = signature.indexOf('(');
        int close = signature.indexOf(')', open + 1);
        if (open < 0 || close < 0 || close <= open + 1) {
            return null;
        }
        String parameters = signature.substring(open + 1, close).trim();
        if (parameters.isEmpty()) {
            return null;
        }
        int comma = parameters.indexOf(',');
        return (comma < 0 ? parameters : parameters.substring(0, comma)).trim();
    }

    private static int readIdentifierEnd(String source, int start) {
        int index = start;
        while (index < source.length()) {
            char c = source.charAt(index);
            if (!Character.isLetterOrDigit(c) && c != '_') {
                break;
            }
            index += 1;
        }
        return index;
    }

    private static int findMatchingBrace(String source, int bodyStart) {
        if (bodyStart < 0 || bodyStart >= source.length()) {
            return -1;
        }

        int depth = 0;
        for (int index = bodyStart; index < source.length(); index += 1) {
            char c = source.charAt(index);
            if (c == '{') {
                depth += 1;
            } else if (c == '}') {
                depth -= 1;
                if (depth == 0) {
                    return index + 1;
                }
            }
        }
        return -1;
    }

    private static int minPositive(int left, int right) {
        if (left < 0) {
            return right;
        }
        if (right < 0) {
            return left;
        }
        return Math.min(left, right);
    }

    private static String titleCase(String value) {
        if (value.isEmpty()) {
            return value;
        }
        return value.substring(0, 1).toUpperCase(Locale.US) + value.substring(1);
    }


    private static final class GamePreviewView extends GLSurfaceView {
        interface CaptureCallback {
            void onCaptured(Bitmap bitmap, String error,
                    StasisPreviewRenderer.LogicalFrameSnapshot capturedFrame);
        }

        static final class AcceptanceFrameSubmission {
            private final int status;
            private final long baselinePresentationSerial;
            private final int frameToken;
            private final int trace;

            AcceptanceFrameSubmission(int status, long baselinePresentationSerial,
                    int frameToken, int trace) {
                this.status = status;
                this.baselinePresentationSerial = baselinePresentationSerial;
                this.frameToken = frameToken;
                this.trace = trace;
            }

            int status() {
                return status;
            }

            long baselinePresentationSerial() {
                return baselinePresentationSerial;
            }

            int frameToken() {
                return frameToken;
            }

            int trace() {
                return trace;
            }
        }

        private static final AcceptanceFrameSubmission NORMAL_SUCCESS_SUBMISSION =
                new AcceptanceFrameSubmission(0, -1L, -1, -1);

        private final MainActivity activity;
        private final StasisPreviewRenderer renderer;
        private final Runnable performanceRenderPump;
        private final WorkshopTextureProvider textureProvider;
        private static final long ACCEPTANCE_RENDER_PUMP_SLICE_MILLIS = 100L;
        private int touchX;
        private int touchY;
        private boolean touchActive;
        private boolean acceptanceTouchDispatch;
        private long lastNativeFrameDurationNanos;
        private long lastRendererSyncWaitNanos;

        GamePreviewView(MainActivity activity) {
            super(activity);
            this.activity = activity;
            setEGLContextClientVersion(2);
            setPreserveEGLContextOnPause(true);
            textureProvider = new WorkshopTextureProvider(activity);
            renderer = new StasisPreviewRenderer(textureProvider,
                    activity::recordRenderTimeNanos);
            performanceRenderPump = new Runnable() {
                @Override public void run() {
                    if (!renderer.isPerformanceSamplingForAcceptanceActive()) return;
                    requestRender();
                    postOnAnimation(this);
                }
            };
            setRenderer(renderer);
            setRenderMode(GLSurfaceView.RENDERMODE_WHEN_DIRTY);
            setFocusable(true);
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

        void dispatchAcceptanceTouch(int action, float x, float y) {
            long now = SystemClock.uptimeMillis();
            MotionEvent event = MotionEvent.obtain(now, now, action, x, y, 0);
            try {
                acceptanceTouchDispatch = true;
                onTouchEvent(event);
            } finally {
                acceptanceTouchDispatch = false;
                event.recycle();
            }
        }

        int acceptanceTrace() {
            synchronized (renderer) {
                return renderer.acceptanceTrace();
            }
        }

        int frameToken() {
            synchronized (renderer) {
                return renderer.frameToken();
            }
        }

        int rectCount() {
            synchronized (renderer) {
                return renderer.rectCount();
            }
        }

        void resetResourceScopeMetrics() {
            synchronized (renderer) {
                textureProvider.resetAcceptanceMetrics();
            }
        }

        void startPerformanceSamplingForAcceptance() {
            if (!BuildConfig.STASIS_RENDER_ACCEPTANCE) return;
            removeCallbacks(performanceRenderPump);
            queueEvent(renderer::startPerformanceSamplingForAcceptance);
            queueEvent(() -> activity.runOnUiThread(
                    () -> postOnAnimation(performanceRenderPump)));
        }

        JSONObject resourceScopeSnapshot() throws Exception {
            synchronized (renderer) {
                JSONObject snapshot = textureProvider.acceptanceSnapshot();
                snapshot.put("lifecycle_surface_generation", renderer.surfaceGeneration());
                snapshot.put("lifecycle_renderer_generation", renderer.rendererGeneration());
                snapshot.put("resources_ready", renderer.resourcesReady());
                return snapshot;
            }
        }

        JSONObject acceptanceBufferSnapshot() throws Exception {
            synchronized (renderer) {
                return renderer.acceptanceBufferSnapshot();
            }
        }

        void setWorkshopSoakAcceptanceActive(boolean active) {
            synchronized (renderer) {
                renderer.setWorkshopSoakAcceptanceActive(active);
            }
        }

        JSONObject workshopSoakPresentationSnapshot() throws Exception {
            synchronized (renderer) {
                return renderer.workshopSoakPresentationSnapshot();
            }
        }

        int rendererGeneration() {
            synchronized (renderer) {
                return renderer.rendererGeneration();
            }
        }

        boolean recreateEglContextForAcceptance(long timeoutMillis) {
            if (!BuildConfig.STASIS_RENDER_ACCEPTANCE) return false;
            int previous = rendererGeneration();
            setPreserveEGLContextOnPause(false);
            try {
                onHostPause();
                onHostResume();
                long deadline = SystemClock.uptimeMillis() + timeoutMillis;
                while (SystemClock.uptimeMillis() < deadline) {
                    requestRender();
                    if (rendererGeneration() > previous) return true;
                    SystemClock.sleep(10L);
                }
                return false;
            } finally {
                setPreserveEGLContextOnPause(true);
            }
        }

        float acceptanceFrameF32(int index) {
            synchronized (renderer) {
                return renderer.acceptanceFrameF32(index);
            }
        }

        boolean awaitPresentedFrame(AcceptanceFrameSubmission submission, long timeoutMillis) {
            return awaitPresentedFrame(submission.frameToken(), submission.trace(),
                    submission.baselinePresentationSerial(), timeoutMillis);
        }

        private boolean awaitPresentedFrame(int token, int trace, long afterPresentationSerial,
                long timeoutMillis) {
            if (!BuildConfig.STASIS_RENDER_ACCEPTANCE || timeoutMillis <= 0L) {
                return renderer.awaitPresentedFrame(token, trace, afterPresentationSerial,
                        timeoutMillis);
            }
            long deadline = System.nanoTime() + timeoutMillis * 1_000_000L;
            while (true) {
                long remainingNanos = deadline - System.nanoTime();
                if (remainingNanos <= 0L) return false;
                requestRender();
                remainingNanos = deadline - System.nanoTime();
                if (remainingNanos <= 0L) return false;
                long remainingMillis = (remainingNanos + 999_999L) / 1_000_000L;
                long waitMillis = Math.min(remainingMillis,
                        ACCEPTANCE_RENDER_PUMP_SLICE_MILLIS);
                if (renderer.awaitPresentedFrame(token, trace, afterPresentationSerial,
                        waitMillis)) {
                    return System.nanoTime() <= deadline;
                }
            }
        }

        boolean awaitPresentedFrameToken(int token, long timeoutMillis) {
            if (!BuildConfig.STASIS_RENDER_ACCEPTANCE || timeoutMillis <= 0L) {
                return renderer.awaitPresentedFrameToken(token, timeoutMillis);
            }
            long deadline = System.nanoTime() + timeoutMillis * 1_000_000L;
            while (true) {
                long remainingNanos = deadline - System.nanoTime();
                if (remainingNanos <= 0L) return false;
                requestRender();
                remainingNanos = deadline - System.nanoTime();
                if (remainingNanos <= 0L) return false;
                long remainingMillis = (remainingNanos + 999_999L) / 1_000_000L;
                long waitMillis = Math.min(remainingMillis,
                        ACCEPTANCE_RENDER_PUMP_SLICE_MILLIS);
                if (renderer.awaitPresentedFrameToken(token, waitMillis)) {
                    return System.nanoTime() <= deadline;
                }
            }
        }

        void onHostPause() {
            removeCallbacks(performanceRenderPump);
            renderer.onHostPaused();
            onPause();
        }

        void onHostResume() {
            onResume();
            queueEvent(renderer::onHostResumed);
            requestRender();
            if (renderer.isPerformanceSamplingForAcceptanceActive()) {
                postOnAnimation(performanceRenderPump);
            }
        }

        int runNativeFrame(String projectRoot, int inputX, int inputY, int inputActive,
                int screenWidth, int screenHeight, int[] header) {
            return runNativeFrameInternal(projectRoot, inputX, inputY, inputActive,
                    screenWidth, screenHeight, header, false).status();
        }

        AcceptanceFrameSubmission runNativeAcceptanceFrame(String projectRoot, int inputX,
                int inputY, int inputActive,
                int screenWidth, int screenHeight, int[] header) {
            return runNativeFrameInternal(projectRoot, inputX, inputY, inputActive,
                    screenWidth, screenHeight, header, true);
        }

        private AcceptanceFrameSubmission runNativeFrameInternal(String projectRoot, int inputX,
                int inputY,
                int inputActive, int screenWidth, int screenHeight, int[] header,
                boolean acceptance) {
            int status;
            long baselinePresentationSerial = -1L;
            int submittedFrameToken = -1;
            int submittedTrace = -1;
            boolean releaseBatchEnqueued = false;
            boolean releaseCancellationApplied = false;
            long requested = System.nanoTime();
            synchronized (renderer) {
                if (acceptance) baselinePresentationSerial = renderer.presentationSerial();
                long started = System.nanoTime();
                lastRendererSyncWaitNanos = started - requested;
                boolean drainReleases = !renderer.hasPendingSpriteReleases();
                status = nativeRunFrameInto(projectRoot, inputX, inputY, inputActive,
                        screenWidth, screenHeight, renderer.frameI32Bytes(),
                        renderer.frameF32Bytes(), renderer.frameU8Bytes());
                if (activity.audioFocus != null && nativeAudioRequested()) activity.audioFocus.resume();
                releaseCancellationApplied = renderer.cancelPendingSpriteReleases(
                        nativePollSpriteReleaseCancellations());
                if (!drainReleases && releaseCancellationApplied) {
                    drainReleases = !renderer.hasPendingSpriteReleases();
                }
                if (drainReleases) {
                    releaseBatchEnqueued = renderer.enqueuePendingSpriteReleases(
                            nativeDrainSpriteReleases());
                }
                lastNativeFrameDurationNanos = System.nanoTime() - started;
                renderer.copyFrameHeaderInto(header);
                if (acceptance && status == 0) {
                    submittedFrameToken = renderer.frameToken();
                    submittedTrace = MainActivity.nativeFrameTrace(renderer.frameI32Bytes(),
                            renderer.frameF32Bytes(), renderer.frameU8Bytes());
                    renderer.setAcceptanceTrace(submittedFrameToken, submittedTrace);
                } else {
                    renderer.clearAcceptanceTrace();
                }
            }
            if (status == 0 || releaseBatchEnqueued || releaseCancellationApplied) requestRender();
            if (!acceptance && status == 0) return NORMAL_SUCCESS_SUBMISSION;
            return new AcceptanceFrameSubmission(status, baselinePresentationSerial,
                    submittedFrameToken, submittedTrace);
        }

        long lastNativeFrameDurationNanos() {
            return lastNativeFrameDurationNanos;
        }

        long lastRendererSyncWaitNanos() {
            return lastRendererSyncWaitNanos;
        }

        void captureFrame(CaptureCallback callback) {
            renderer.requestCapture(callback::onCaptured);
            requestRender();
        }

        StasisPreviewRenderer.LogicalFrameSnapshot logicalFrameSnapshot() {
            return renderer.captureLogicalFrame();
        }

        @Override
        public boolean onTouchEvent(MotionEvent event) {
            touchX = Math.round(event.getX());
            touchY = Math.round(event.getY());
            int action = event.getActionMasked();
            if (action == MotionEvent.ACTION_DOWN && !acceptanceTouchDispatch) {
                nativeArmExternalUrlAction();
            }
            if (action == MotionEvent.ACTION_CANCEL) nativeClearExternalUrlAction();
            if (action == MotionEvent.ACTION_POINTER_DOWN && event.getPointerCount() >= 3) {
                activity.toggleBenchmarkHudFromPreview();
            }
            touchActive = action != MotionEvent.ACTION_UP && action != MotionEvent.ACTION_CANCEL;
            return true;
        }
    }

    public static boolean openExternalUrlFromNative(byte[] utf8Url) {
        MainActivity activity = externalUrlActivity;
        if (activity == null || utf8Url == null || utf8Url.length == 0
                || utf8Url.length > 2048 || activity.activityDestroyed
                || activity.isFinishing()) return false;
        final String url = new String(utf8Url, StandardCharsets.UTF_8);
        final Intent intent = new Intent(Intent.ACTION_VIEW, Uri.parse(url));
        intent.addCategory(Intent.CATEGORY_BROWSABLE);
        try {
            if (intent.resolveActivity(activity.getPackageManager()) == null) return false;
        } catch (RuntimeException error) {
            return false;
        }
        activity.runOnUiThread(() -> {
            MainActivity current = externalUrlActivity;
            if (current != activity || activity.activityDestroyed || activity.isFinishing()) return;
            try {
                activity.startActivity(intent);
            } catch (ActivityNotFoundException | SecurityException error) {
                android.util.Log.w("StasisWorkshop", "External URL request was blocked", error);
            }
        });
        return true;
    }


    private static final class RollingMetric {
        private static final long WINDOW_NANOS = 5_000_000_000L;
        private static final int CAPACITY = 600;
        private final long[] timestamps = new long[CAPACITY];
        private final long[] durations = new long[CAPACITY];
        private final long[] orderedDurations = new long[CAPACITY];
        private int next;
        private int count;

        void add(long timestampNanos, long durationNanos) {
            timestamps[next] = timestampNanos;
            durations[next] = durationNanos;
            next = (next + 1) % CAPACITY;
            if (count < CAPACITY) {
                count += 1;
            }
        }

        double averageMillis() {
            long now = System.nanoTime();
            long total = 0L;
            int samples = 0;
            for (int index = 0; index < count; index += 1) {
                if (now - timestamps[index] <= WINDOW_NANOS) {
                    total += durations[index];
                    samples += 1;
                }
            }
            if (samples == 0) {
                return 0.0;
            }
            return total / (samples * 1_000_000.0);
        }

        double percentileMillis(int percentile) {
            long now = System.nanoTime();
            int samples = 0;
            for (int index = 0; index < count; index += 1) {
                if (now - timestamps[index] <= WINDOW_NANOS) {
                    orderedDurations[samples++] = durations[index];
                }
            }
            if (samples == 0) return 0.0;
            Arrays.sort(orderedDurations, 0, samples);
            int rank = Math.min(samples - 1, (samples * percentile + 99) / 100 - 1);
            return orderedDurations[Math.max(0, rank)] / 1_000_000.0;
        }
    }

    private static final class SourceFile {
        final String path;
        final File diskFile;
        String source;

        SourceFile(String path, File diskFile, String source) {
            this.path = path;
            this.diskFile = diskFile;
            this.source = source;
        }
    }

    private static final class SymbolSection {
        final String title;
        final List<SymbolGroup> groups;

        SymbolSection(String title, List<SymbolGroup> groups) {
            this.title = title;
            this.groups = groups;
        }
    }

    private static final class SymbolGroup {
        final String title;
        final List<SymbolEntry> symbols;

        SymbolGroup(String title, List<SymbolEntry> symbols) {
            this.title = title;
            this.symbols = symbols;
        }
    }

    private static final class SymbolEntry {
        final String kind;
        final String name;
        final String owner;
        final String signature;
        final SourceFile sourceFile;
        final String file;
        String source;
        final String backingStructSource;
        String canonicalSymbolId;
        final int start;
        int end;

        SymbolEntry(String kind, String name, String owner, String signature, SourceFile sourceFile, String file, String source, int start, int end) {
            this(kind, name, owner, signature, sourceFile, file, source, start, end, "");
        }

        SymbolEntry(String kind, String name, String owner, String signature, SourceFile sourceFile, String file, String source, int start, int end, String backingStructSource) {
            this.kind = kind;
            this.name = name;
            this.owner = owner;
            this.signature = signature;
            this.sourceFile = sourceFile;
            this.file = file;
            this.source = source;
            this.start = start;
            this.end = end;
            this.backingStructSource = backingStructSource;
            this.canonicalSymbolId = "";
        }

        String displayName() {
            if ("struct".equals(kind)) {
                return signature;
            }
            return signature;
        }

        String identityKey() {
            return CanonicalSymbolIdentity.identityKey(
                    canonicalSymbolId, kind, file, owner, name);
        }
    }
}
