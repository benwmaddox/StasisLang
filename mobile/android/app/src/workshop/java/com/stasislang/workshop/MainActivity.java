package com.stasislang.workshop;

import android.app.Activity;
import android.app.AlertDialog;
import android.Manifest;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.res.AssetManager;
import android.content.pm.PackageManager;
import android.graphics.Color;
import android.graphics.Bitmap;
import android.opengl.GLES20;
import android.opengl.GLSurfaceView;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.media.MediaPlayer;
import android.media.MediaRecorder;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.speech.RecognitionListener;
import android.speech.RecognizerIntent;
import android.speech.SpeechRecognizer;
import android.text.InputType;
import android.view.DisplayCutout;
import android.view.Gravity;
import android.view.MotionEvent;
import android.view.View;
import android.view.Window;
import android.view.WindowInsets;
import android.util.Base64;
import android.widget.Button;
import android.widget.CheckBox;
import android.widget.AdapterView;
import android.widget.ArrayAdapter;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.HorizontalScrollView;
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
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.FloatBuffer;
import java.nio.IntBuffer;
import java.net.HttpURLConnection;
import java.net.URL;
import java.net.URLEncoder;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Calendar;
import java.util.Collections;
import java.util.Comparator;
import java.util.HashSet;
import java.util.Iterator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.TreeSet;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

import org.json.JSONArray;
import org.json.JSONObject;

public final class MainActivity extends Activity {
    private static final String PROJECT_DIR = WorkshopProjectRegistry.LEGACY_PROJECT_DIR;
    private static final String PROJECT_BASELINES_DIR = "workshop_project_baselines";
    private static final String PROJECT_BASELINE_READY = ".ready";
    private static final String SAMPLE_MIGRATION_PREFS = "workshop_sample_migrations";
    private static final String PONG_SLOW_BALL_MIGRATION = "pong_slow_ball_v1";
    private static final String AI_PREFS = "ai_settings";
    private static final String ONBOARDING_PREFS = "onboarding_settings";
    private static final String ONBOARDING_COMPLETE = "manual_tutorial_seen_v1";
    private static final String AI_SETUP_COMPLETE = "ai_setup_complete_v1";
    private static final String AI_PREF_API_KEY = "openai_api_key";
    private static final String AI_PREF_PROVIDER = "ai_provider";
    private static final String AI_PROVIDER_CODEX = "codex_on_device";
    private static final String AI_PROVIDER_API = "openai_api";
    private static final String AI_PREF_MODEL = "openai_model";
    private static final String AI_PREF_MODEL_DEFAULT_VERSION = "openai_model_default_version";
    private static final String AI_PREF_LAST_USAGE = "last_ai_usage";
    private static final String AI_PREF_COMMAND_HISTORY_PREFIX = "command_history_";
    private static final String AI_PREF_OUTCOME_HISTORY_PREFIX = "outcome_history_";
    private static final String AI_PREF_MONTHLY_LIMIT_USD = "monthly_limit_usd";
    private static final String AI_PREF_MONTH_KEY = "monthly_spend_month";
    private static final String AI_PREF_MONTH_SPEND_USD = "monthly_spend_usd";
    private static final String AI_PREF_CODEX_LIMITS_JSON = "codex_limits_json";
    private static final String AI_PREF_CODEX_LIMITS_REFRESH_ATTEMPT_MS = "codex_limits_refresh_attempt_ms";
    private static final String AI_PREF_CODEX_PRIMARY_MIGRATION = "codex_primary_after_turn_bridge_v1";
    private static final String AI_PREF_DESIGN_SKETCHES_PREFIX = "design_sketches_";
    private static final String GITHUB_PREFS = "github_sync_settings";
    private static final String GITHUB_PREF_TOKEN = "github_token";
    private static final String GITHUB_PREF_REPOSITORY = "github_repository";
    private static final String GITHUB_PREF_BRANCH = "github_branch";
    private static final String GITHUB_PREF_OPERATION = "github_pending_operation";
    private static final String GITHUB_PREF_OPERATION_STATE = "github_operation_state";
    private static final String GITHUB_PREF_OPERATION_DETAIL = "github_operation_detail";
    private static final String GITHUB_PREF_REVIEW_FINGERPRINT = "github_review_fingerprint";
    private static final String AI_TRACE_LOG = "ai_trace.jsonl";
    private static final String DEFAULT_AI_MODEL = "gpt-5.6-sol";
    private static final int DEFAULT_AI_MODEL_VERSION = 2;
    private static final String AI_PROMPT_CACHE_KEY = "stasis-android-workshop-v2";
    private static final long AI_TRACE_RETENTION_MS = 24L * 60L * 60L * 1000L;
    private static final long DEFAULT_TICK_INTERVAL_MS = 16L;
    private static final long DEBUG_UPDATE_INTERVAL_NANOS = 250_000_000L;
    private static final double FRAME_BUDGET_MILLIS = 1000.0 / 60.0;
    private static final int MAX_RENDER_COMMANDS = 8;
    private static final int MAX_AI_AGENT_TURNS = 15;
    private static final int MAX_AI_TOOL_CALLS_PER_BATCH = 12;
    private static final int MAX_AI_READ_ONLY_BATCHES = 2;
    private static final int MAX_AI_OUTPUT_TOKENS = 8192;
    private static final int MAX_AI_IMAGE_ATTACHMENTS = 4;
    private static final int MAX_AI_IMAGE_ATTACHMENT_BYTES = 12 * 1024 * 1024;
    private static final int MAX_AI_GENERATED_BASE64_CHARS = ((8 * 1024 * 1024 + 2) / 3) * 4 + 16;
    private static final long MAX_PREVIEW_CAPTURE_PIXELS = 8_000_000L;
    private static final int MAX_COMMAND_HISTORY = 20;
    private static final int GITHUB_NETWORK_TIMEOUT_MS = 15_000;
    private static final int MAX_GITHUB_BACKUP_BYTES = 32 * 1024 * 1024;
    private static final int TOP_CONTROL_END_MARGIN_DP = 10;
    private static final int VOICE_TOP_MARGIN_DP = 64;
    private static final int VOICE_ACTION_TOP_MARGIN_DP = 120;
    private static final int AI_CONNECT_TIMEOUT_MS = 15_000;
    private static final int AI_READ_TIMEOUT_MS = 120_000;
    private static final long CODEX_LIMIT_REFRESH_DEBOUNCE_MS = 30L * 60L * 1000L;
    private static final long STABLE_LAUNCH_DELAY_MS = 60_000L;
    private static final int VOICE_RECORD_PERMISSION_REQUEST = 41;
    private static final int AUDIO_RECORD_PERMISSION_REQUEST = 42;
    private static final int EXPORT_PROJECT_REQUEST = 71;
    private static final int IMPORT_PROJECT_REQUEST = 72;
    private static final int IMPORT_IMAGE_REQUEST = 73;
    private static final int IMPORT_AUDIO_REQUEST = 74;
    private static final int EXPORT_SUPPORT_BUNDLE_REQUEST = 75;
    private static final double GPT_IMAGE_2_LOW_1024_USD = 0.006;
    private static final int RENDER_FRAME_HEADER_SIZE = 6;
    private static final int RENDER_COMMAND_STRIDE = 7;
    private static final int RENDER_FRAME_I32_CAPACITY =
            RENDER_FRAME_HEADER_SIZE + MAX_RENDER_COMMANDS * RENDER_COMMAND_STRIDE;
    private static final int RECT_VERTICES = 6;
    private static final int RENDER_VERTEX_FLOATS = 6;
    private static final int RENDER_VERTEX_BYTES = RENDER_VERTEX_FLOATS * 4;
    private static final int RENDER_VERTEX_BUFFER_FLOATS =
            MAX_RENDER_COMMANDS * RECT_VERTICES * RENDER_VERTEX_FLOATS;
    private static final int SPRITE_VERTEX_FLOATS = 8;
    private static final int SPRITE_VERTEX_BYTES = SPRITE_VERTEX_FLOATS * 4;
    private static final int SPRITE_VERTEX_BUFFER_FLOATS =
            MAX_RENDER_COMMANDS * RECT_VERTICES * SPRITE_VERTEX_FLOATS;
    private TextView sourceTitle;
    private LinearLayout selectedSourcePanel;
    private LinearLayout manualEditBody;
    private EditText sourceEditor;
    private EditText aiPromptEditor;
    private EditText aiApiKeyEditor;
    private Spinner aiProviderSelector;
    private boolean aiProviderSelectionFromTouch;
    private TextView codexAccountStatus;
    private EditText aiModelEditor;
    private EditText aiMonthlyLimitUsdEditor;
    private TextView aiBudgetStatus;
    private TextView aiAttachmentStatus;
    private TextView screenshotAttachmentStatus;
    private CheckBox allowAiImageGeneration;
    private TextView aiStepPill;
    private TextView aiActionPill;
    private TextView aiPhasePill;
    private TextView aiElapsedPill;
    private Button aiCancelButton;
    private LinearLayout aiSettingsBody;
    private LinearLayout commandHistoryBody;
    private TextView commandHistoryText;
    private LinearLayout aiQueueSection;
    private LinearLayout aiQueueBody;
    private LinearLayout githubSettingsBody;
    private LinearLayout privacySettingsBody;
    private LinearLayout onboardingBody;
    private EditText githubTokenEditor;
    private EditText githubRepositoryEditor;
    private EditText githubBranchEditor;
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
    private final HashSet<String> selectedImageAssets = new HashSet<>();
    private final HashSet<String> selectedDesignSketchAssets = new HashSet<>();
    private String selectedImageAssetProjectId = "";
    private WorkshopProjectRegistry.ProjectInfo activeProject;
    private WorkshopProjectRegistry.ProjectInfo pendingExportProject;
    private String pendingImportProjectName = "";
    private String projectRegistryError = "";
    private String reviewedGitHubChangeFingerprint = "";
    private String credentialStorageError = "";
    private volatile boolean githubOperationActive;
    private volatile boolean projectIoActive;
    private volatile boolean aiRunActive;
    private volatile boolean activityDestroyed;
    private boolean restartLoopRecoveryActive;
    private boolean phoneNativeCodexReady;
    private boolean codexSignedIn;
    private AlertDialog codexLoginDialog;
    private TextView codexLoginDialogStatus;
    private String codexLoginUserCode = "";
    private String codexLoginVerificationUrl = "";
    private boolean showProjectChooserAfterCodexLogin;
    private final WorkshopCodexLoginLifecycle codexLoginLifecycle = new WorkshopCodexLoginLifecycle();
    private final Runnable codexStatusPoll = new Runnable() {
        @Override public void run() { refreshPhoneNativeCodexStatus(); }
    };
    private volatile boolean aiCancelRequested;
    private volatile HttpURLConnection activeAiConnection;
    private ConnectivityManager connectivityManager;
    private ConnectivityManager.NetworkCallback networkCallback;
    private boolean networkCallbackRegistered;
    private String activeAiPrompt = "";
    private AndroidAiQueue.Entry activeAiQueueEntry;
    private volatile List<AiImageAttachment> activeAiImageAttachments = Collections.emptyList();
    private Bitmap pendingPreviewScreenshot;
    private MediaPlayer activeAudioPreview;
    private MediaRecorder activeAudioRecorder;
    private File activeAudioRecordingFile;
    private boolean audioRecordingActive;
    private JSONObject pendingPreviewLogicalSnapshot;
    private boolean attachPreviewPixels;
    private boolean attachPreviewLogicalSnapshot;
    private TextView reloadStatus;
    private TextView diagnosticStatus;
    private String diagnosticFile = "";
    private String diagnosticSymbol = "";
    private int diagnosticLine;
    private AndroidEditRecoveryStore.Entry selectedRecoveryEntry;
    private TextView changeSummary;
    private TextView gameStatus;
    private GamePreviewView gamePreview;
    private LinearLayout symbolList;
    private File projectRootFile;
    private String projectRootPath;
    private ScrollView editorPanel;
    private Button editorToggle;
    private Button voiceToggle;
    private LinearLayout voiceActionRow;
    private TextView voiceStatus;
    private Button voiceRunButton;
    private SpeechRecognizer voiceRecognizer;
    private String voiceTranscript = "";
    private final Handler gameLoopHandler = new Handler(Looper.getMainLooper());
    private final ExecutorService githubSyncExecutor = Executors.newSingleThreadExecutor();
    private final ExecutorService projectIoExecutor = Executors.newSingleThreadExecutor();
    private final ExecutorService codexExecutor = Executors.newSingleThreadExecutor();
    private Runnable gameLoop;
    private final int[] nativeFrameValues = new int[RENDER_FRAME_I32_CAPACITY];
    private final StringBuilder debugTextBuilder = new StringBuilder(64);
    private final RollingMetric tickMetric = new RollingMetric();
    private final RollingMetric renderMetric = new RollingMetric();
    private boolean compileReady;
    private boolean compileAttempted;
    private String lastCompileResult = "CompileNotRun";
    private int aiSimTouchX;
    private int aiSimTouchY;
    private int aiSimTouchActive;
    private int aiSimScreenWidth;
    private int aiSimScreenHeight;
    private long lastDebugUpdateNanos;
    private long aiStartedAtNanos;
    private int aiProgressStep;
    private int aiProgressActions;
    private SymbolEntry selectedSymbol;

    static {
        System.loadLibrary("stasis_mobile_smoke");
    }

    private static native String nativeStatus();
    private static native String nativeCompileProject(String projectRoot);
    private static native String nativeRunTick(String projectRoot, int touchX, int touchY, int touchActive, int screenWidth, int screenHeight);
    private static native int nativeRunFrameInto(String projectRoot, int touchX, int touchY, int touchActive, int screenWidth, int screenHeight, int[] frameValues);
    private static native String nativeSetRuntimeI32(String projectRoot, String path, int value);
    private static native String nativeGetRuntimeI32(String projectRoot, String path);
    private static native String nativeRunTests(String projectRoot);
    private static native String nativeCodexBeginDeviceLogin(String codexHome);
    private static native String nativeCodexAccountStatus(String codexHome);
    private static native String nativeCodexAccountRateLimits(String codexHome);
    private static native long nativeCodexBeginResponse();
    private static native void nativeCodexCancelResponse();
    private static native String nativeCodexResponse(String codexHome, String requestJson,
                                                     long generation);
    private static native int nativeCodexInitialize(Object applicationContext);

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        AndroidCrashStore.install(this);
        JSONObject crashState = AndroidCrashStore.noteLaunch(this);
        restartLoopRecoveryActive = crashState.optBoolean("restart_loop_detected", false);
        phoneNativeCodexReady = nativeCodexInitialize(getApplicationContext()) == 0;

        try {
            activeProject = WorkshopProjectRegistry.initialize(this,
                    WorkshopTemplateCatalog.DEFAULT_TEMPLATE_ID);
            projectRootFile = activeProject.root;
        } catch (Exception error) {
            projectRegistryError = error.getMessage();
            projectRootFile = new File(getFilesDir(), PROJECT_DIR);
        }
        projectRootPath = projectRootFile.getAbsolutePath();

        Window window = getWindow();
        window.setStatusBarColor(Color.BLACK);
        window.setNavigationBarColor(Color.BLACK);

        ProjectSnapshot project = loadBundledProject();
        try {
            if (migrateBundledPongBallSpeed()) project = loadBundledProject();
            ensureActiveProjectBaseline(project);
        } catch (IOException error) {
            projectRegistryError = "baseline: " + error.getMessage();
        }
        setContentView(createWorkshopView(project));
        registerNetworkMonitoring();
        markInterruptedAiOutcomeIfNeeded();
        restoreWorkshopUiState(savedInstanceState);
        restorePendingDraft();
        gameLoopHandler.post(new Runnable() {
            @Override public void run() { startNextQueuedAiIfIdle(); }
        });
        gameLoopHandler.postDelayed(new Runnable() {
            @Override public void run() { AndroidCrashStore.markLaunchStable(MainActivity.this); }
        }, STABLE_LAUNCH_DELAY_MS);
        if (crashState.optBoolean("present", false)) {
            setStatusText(restartLoopRecoveryActive
                    ? "Restart loop detected; preview and queued AI are paused until the local crash record is cleared in Privacy & Data"
                    : "Previous crash detected; export a redacted support bundle or clear the local crash record in Privacy & Data");
        }
        if (savedInstanceState == null) {
            gameLoopHandler.post(new Runnable() {
                @Override public void run() {
                    if (needsFirstRunAiSetup()) showFirstRunAiSetup();
                    else showProjectChooser();
                }
            });
        }
    }

    @Override
    protected void onResume() {
        super.onResume();
        codexLoginLifecycle.onResume();
        refreshPhoneNativeCodexStatus();
        startNextQueuedAiIfIdle();
    }

    @Override
    protected void onPause() {
        codexLoginLifecycle.onPause();
        gameLoopHandler.removeCallbacks(codexStatusPoll);
        persistPendingDraft();
        stopVoiceRecognition();
        stopAudioPreview();
        cancelAudioRecording(false);
        super.onPause();
    }

    @Override
    protected void onSaveInstanceState(Bundle outState) {
        persistPendingDraft();
        outState.putString("ai_prompt", aiPromptEditor == null ? "" : aiPromptEditor.getText().toString());
        outState.putString("voice_transcript", voiceTranscript);
        outState.putBoolean("editor_open", editorPanel != null && editorPanel.getVisibility() == View.VISIBLE);
        outState.putBoolean("manual_open", manualEditBody != null && manualEditBody.getVisibility() == View.VISIBLE);
        outState.putBoolean("projects_open", projectSettingsBody != null && projectSettingsBody.getVisibility() == View.VISIBLE);
        outState.putBoolean("history_open", commandHistoryBody != null && commandHistoryBody.getVisibility() == View.VISIBLE);
        outState.putBoolean("ai_settings_open", aiSettingsBody != null && aiSettingsBody.getVisibility() == View.VISIBLE);
        outState.putBoolean("github_settings_open", githubSettingsBody != null && githubSettingsBody.getVisibility() == View.VISIBLE);
        outState.putBoolean("privacy_open", privacySettingsBody != null && privacySettingsBody.getVisibility() == View.VISIBLE);
        outState.putBoolean("onboarding_open", onboardingBody != null && onboardingBody.getVisibility() == View.VISIBLE);
        outState.putInt("editor_scroll_y", editorPanel == null ? 0 : editorPanel.getScrollY());
        outState.putStringArrayList("selected_image_paths", new ArrayList<String>(selectedImageAssets));
        outState.putStringArrayList("selected_design_sketch_paths",
                new ArrayList<String>(selectedDesignSketchAssets));
        if (selectedSymbol != null) {
            outState.putString("selected_file", selectedSymbol.file);
            outState.putString("selected_kind", selectedSymbol.kind);
            outState.putString("selected_name", selectedSymbol.name);
            outState.putString("selected_owner", selectedSymbol.owner);
        }
        super.onSaveInstanceState(outState);
    }

    @Override
    protected void onDestroy() {
        activityDestroyed = true;
        stopVoiceRecognition();
        gameLoopHandler.removeCallbacks(codexStatusPoll);
        if (codexLoginDialog != null) codexLoginDialog.dismiss();
        if (!WorkshopLongWorkCoordinator.isAiActive()) {
            aiCancelRequested = true;
            nativeCodexCancelResponse();
        }
        unregisterNetworkMonitoring();
        if (!WorkshopLongWorkCoordinator.isAiActive()) {
            HttpURLConnection aiConnection = activeAiConnection;
            if (aiConnection != null) aiConnection.disconnect();
        }
        if (!WorkshopLongWorkCoordinator.isGitHubActive()) githubSyncExecutor.shutdownNow();
        if (!WorkshopLongWorkCoordinator.isProjectIoActive()) projectIoExecutor.shutdownNow();
        codexExecutor.shutdownNow();
        if (pendingPreviewScreenshot != null && !pendingPreviewScreenshot.isRecycled()) {
            pendingPreviewScreenshot.recycle();
        }
        stopAudioPreview();
        cancelAudioRecording(false);
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
        FrameLayout root = new FrameLayout(this);
        root.setBackgroundColor(Color.rgb(15, 20, 28));
        installSystemInsetGuard(root);

        gamePreview = new GamePreviewView(this);
        gamePreview.setContentDescription("Interactive Stasis game preview. Touch the game to control it.");
        root.addView(gamePreview, new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT,
                FrameLayout.LayoutParams.MATCH_PARENT));

        installVoiceChangeControls(root);

        if (BuildConfig.STASIS_PUBLISHED_BUILD) {
            installGameStatusOverlay(root, false);
            startGameLoop();
            return root;
        }

        installGameStatusOverlay(root, true);
        LinearLayout content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        content.setPadding(dp(14), dp(12), dp(14), dp(12));
        content.setBackground(createPanelBackground(Color.rgb(247, 248, 251), Color.rgb(190, 199, 212)));

        TextView title = new TextView(this);
        title.setText("Stasis Workshop");
        title.setTextColor(Color.rgb(22, 27, 34));
        title.setTextSize(20.0f);
        title.setTypeface(Typeface.DEFAULT_BOLD);
        if (Build.VERSION.SDK_INT >= 28) title.setAccessibilityHeading(true);
        title.setPadding(0, 0, 0, dp(8));
        content.addView(title, fullWidth());

        content.addView(createAiControls(), fullWidth());

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
        sourceTitle.setTextColor(Color.rgb(22, 27, 34));
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
        reloadStatus.setTextColor(Color.rgb(73, 84, 100));
        reloadStatus.setTextSize(13.0f);
        reloadStatus.setPadding(0, dp(8), 0, dp(6));
        reloadStatus.setAccessibilityLiveRegion(View.ACCESSIBILITY_LIVE_REGION_POLITE);
        content.addView(reloadStatus, fullWidth());

        Button diagnosticToggle = new Button(this);
        diagnosticToggle.setText("Diagnostics & Recovery");
        content.addView(diagnosticToggle, fullWidth());
        final LinearLayout diagnosticBody = new LinearLayout(this);
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
        diagnosticStatus.setTextColor(Color.rgb(125, 55, 45));
        diagnosticStatus.setTypeface(Typeface.MONOSPACE);
        diagnosticStatus.setAccessibilityLiveRegion(View.ACCESSIBILITY_LIVE_REGION_ASSERTIVE);
        diagnosticBody.addView(diagnosticStatus, fullWidth());
        LinearLayout diagnosticActions = new LinearLayout(this);
        boolean narrowLayout = getResources().getConfiguration().screenWidthDp < 480;
        diagnosticActions.setOrientation(narrowLayout ? LinearLayout.VERTICAL : LinearLayout.HORIZONTAL);
        Button goToDiagnostic = new Button(this);
        goToDiagnostic.setText("Go to Diagnostic");
        goToDiagnostic.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { goToDiagnosticSource(); }
        });
        diagnosticActions.addView(goToDiagnostic, narrowLayout ? fullWidth()
                : new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1.0f));
        Button recoveryHistory = new Button(this);
        recoveryHistory.setText("Recovery History");
        recoveryHistory.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { showRecoveryHistory(); }
        });
        diagnosticActions.addView(recoveryHistory, narrowLayout ? fullWidth()
                : new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1.0f));
        Button undoFailedApply = new Button(this);
        undoFailedApply.setText("Undo Failed Apply");
        undoFailedApply.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { undoSelectedFailedApply(); }
        });
        diagnosticActions.addView(undoFailedApply, narrowLayout ? fullWidth()
                : new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1.0f));
        diagnosticBody.addView(diagnosticActions, fullWidth());
        refreshRecoveryStatus();

        changeSummary = new TextView(this);
        changeSummary.setTextColor(Color.rgb(73, 84, 100));
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
        editorPanel.setVisibility(View.GONE);
        editorPanel.addView(content);
        FrameLayout.LayoutParams editorParams = new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT,
                FrameLayout.LayoutParams.MATCH_PARENT,
                Gravity.TOP | Gravity.START);
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
        editorToggle.setTextColor(Color.WHITE);
        editorToggle.setContentDescription("Open Workshop menu");
        editorToggle.setBackground(createPanelBackground(Color.rgb(35, 45, 60), Color.rgb(83, 96, 115)));
        editorToggle.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                toggleEditorPanel();
            }
        });
        FrameLayout.LayoutParams toggleParams = new FrameLayout.LayoutParams(dp(52), dp(48), Gravity.TOP | Gravity.END);
        toggleParams.setMargins(0, dp(8), dp(TOP_CONTROL_END_MARGIN_DP), 0);
        root.addView(editorToggle, toggleParams);
        if (voiceToggle != null) {
            voiceToggle.bringToFront();
        }

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
        gameStatus.setText("tick=-- ms  render=-- ms  budget=--%");
        gameStatus.setTextColor(Color.WHITE);
        gameStatus.setTextSize(12.0f);
        gameStatus.setSingleLine(true);
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

    private void installVoiceChangeControls(FrameLayout root) {
        voiceToggle = new Button(this);
        voiceToggle.setText("Voice");
        voiceToggle.setContentDescription("Start voice command recording");
        voiceToggle.setTextColor(Color.WHITE);
        voiceToggle.setBackground(createPanelBackground(Color.rgb(35, 45, 60), Color.rgb(83, 96, 115)));
        voiceToggle.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                startVoiceChange();
            }
        });
        FrameLayout.LayoutParams voiceParams = new FrameLayout.LayoutParams(dp(74), dp(48), Gravity.TOP | Gravity.END);
        voiceParams.setMargins(0, dp(VOICE_TOP_MARGIN_DP), dp(TOP_CONTROL_END_MARGIN_DP), 0);
        root.addView(voiceToggle, voiceParams);

        voiceActionRow = new LinearLayout(this);
        voiceActionRow.setOrientation(LinearLayout.HORIZONTAL);
        voiceActionRow.setPadding(dp(8), dp(4), dp(8), dp(4));
        voiceActionRow.setBackground(createPanelBackground(Color.rgb(35, 45, 60), Color.rgb(83, 96, 115)));
        voiceActionRow.setVisibility(View.GONE);

        voiceStatus = new TextView(this);
        voiceStatus.setTextColor(Color.WHITE);
        voiceStatus.setTextSize(12.0f);
        voiceStatus.setGravity(Gravity.CENTER_VERTICAL);
        voiceActionRow.addView(voiceStatus, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1.0f));

        Button voiceCancel = new Button(this);
        voiceCancel.setText("Cancel");
        voiceCancel.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                cancelVoiceChange();
            }
        });
        voiceActionRow.addView(voiceCancel, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.WRAP_CONTENT, LinearLayout.LayoutParams.WRAP_CONTENT));

        voiceRunButton = new Button(this);
        voiceRunButton.setText("Run");
        voiceRunButton.setEnabled(false);
        voiceRunButton.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                runVoiceChange();
            }
        });
        voiceActionRow.addView(voiceRunButton, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.WRAP_CONTENT, LinearLayout.LayoutParams.WRAP_CONTENT));

        FrameLayout.LayoutParams actionParams = new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT,
                FrameLayout.LayoutParams.WRAP_CONTENT,
                Gravity.TOP | Gravity.START);
        actionParams.setMargins(dp(8), dp(VOICE_ACTION_TOP_MARGIN_DP), dp(8), 0);
        root.addView(voiceActionRow, actionParams);
    }

    private void startVoiceChange() {
        if (audioRecordingActive) {
            setStatusText("Finish or cancel audio recording before starting a voice command");
            return;
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M
                && checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            requestPermissions(new String[] { Manifest.permission.RECORD_AUDIO }, VOICE_RECORD_PERMISSION_REQUEST);
            return;
        }
        if (!SpeechRecognizer.isRecognitionAvailable(this)) {
            setStatusText("Voice change unavailable: speech recognition is not installed");
            return;
        }

        stopVoiceRecognition();
        voiceTranscript = "";
        voiceActionRow.setVisibility(View.VISIBLE);
        voiceActionRow.bringToFront();
        voiceStatus.setText("Listening for a change request...");
        voiceRunButton.setEnabled(false);
        voiceToggle.setEnabled(false);
        voiceRecognizer = SpeechRecognizer.createSpeechRecognizer(this);
        voiceRecognizer.setRecognitionListener(new RecognitionListener() {
            @Override public void onReadyForSpeech(android.os.Bundle params) { }
            @Override public void onBeginningOfSpeech() { voiceStatus.setText("Recording change request..."); }
            @Override public void onRmsChanged(float rmsdB) { }
            @Override public void onBufferReceived(byte[] buffer) { }
            @Override public void onEndOfSpeech() { voiceStatus.setText("Transcribing change request..."); }
            @Override public void onError(int error) {
                voiceStatus.setText("Voice recording failed; Cancel or try again");
                voiceToggle.setEnabled(true);
            }
            @Override public void onResults(android.os.Bundle results) { acceptVoiceResults(results); }
            @Override public void onPartialResults(android.os.Bundle partialResults) { }
            @Override public void onEvent(int eventType, android.os.Bundle params) { }
        });
        Intent intent = new Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH);
        intent.putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM);
        intent.putExtra(RecognizerIntent.EXTRA_PROMPT, "Describe the Stasis change to make");
        voiceRecognizer.startListening(intent);
    }

    private void acceptVoiceResults(android.os.Bundle results) {
        ArrayList<String> candidates = results.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION);
        voiceTranscript = candidates == null || candidates.isEmpty() ? "" : candidates.get(0).trim();
        if (voiceTranscript.isEmpty()) {
            voiceStatus.setText("No voice request captured; Cancel or try again");
            voiceToggle.setEnabled(true);
            return;
        }
        voiceStatus.setText("Ready: " + voiceTranscript);
        voiceRunButton.setEnabled(true);
        voiceToggle.setEnabled(true);
    }

    private void cancelVoiceChange() {
        stopVoiceRecognition();
        voiceTranscript = "";
        if (voiceActionRow != null) {
            voiceActionRow.setVisibility(View.GONE);
        }
        if (voiceToggle != null) {
            voiceToggle.setEnabled(true);
        }
        setStatusText("Voice change cancelled");
    }

    private void runVoiceChange() {
        if (voiceTranscript.isEmpty()) {
            setStatusText("Voice change needs a captured request before Run");
            return;
        }
        stopVoiceRecognition();
        if (aiPromptEditor != null) {
            aiPromptEditor.setText(voiceTranscript);
        }
        if (voiceActionRow != null) {
            voiceActionRow.setVisibility(View.GONE);
        }
        if (voiceToggle != null) {
            voiceToggle.setEnabled(true);
        }
        setStatusText("Voice change confirmed: adding it to the AI queue");
        runAiPatch("voice", null);
    }

    private void stopVoiceRecognition() {
        if (voiceRecognizer != null) {
            voiceRecognizer.cancel();
            voiceRecognizer.destroy();
            voiceRecognizer = null;
        }
    }

    @Override
    public void onRequestPermissionsResult(int requestCode, String[] permissions, int[] grantResults) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults);
        if (requestCode == VOICE_RECORD_PERMISSION_REQUEST) {
            if (grantResults.length > 0 && grantResults[0] == PackageManager.PERMISSION_GRANTED) {
                startVoiceChange();
            } else {
                setStatusText("Voice change needs microphone permission");
            }
        } else if (requestCode == AUDIO_RECORD_PERMISSION_REQUEST) {
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
        if (opening) {
            editorPanel.bringToFront();
        }
        if (editorToggle != null) {
            editorToggle.setText(opening ? "\u00D7" : "\u2630");
            editorToggle.setContentDescription(opening ? "Close Workshop menu" : "Open Workshop menu");
            editorToggle.bringToFront();
        }
        if (voiceActionRow != null && voiceActionRow.getVisibility() == View.VISIBLE) {
            voiceActionRow.bringToFront();
        }
        if (voiceToggle != null) {
            voiceToggle.setVisibility(opening ? View.GONE : View.VISIBLE);
            if (!opening) voiceToggle.bringToFront();
        }
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
                if (compileReady) {
                    runNativeTick();
                }
                gameLoopHandler.postDelayed(this, DEFAULT_TICK_INTERVAL_MS);
            }
        };
        gameLoopHandler.post(gameLoop);
    }

    private static boolean isRunnableCompile(String compileResult) {
        return compileResult.startsWith("CompilePlanned") && compileResult.contains("status=0");
    }

    private void setStatusText(String status) {
        if (reloadStatus != null) {
            reloadStatus.setText(compactStatusText(status));
        }
    }

    private static String compactStatusText(String status) {
        if (status == null) return "";
        if (status.startsWith("CompilePlanned") && status.contains("status=0")) {
            String reload = reloadKind(status);
            if ("FastReload".equals(reload)) return "Game updated - hot swapped";
            if ("ResetRequired".equals(reload)) return "Game updated - restarted";
            return "Game ready";
        }
        return status;
    }

    private TextView createAiProgressPill(String text) {
        TextView pill = new TextView(this);
        pill.setText(text);
        pill.setTextSize(11.0f);
        pill.setTextColor(Color.rgb(35, 45, 60));
        pill.setTypeface(Typeface.DEFAULT_BOLD);
        pill.setGravity(Gravity.CENTER);
        pill.setPadding(dp(10), dp(4), dp(10), dp(4));
        GradientDrawable background = new GradientDrawable();
        background.setColor(Color.rgb(228, 234, 242));
        background.setStroke(dp(1), Color.rgb(180, 192, 208));
        background.setCornerRadius(dp(14));
        pill.setBackground(background);
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(LinearLayout.LayoutParams.WRAP_CONTENT, LinearLayout.LayoutParams.WRAP_CONTENT);
        params.setMargins(0, dp(6), dp(6), dp(2));
        pill.setLayoutParams(params);
        return pill;
    }

    private void updateAiProgress(int step, int actions, String phase) {
        aiProgressStep = step;
        aiProgressActions = actions;
        if (aiStepPill != null) {
            aiStepPill.setText("step " + step + "/" + MAX_AI_AGENT_TURNS);
        }
        if (aiActionPill != null) {
            aiActionPill.setText("actions " + actions);
        }
        if (aiPhasePill != null) {
            aiPhasePill.setText(phase);
        }
        if (aiElapsedPill != null) {
            aiElapsedPill.setText("time " + currentAiElapsedText());
        }
    }

    private String currentAiElapsedText() {
        if (aiStartedAtNanos == 0L) {
            return "0.0s";
        }
        return formatElapsedMillis((System.nanoTime() - aiStartedAtNanos) / 1_000_000L);
    }

    private static String formatElapsedMillis(long millis) {
        long tenths = (millis + 50L) / 100L;
        return Long.toString(tenths / 10L) + "." + Long.toString(tenths % 10L) + "s";
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

    private static String aiReloadPhase(String compileResult) {
        String reload = reloadKind(compileResult);
        if ("FastReload".equals(reload)) {
            return "hot swapped";
        }
        if ("NoChange".equals(reload)) {
            return "no change";
        }
        if ("ResetRequired".equals(reload)) {
            return "reset reload";
        }
        if ("InitialCompile".equals(reload)) {
            return "compiled";
        }
        return "applied";
    }

    private static String aiReloadSummary(String compileResult) {
        String reload = reloadKind(compileResult);
        if ("FastReload".equals(reload)) {
            return "hot swap=FastReload";
        }
        if ("NoChange".equals(reload)) {
            return "hot swap=NoChange";
        }
        return "reload=" + reload;
    }
    private void postAiProgress(final int step, final int actions, final String phase) {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            updateAiProgress(step, actions, phase);
            return;
        }
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                updateAiProgress(step, actions, phase);
            }
        });
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
        double renderMillis = renderMetric.averageMillis();
        int budgetPercent = Math.max(0, (int)(((tickMillis + renderMillis) * 100.0 / FRAME_BUDGET_MILLIS) + 0.5));
        debugTextBuilder.setLength(0);
        debugTextBuilder.append("tick=");
        appendMillis(debugTextBuilder, tickMillis);
        debugTextBuilder.append(" ms  render=");
        appendMillis(debugTextBuilder, renderMillis);
        debugTextBuilder.append(" ms  budget=");
        appendPercent(debugTextBuilder, budgetPercent);
        appendExplorationProgress(debugTextBuilder);
        gameStatus.setTextColor(debugColorForBudget(budgetPercent));
        gameStatus.setText(debugTextBuilder.toString());
    }

    private void appendExplorationProgress(StringBuilder text) {
        if (!compileReady || activeProject == null || !"exploration".equals(activeProject.templateId)) return;
        String collectedResult = nativeGetRuntimeI32(projectRootPath(), "GameState.collected_count");
        String totalResult = nativeGetRuntimeI32(projectRootPath(), "GameState.total_collectibles");
        String stageResult = nativeGetRuntimeI32(projectRootPath(), "GameState.tutorial_stage");
        if (collectedResult == null || collectedResult.startsWith("StateError")
                || totalResult == null || totalResult.startsWith("StateError")
                || stageResult == null || stageResult.startsWith("StateError")) return;
        int collected = extractIntField(collectedResult, "value", 0);
        int total = extractIntField(totalResult, "value", 0);
        int stage = extractIntField(stageResult, "value", 0);
        text.append('\n').append("keepsakes=").append(collected).append('/').append(total).append("  lesson=");
        if (stage <= 0) text.append("tap to explore");
        else if (stage == 1) text.append("find the rest");
        else text.append("garden complete");
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

    private LinearLayout createAiControls() {
        LinearLayout controls = new LinearLayout(this);
        controls.setOrientation(LinearLayout.VERTICAL);
        controls.setPadding(0, dp(8), 0, 0);

        SharedPreferences aiPrefs = getSharedPreferences(AI_PREFS, MODE_PRIVATE);

        aiPromptEditor = new EditText(this);
        aiPromptEditor.setHint("Describe a game change or command. The workspace will inspect, edit, compile, and test it.");
        aiPromptEditor.setSingleLine(false);
        aiPromptEditor.setMinLines(3);
        aiPromptEditor.setTextSize(14.0f);
        aiPromptEditor.setContentDescription("Game change or command for the AI workshop agent");
        controls.addView(aiPromptEditor, fullWidth());

        Button contextToggle = new Button(this);
        contextToggle.setText("Context & Images");
        final LinearLayout contextBody = new LinearLayout(this);
        contextBody.setOrientation(LinearLayout.VERTICAL);
        contextBody.setVisibility(View.GONE);
        contextToggle.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) {
                contextBody.setVisibility(contextBody.getVisibility() == View.VISIBLE
                        ? View.GONE : View.VISIBLE);
            }
        });

        aiAttachmentStatus = new TextView(this);
        aiAttachmentStatus.setTextSize(12.0f);
        aiAttachmentStatus.setTextColor(Color.rgb(73, 84, 100));
        aiAttachmentStatus.setPadding(0, dp(3), 0, dp(2));
        contextBody.addView(aiAttachmentStatus, fullWidth());
        Button sketchLayout = new Button(this);
        sketchLayout.setText("Sketch Layout for AI");
        sketchLayout.setContentDescription("Open a rough paint canvas and attach the saved sketch to the next AI command");
        sketchLayout.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) {
                if (!canModifyImageAssets()) return;
                showPaintEditor(512, 512, null, "ai_layout_sketch", true);
            }
        });
        contextBody.addView(sketchLayout, fullWidth());
        Button reviewAttachments = new Button(this);
        reviewAttachments.setText("Review AI Image Attachments");
        reviewAttachments.setContentDescription("Review or remove project images selected for the next AI request");
        reviewAttachments.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { reviewAiImageAttachments(); }
        });
        contextBody.addView(reviewAttachments, fullWidth());
        refreshAiAttachmentStatus();
        screenshotAttachmentStatus = new TextView(this);
        screenshotAttachmentStatus.setTextSize(12.0f);
        screenshotAttachmentStatus.setTextColor(Color.rgb(73, 84, 100));
        screenshotAttachmentStatus.setPadding(0, dp(3), 0, dp(2));
        screenshotAttachmentStatus.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { reviewPreviewCaptureForAi(); }
        });
        contextBody.addView(screenshotAttachmentStatus, fullWidth());
        Button capturePreview = new Button(this);
        capturePreview.setText("Capture Preview for AI");
        capturePreview.setContentDescription("Capture and review the rendered game preview for the next AI request");
        capturePreview.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { capturePreviewForAi(); }
        });
        contextBody.addView(capturePreview, fullWidth());
        refreshScreenshotAttachmentStatus();
        allowAiImageGeneration = new CheckBox(this);
        allowAiImageGeneration.setText("Allow one low-quality 1024x1024 AI image (~$0.006 plus Sol usage)");
        allowAiImageGeneration.setChecked(false);
        contextBody.addView(allowAiImageGeneration, fullWidth());

        LinearLayout aiActionRow = new LinearLayout(this);
        aiActionRow.setOrientation(LinearLayout.HORIZONTAL);
        Button aiPatch = new Button(this);
        aiPatch.setText("Run");
        aiPatch.setContentDescription("Queue the requested AI change with current reviewed attachments and budget limits");
        aiPatch.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                runAiPatch();
            }
        });
        aiActionRow.addView(aiPatch, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1.0f));
        Button voiceCommand = new Button(this);
        voiceCommand.setText("Voice");
        voiceCommand.setContentDescription("Speak a game change or command");
        voiceCommand.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { startVoiceChange(); }
        });
        aiActionRow.addView(voiceCommand, new LinearLayout.LayoutParams(
                0, LinearLayout.LayoutParams.WRAP_CONTENT, 1.0f));
        aiCancelButton = new Button(this);
        aiCancelButton.setText("Stop");
        aiCancelButton.setVisibility(View.GONE);
        aiCancelButton.setContentDescription("Cancel the active AI run after its current atomic operation");
        aiCancelButton.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { cancelAiRun(); }
        });
        aiActionRow.addView(aiCancelButton, new LinearLayout.LayoutParams(
                0, LinearLayout.LayoutParams.WRAP_CONTENT, 1.0f));
        controls.addView(aiActionRow, fullWidth());

        aiQueueSection = new LinearLayout(this);
        aiQueueSection.setOrientation(LinearLayout.VERTICAL);
        TextView queueTitle = new TextView(this);
        queueTitle.setText("AI Work Queue");
        queueTitle.setTextColor(Color.rgb(35, 45, 60));
        queueTitle.setTypeface(Typeface.DEFAULT_BOLD);
        queueTitle.setPadding(0, dp(6), 0, dp(2));
        aiQueueSection.addView(queueTitle, fullWidth());
        aiQueueBody = new LinearLayout(this);
        aiQueueBody.setOrientation(LinearLayout.VERTICAL);
        aiQueueSection.addView(aiQueueBody, fullWidth());
        controls.addView(aiQueueSection, fullWidth());
        refreshAiQueue();

        LinearLayout progressRow = new LinearLayout(this);
        progressRow.setOrientation(LinearLayout.HORIZONTAL);
        progressRow.setGravity(Gravity.LEFT);
        aiStepPill = createAiProgressPill("step 0/" + MAX_AI_AGENT_TURNS);
        aiActionPill = createAiProgressPill("actions 0");
        aiPhasePill = createAiProgressPill("idle");
        aiElapsedPill = createAiProgressPill("time 0.0s");
        progressRow.addView(aiStepPill);
        progressRow.addView(aiActionPill);
        progressRow.addView(aiPhasePill);
        progressRow.addView(aiElapsedPill);
        HorizontalScrollView progressScroller = new HorizontalScrollView(this);
        progressScroller.setHorizontalScrollBarEnabled(false);
        progressScroller.addView(progressRow);
        controls.addView(progressScroller, fullWidth());

        aiBudgetStatus = new TextView(this);
        aiBudgetStatus.setTextSize(12.0f);
        aiBudgetStatus.setTextColor(Color.rgb(73, 84, 100));
        aiBudgetStatus.setPadding(0, dp(4), 0, dp(2));
        controls.addView(aiBudgetStatus, fullWidth());
        refreshAiBudgetStatus();

        controls.addView(contextToggle, fullWidth());
        controls.addView(contextBody, fullWidth());

        Button moreToolsToggle = new Button(this);
        moreToolsToggle.setText("More Tools & Settings");
        controls.addView(moreToolsToggle, fullWidth());
        final LinearLayout moreToolsBody = new LinearLayout(this);
        moreToolsBody.setOrientation(LinearLayout.VERTICAL);
        moreToolsBody.setVisibility(View.GONE);
        moreToolsToggle.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) {
                moreToolsBody.setVisibility(moreToolsBody.getVisibility() == View.VISIBLE
                        ? View.GONE : View.VISIBLE);
            }
        });

        Button historyToggle = new Button(this);
        historyToggle.setText("Recent Commands");
        historyToggle.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { toggleCommandHistory(); }
        });
        moreToolsBody.addView(historyToggle, fullWidth());
        commandHistoryBody = new LinearLayout(this);
        commandHistoryBody.setOrientation(LinearLayout.VERTICAL);
        commandHistoryBody.setVisibility(View.GONE);
        commandHistoryText = new TextView(this);
        commandHistoryText.setTextSize(12.0f);
        commandHistoryText.setTextColor(Color.rgb(73, 84, 100));
        commandHistoryText.setPadding(dp(8), dp(6), dp(8), dp(6));
        commandHistoryBody.addView(commandHistoryText, fullWidth());
        Button clearHistory = new Button(this);
        clearHistory.setText("Clear Commands + Outcomes");
        clearHistory.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { clearCommandHistory(); }
        });
        commandHistoryBody.addView(clearHistory, fullWidth());
        Button retryLastAi = new Button(this);
        retryLastAi.setText("Retry Last AI");
        retryLastAi.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { retryLastAiRequest(); }
        });
        commandHistoryBody.addView(retryLastAi, fullWidth());
        moreToolsBody.addView(commandHistoryBody, fullWidth());
        refreshCommandHistory();

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
        recordingActions.setOrientation(getResources().getConfiguration().screenWidthDp < 480
                ? LinearLayout.VERTICAL : LinearLayout.HORIZONTAL);
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
        LinearLayout.LayoutParams recordingButtonParams = getResources().getConfiguration().screenWidthDp < 480
                ? fullWidth() : new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1.0f);
        recordingActions.addView(startRecording, recordingButtonParams);
        recordingActions.addView(saveRecording, getResources().getConfiguration().screenWidthDp < 480
                ? fullWidth() : new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1.0f));
        recordingActions.addView(cancelRecording, getResources().getConfiguration().screenWidthDp < 480
                ? fullWidth() : new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1.0f));
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

        Button settingsToggle = new Button(this);
        settingsToggle.setText("AI Settings");
        settingsToggle.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                toggleAiSettings();
            }
        });
        moreToolsBody.addView(settingsToggle, fullWidth());

        aiSettingsBody = new LinearLayout(this);
        aiSettingsBody.setOrientation(LinearLayout.VERTICAL);
        aiSettingsBody.setVisibility(View.GONE);

        TextView providerLabel = new TextView(this);
        providerLabel.setText("Primary AI provider");
        providerLabel.setTextSize(12.0f);
        aiSettingsBody.addView(providerLabel, fullWidth());
        aiProviderSelector = new Spinner(this);
        ArrayAdapter<String> providerAdapter = new ArrayAdapter<>(this,
                android.R.layout.simple_spinner_item,
                Arrays.asList("Codex subscription (on this phone)", "OpenAI API key (fallback)"));
        providerAdapter.setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item);
        aiProviderSelector.setAdapter(providerAdapter);
        String configuredProvider = aiPrefs.getString(AI_PREF_PROVIDER, "");
        if (configuredProvider.isEmpty()) {
            configuredProvider = WorkshopAiProviderPolicy.defaultToCodex(phoneNativeCodexReady)
                    ? AI_PROVIDER_CODEX : AI_PROVIDER_API;
        }
        aiProviderSelector.setSelection(AI_PROVIDER_API.equals(configuredProvider) ? 1 : 0);
        aiSettingsBody.addView(aiProviderSelector, fullWidth());

        codexAccountStatus = new TextView(this);
        codexAccountStatus.setText("Codex account: checking this phone...");
        codexAccountStatus.setTextSize(12.0f);
        codexAccountStatus.setPadding(0, dp(4), 0, dp(4));
        aiSettingsBody.addView(codexAccountStatus, fullWidth());
        Button codexSignIn = new Button(this);
        codexSignIn.setText("Sign in to ChatGPT on this phone");
        codexSignIn.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { beginPhoneNativeCodexLogin(); }
        });
        aiSettingsBody.addView(codexSignIn, fullWidth());

        TextView apiFallbackLabel = new TextView(this);
        apiFallbackLabel.setText("Optional API fallback");
        apiFallbackLabel.setTextSize(12.0f);
        apiFallbackLabel.setPadding(0, dp(8), 0, 0);
        aiSettingsBody.addView(apiFallbackLabel, fullWidth());
        aiApiKeyEditor = new EditText(this);
        aiApiKeyEditor.setHint("OpenAI API key (fallback only)");
        aiApiKeyEditor.setSingleLine(true);
        aiApiKeyEditor.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_PASSWORD);
        aiApiKeyEditor.setText(readSecretPreference(aiPrefs, AI_PREF_API_KEY));
        aiApiKeyEditor.setTextSize(12.0f);
        aiSettingsBody.addView(aiApiKeyEditor, fullWidth());

        aiModelEditor = new EditText(this);
        aiModelEditor.setHint("Model (GPT-5.6 Sol default)");
        aiModelEditor.setContentDescription("OpenAI model; GPT-5.6 Sol defaults to medium reasoning");
        aiModelEditor.setSingleLine(true);
        String configuredModel = aiPrefs.getString(AI_PREF_MODEL, DEFAULT_AI_MODEL);
        if (aiPrefs.getInt(AI_PREF_MODEL_DEFAULT_VERSION, 0) < DEFAULT_AI_MODEL_VERSION) {
            if ("gpt-5.6-terra".equals(configuredModel)) configuredModel = DEFAULT_AI_MODEL;
            aiPrefs.edit().putString(AI_PREF_MODEL, configuredModel)
                    .putInt(AI_PREF_MODEL_DEFAULT_VERSION, DEFAULT_AI_MODEL_VERSION).apply();
        }
        aiModelEditor.setText(configuredModel);
        aiModelEditor.setTextSize(12.0f);
        aiSettingsBody.addView(aiModelEditor, fullWidth());

        TextView reasoningSummary = new TextView(this);
        reasoningSummary.setText("Reasoning: medium");
        reasoningSummary.setTextSize(12.0f);
        aiSettingsBody.addView(reasoningSummary, fullWidth());

        aiMonthlyLimitUsdEditor = new EditText(this);
        aiMonthlyLimitUsdEditor.setHint("Device monthly AI limit USD");
        aiMonthlyLimitUsdEditor.setSingleLine(true);
        aiMonthlyLimitUsdEditor.setText(aiPrefs.getString(AI_PREF_MONTHLY_LIMIT_USD, "5.00"));
        aiSettingsBody.addView(aiMonthlyLimitUsdEditor, fullWidth());
        aiProviderSelector.setOnTouchListener(new View.OnTouchListener() {
            @Override public boolean onTouch(View view, MotionEvent event) {
                if (event.getAction() == MotionEvent.ACTION_DOWN) aiProviderSelectionFromTouch = true;
                return false;
            }
        });
        aiProviderSelector.setOnItemSelectedListener(new AdapterView.OnItemSelectedListener() {
            @Override public void onItemSelected(AdapterView<?> parent, View view, int position, long id) {
                updateAiProviderVisibility();
                SharedPreferences.Editor edit = getSharedPreferences(AI_PREFS, MODE_PRIVATE).edit()
                        .putString(AI_PREF_PROVIDER, position == 1 ? AI_PROVIDER_API : AI_PROVIDER_CODEX);
                if (aiProviderSelectionFromTouch) {
                    edit.putBoolean(AI_PREF_CODEX_PRIMARY_MIGRATION, true);
                }
                aiProviderSelectionFromTouch = false;
                edit.apply();
            }
            @Override public void onNothingSelected(AdapterView<?> parent) { }
        });
        updateAiProviderVisibility();

        Button saveSettings = new Button(this);
        saveSettings.setText("Save AI Settings");
        saveSettings.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                saveAiSettingsFromEditors();
            }
        });
        aiSettingsBody.addView(saveSettings, fullWidth());
        moreToolsBody.addView(aiSettingsBody, fullWidth());
        refreshPhoneNativeCodexStatus();

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
        privacyDisclosure.setText("On-device by default: project code, assets, drafts, recovery, and traces. "
                + "Queue AI Change snapshots the command, workspace context, and only media explicitly selected in review. "
                + "GitHub receives project files only when Sync or PR is pressed. Microphone access is used only for explicit voice or audio-recording actions.");
        privacyDisclosure.setTextSize(12.0f);
        privacyDisclosure.setTextColor(Color.rgb(73, 84, 100));
        privacyDisclosure.setPadding(dp(8), dp(8), dp(8), dp(8));
        privacySettingsBody.addView(privacyDisclosure, fullWidth());
        Button revokeOpenAi = new Button(this);
        revokeOpenAi.setText("Revoke OpenAI API Key");
        revokeOpenAi.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { revokeOpenAiCredential(); }
        });
        privacySettingsBody.addView(revokeOpenAi, fullWidth());
        Button revokeGitHub = new Button(this);
        revokeGitHub.setText("Revoke GitHub Token");
        revokeGitHub.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { revokeGitHubCredential(); }
        });
        privacySettingsBody.addView(revokeGitHub, fullWidth());
        Button clearPendingMedia = new Button(this);
        clearPendingMedia.setText("Clear Pending Media Consent");
        clearPendingMedia.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { clearPendingMediaConsent(); }
        });
        privacySettingsBody.addView(clearPendingMedia, fullWidth());
        Button eraseAiActivity = new Button(this);
        eraseAiActivity.setText("Erase AI Histories + Trace");
        eraseAiActivity.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { confirmEraseAiActivity(); }
        });
        privacySettingsBody.addView(eraseAiActivity, fullWidth());
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
                    startNextQueuedAiIfIdle();
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
        TextView onboardingSummary = new TextView(this);
        onboardingSummary.setText("Manual path (no API key):\n"
                + "1. Tap the Exploration Garden, walk to a keepsake, then open the top-right menu.\n"
                + "2. Open Manual Symbols & Source and choose a symbol.\n"
                + "3. Edit, Apply, then Run Tests; use Changes before backup.\n"
                + "4. Projects creates/switches workshops and exports portable archives.\n\n"
                + "Optional: AI Settings stores an OpenAI key; GitHub Settings stores a token for explicit Sync/PR actions. "
                + "Image/Audio Assets stay under Projects. Voice or audio recording asks for microphone permission only when started.");
        onboardingSummary.setTextSize(12.0f);
        onboardingSummary.setTextColor(Color.rgb(73, 84, 100));
        onboardingSummary.setPadding(dp(8), dp(8), dp(8), dp(8));
        onboardingBody.addView(onboardingSummary, fullWidth());
        Button showWelcome = new Button(this);
        showWelcome.setText("Show Welcome Guide");
        showWelcome.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { showOnboardingGuide(false); }
        });
        onboardingBody.addView(showWelcome, fullWidth());
        Button startManual = new Button(this);
        startManual.setText("Start Zero-AI Manual Tutorial");
        startManual.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { startManualTutorial(); }
        });
        onboardingBody.addView(startManual, fullWidth());
        moreToolsBody.addView(onboardingBody, fullWidth());
        controls.addView(moreToolsBody, fullWidth());
        return controls;
    }

    private String commandHistoryPreferenceKey() {
        String root = projectRootPath == null ? PROJECT_DIR : projectRootPath;
        return AI_PREF_COMMAND_HISTORY_PREFIX + Integer.toHexString(root.hashCode());
    }

    private String outcomeHistoryPreferenceKey() {
        String root = projectRootPath == null ? PROJECT_DIR : projectRootPath;
        return AI_PREF_OUTCOME_HISTORY_PREFIX + Integer.toHexString(root.hashCode());
    }

    private void toggleCommandHistory() {
        if (commandHistoryBody != null) {
            commandHistoryBody.setVisibility(commandHistoryBody.getVisibility() == View.VISIBLE ? View.GONE : View.VISIBLE);
        }
    }

    private void recordCommandHistory(String prompt) {
        SharedPreferences prefs = getSharedPreferences(AI_PREFS, MODE_PRIVATE);
        JSONArray existing;
        try {
            existing = new JSONArray(prefs.getString(commandHistoryPreferenceKey(), "[]"));
        } catch (Exception ignored) {
            existing = new JSONArray();
        }
        JSONArray updated = new JSONArray();
        updated.put(prompt);
        for (int index = 0; index < existing.length() && updated.length() < MAX_COMMAND_HISTORY; index += 1) {
            String prior = existing.optString(index, "").trim();
            if (!prior.isEmpty() && !prior.equals(prompt)) {
                updated.put(prior);
            }
        }
        prefs.edit().putString(commandHistoryPreferenceKey(), updated.toString()).apply();
        refreshCommandHistory();
    }

    private void refreshCommandHistory() {
        if (commandHistoryText == null) {
            return;
        }
        JSONArray history;
        try {
            history = new JSONArray(getSharedPreferences(AI_PREFS, MODE_PRIVATE)
                    .getString(commandHistoryPreferenceKey(), "[]"));
        } catch (Exception ignored) {
            history = new JSONArray();
        }
        StringBuilder text = new StringBuilder();
        if (history.length() == 0) text.append("No commands submitted for this project");
        for (int index = 0; index < history.length(); index += 1) {
            if (text.length() > 0) text.append('\n');
            text.append(index + 1).append(". ").append(history.optString(index, ""));
        }
        JSONArray outcomes = aiOutcomeHistory();
        text.append("\n\nAI outcomes:");
        if (outcomes.length() == 0) text.append(" none");
        for (int index = 0; index < outcomes.length(); index += 1) {
            JSONObject outcome = outcomes.optJSONObject(index);
            if (outcome == null) continue;
            text.append('\n').append(index + 1).append(". ")
                    .append(outcome.optString("status", "unknown"))
                    .append(" - ").append(outcome.optString("summary", ""));
            String usage = outcome.optString("usage", "");
            if (!usage.isEmpty()) text.append(" - ").append(usage);
        }
        commandHistoryText.setText(text.toString());
    }

    private void clearCommandHistory() {
        getSharedPreferences(AI_PREFS, MODE_PRIVATE).edit()
                .remove(commandHistoryPreferenceKey())
                .remove(outcomeHistoryPreferenceKey())
                .apply();
        refreshCommandHistory();
        setStatusText("Command and AI outcome history cleared for this project");
    }

    private JSONArray aiOutcomeHistory() {
        try {
            return new JSONArray(getSharedPreferences(AI_PREFS, MODE_PRIVATE)
                    .getString(outcomeHistoryPreferenceKey(), "[]"));
        } catch (Exception ignored) {
            return new JSONArray();
        }
    }

    private void recordAiOutcome(String prompt, String status, String summary, String usage) {
        boolean terminal = !"started".equals(status);
        try {
            JSONArray existing = aiOutcomeHistory();
            JSONArray updated = new JSONArray();
            updated.put(new JSONObject()
                    .put("timestamp_ms", System.currentTimeMillis())
                    .put("request", prompt == null ? "" : prompt)
                    .put("status", status)
                    .put("summary", summary == null ? "" : summary)
                    .put("usage", usage == null ? "" : usage)
                    .put("trace_path", aiTraceLogPath()));
            int firstPrior = 0;
            JSONObject prior = existing.optJSONObject(0);
            if (!"started".equals(status) && prior != null
                    && "started".equals(prior.optString("status", ""))
                    && (prompt == null ? "" : prompt).equals(prior.optString("request", ""))) {
                firstPrior = 1;
            }
            for (int index = firstPrior; index < existing.length() && updated.length() < MAX_COMMAND_HISTORY; index += 1) {
                updated.put(existing.get(index));
            }
            getSharedPreferences(AI_PREFS, MODE_PRIVATE).edit()
                    .putString(outcomeHistoryPreferenceKey(), updated.toString()).apply();
            refreshCommandHistory();
            finishActiveAiQueueItem(status, summary);
        } catch (Exception ignored) {
            // Outcome history must not interfere with AI execution or source recovery.
        } finally {
            if (terminal && WorkshopLongWorkCoordinator.isAiActive()) {
                aiRunActive = false;
                WorkshopLongWorkCoordinator.finishAi(this);
                if (aiCancelButton != null) aiCancelButton.setVisibility(View.GONE);
            }
        }
    }

    private void retryLastAiRequest() {
        JSONArray outcomes = aiOutcomeHistory();
        for (int index = 0; index < outcomes.length(); index += 1) {
            JSONObject outcome = outcomes.optJSONObject(index);
            String request = outcome == null ? "" : outcome.optString("request", "").trim();
            if (!request.isEmpty()) {
                aiPromptEditor.setText(request);
                setStatusText("Retrying last AI request as a new budget-checked run");
                runAiPatch();
                return;
            }
        }
        setStatusText("No AI request is available to retry");
    }

    private boolean needsFirstRunAiSetup() {
        SharedPreferences onboarding = getSharedPreferences(ONBOARDING_PREFS, MODE_PRIVATE);
        if (onboarding.getBoolean(AI_SETUP_COMPLETE, false)) return false;
        SharedPreferences ai = getSharedPreferences(AI_PREFS, MODE_PRIVATE);
        if (!ai.getString(AI_PREF_PROVIDER, "").isEmpty()
                || !readSecretPreference(ai, AI_PREF_API_KEY).isEmpty()) {
            onboarding.edit().putBoolean(AI_SETUP_COMPLETE, true).apply();
            return false;
        }
        return true;
    }

    private void markAiSetupComplete() {
        getSharedPreferences(ONBOARDING_PREFS, MODE_PRIVATE).edit()
                .putBoolean(AI_SETUP_COMPLETE, true).apply();
    }

    private void showFirstRunAiSetup() {
        final EditText apiKey = new EditText(this);
        apiKey.setHint("Optional OpenAI API key");
        apiKey.setSingleLine(true);
        apiKey.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_PASSWORD);
        apiKey.setPadding(dp(20), dp(8), dp(20), dp(8));
        new AlertDialog.Builder(this)
                .setTitle("Set up AI")
                .setMessage("Sign in with your ChatGPT subscription on this phone, or save an API key as the fallback. You can change both later in Settings.")
                .setView(apiKey)
                .setPositiveButton("ChatGPT Sign-in", new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        getSharedPreferences(AI_PREFS, MODE_PRIVATE).edit()
                                .putString(AI_PREF_PROVIDER, AI_PROVIDER_CODEX).apply();
                        if (aiProviderSelector != null) aiProviderSelector.setSelection(0);
                        markAiSetupComplete();
                        refreshAiBudgetStatus();
                        showProjectChooserAfterCodexLogin = true;
                        beginPhoneNativeCodexLogin();
                    }
                })
                .setNeutralButton("Save API Key", new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        String key = apiKey.getText().toString().trim();
                        if (key.isEmpty() || !saveAiSettings(key, DEFAULT_AI_MODEL)) {
                            setStatusText("Enter a valid API key, or choose ChatGPT sign-in / without AI");
                            gameLoopHandler.post(new Runnable() {
                                @Override public void run() { showFirstRunAiSetup(); }
                            });
                            return;
                        }
                        getSharedPreferences(AI_PREFS, MODE_PRIVATE).edit()
                                .putString(AI_PREF_PROVIDER, AI_PROVIDER_API).apply();
                        if (aiProviderSelector != null) aiProviderSelector.setSelection(1);
                        markAiSetupComplete();
                        refreshAiBudgetStatus();
                        showProjectChooser();
                    }
                })
                .setNegativeButton("Without AI", new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        markAiSetupComplete();
                        showProjectChooser();
                    }
                })
                .setCancelable(false)
                .show();
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
        if (aiRunActive || githubOperationActive || projectIoActive || audioRecordingActive
                || pendingExportProject != null || !pendingImportProjectName.isEmpty()) {
            setStatusText("Project creation blocked while AI, GitHub, or project I/O is active");
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

    private boolean activateProject(WorkshopProjectRegistry.ProjectInfo project) {
        if (aiRunActive || githubOperationActive || projectIoActive || audioRecordingActive
                || pendingExportProject != null || !pendingImportProjectName.isEmpty()) {
            setStatusText("Project switch blocked while AI, GitHub, or project I/O is active");
            return false;
        }
        if (hasPendingSourceEdit()) {
            setStatusText("Apply or Reset the pending source edit before switching projects");
            return false;
        }
        try {
            WorkshopProjectRegistry.setActive(this, project);
            activeProject = project;
            projectRootFile = project.root;
            projectRootPath = project.root.getAbsolutePath();
            clearPendingPreviewCapture();
            stopAudioPreview();
            cancelAudioRecording(false);
            selectedSymbol = null;
            diagnosticFile = "";
            diagnosticSymbol = "";
            compileAttempted = false;
            compileReady = false;
            lastCompileResult = "CompileNotRun";
            reviewedGitHubChangeFingerprint = "";
            ProjectSnapshot snapshot = loadBundledProject();
            ensureActiveProjectBaseline(snapshot);
            rebuildSymbolList(snapshot);
            if (snapshot.firstSymbol != null) showSymbol(snapshot.firstSymbol);
            refreshChangeSummary(snapshot);
            refreshCommandHistory();
            refreshAiQueue();
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
            setStatusText(compileReady ? "Working on " + project.name
                    : "Unable to run " + project.name + " - " + compileResult);
            gameLoopHandler.post(new Runnable() {
                @Override public void run() { startNextQueuedAiIfIdle(); }
            });
            return true;
        } catch (Exception error) {
            setStatusText("Project switch failed: " + error.getMessage());
            return false;
        }
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
        if (aiRunActive || githubOperationActive || projectIoActive || audioRecordingActive
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
        if (aiRunActive || githubOperationActive || projectIoActive || audioRecordingActive
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
        String token = githubTokenEditor == null ? "" : githubTokenEditor.getText().toString().trim();
        String repository = githubRepositoryEditor == null ? "" : githubRepositoryEditor.getText().toString().trim();
        String branch = githubBranchEditor == null ? "" : githubBranchEditor.getText().toString().trim();
        if (token.isEmpty() || repository.indexOf('/') <= 0 || repository.endsWith("/")) {
            setStatusText("GitHub sync settings need a token and owner/repository");
            return;
        }
        SharedPreferences preferences = getSharedPreferences(GITHUB_PREFS, MODE_PRIVATE);
        if (!writeSecretPreference(preferences, GITHUB_PREF_TOKEN, token)) return;
        preferences.edit()
                .putString(githubProjectPreferenceKey(GITHUB_PREF_REPOSITORY), repository)
                .putString(githubProjectPreferenceKey(GITHUB_PREF_BRANCH), branch.isEmpty() ? "main" : branch)
                .apply();
        refreshGitHubSyncStatus();
        setStatusText("GitHub sync settings saved; background sync is ready");
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
        String operation = prefs.getString(githubProjectPreferenceKey(GITHUB_PREF_OPERATION), "");
        String state = prefs.getString(githubProjectPreferenceKey(GITHUB_PREF_OPERATION_STATE), "");
        String detail = prefs.getString(githubProjectPreferenceKey(GITHUB_PREF_OPERATION_DETAIL), "");
        if (("queued".equals(state) || "running".equals(state)) && !operation.isEmpty()) {
            persistGitHubOperationState(operation, "interrupted", "app stopped before completion");
            githubSyncStatus.setText("GitHub sync: interrupted; retry available");
            return;
        }
        if (("error".equals(state) || "interrupted".equals(state)) && !operation.isEmpty()) {
            githubSyncStatus.setText("GitHub sync: retry available" + (detail.isEmpty() ? "" : " - " + detail));
            return;
        }
        githubSyncStatus.setText("GitHub sync: ready for " + repository);
    }

    private void queueGitHubSync() {
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
        if (!beginGitHubOperation("sync", "GitHub sync: queued")) return;
        githubSyncExecutor.submit(new Runnable() {
            @Override public void run() {
                try {
                    Map<String, byte[]> files = githubBackupFiles();
                    if (files.isEmpty()) {
                        postGitHubOperationState("", "complete", "GitHub sync: no project files");
                        return;
                    }
                    int completed = 0;
                    for (Map.Entry<String, byte[]> entry : files.entrySet()) {
                        completed += 1;
                        postGitHubOperationState("sync", "running", "GitHub sync: " + completed + "/" + files.size());
                        uploadGitHubFile(token, repository, branch, entry.getKey(), entry.getValue());
                    }
                    postGitHubOperationState("", "complete", "GitHub sync: complete (" + completed + " files)");
                } catch (final Exception error) {
                    postGitHubOperationState("sync", "error", "GitHub sync error: " + error.getMessage());
                }
            }
        });
    }

    private void enqueuePendingAiRequest(String source) {
        String prompt = aiPromptEditor == null ? "" : aiPromptEditor.getText().toString().trim();
        if (prompt.isEmpty()) {
            setStatusText("AI queue needs a request");
            return;
        }
        try {
            List<WorkshopImageAssets.AssetInfo> images = selectedAiImageInfos();
            JSONArray metadata = aiImageMetadata(images);
            JSONObject logical = attachPreviewLogicalSnapshot && pendingPreviewLogicalSnapshot != null
                    ? new JSONObject(pendingPreviewLogicalSnapshot.toString()) : null;
            boolean imageGeneration = allowAiImageGeneration != null && allowAiImageGeneration.isChecked();
            Bitmap preview = attachPreviewPixels ? pendingPreviewScreenshot : null;
            if (attachPreviewPixels && preview == null) throw new IOException("selected preview pixels are unavailable");
            if (preview != null && preview.isRecycled()) throw new IOException("selected preview pixels are unavailable");
            byte[] previewPng = preview == null ? null : encodeBitmapPng(preview);
            AndroidAiQueue.enqueue(this, activeRecoveryProjectId(), source, prompt, metadata, logical,
                    imageGeneration, previewPng, preview == null ? 0 : preview.getWidth(),
                    preview == null ? 0 : preview.getHeight());
            recordCommandHistory(prompt);
            if (imageGeneration) allowAiImageGeneration.setChecked(false);
            refreshAiQueue();
            setStatusText("AI request queued behind the active item");
        } catch (Exception error) {
            setStatusText("AI queue failed: " + error.getMessage());
        }
    }

    private List<WorkshopImageAssets.AssetInfo> aiImageInfosForQueueEntry(AndroidAiQueue.Entry entry)
            throws IOException {
        if (entry.imageAttachments.length() == 0) return Collections.emptyList();
        if (activeProject == null || !entry.projectId.equals(activeRecoveryProjectId())) {
            throw new IOException("queued image request does not belong to the active project");
        }
        Map<String, WorkshopImageAssets.AssetInfo> available = new LinkedHashMap<>();
        for (WorkshopImageAssets.AssetInfo image : WorkshopImageAssets.list(activeProject.root)) {
            available.put(image.relativePath, image);
        }
        ArrayList<WorkshopImageAssets.AssetInfo> result = new ArrayList<>();
        for (int index = 0; index < entry.imageAttachments.length(); index += 1) {
            JSONObject expected = entry.imageAttachments.optJSONObject(index);
            String path = expected == null ? "" : expected.optString("project_path", "");
            WorkshopImageAssets.AssetInfo actual = available.get(path);
            if (actual == null || actual.width != expected.optInt("width", -1)
                    || actual.height != expected.optInt("height", -1)
                    || actual.bytes != expected.optLong("bytes", -1L)
                    || !sha256Bytes(WorkshopImageAssets.readForSync(actual))
                            .equals(expected.optString("sha256", ""))) {
                throw new IOException("queued image changed or disappeared: " + path);
            }
            result.add(actual);
        }
        return Collections.unmodifiableList(result);
    }

    private void refreshAiQueue() {
        if (aiQueueBody == null) return;
        aiQueueBody.removeAllViews();
        try {
            List<AndroidAiQueue.Entry> items = AndroidAiQueue.list(this, activeRecoveryProjectId());
            boolean hasActiveItems = false;
            for (AndroidAiQueue.Entry item : items) {
                if (AndroidAiQueue.PENDING.equals(item.state)
                        || AndroidAiQueue.IN_PROGRESS.equals(item.state)) {
                    hasActiveItems = true;
                    break;
                }
            }
            if (!hasActiveItems) {
                if (aiQueueSection != null) aiQueueSection.setVisibility(View.GONE);
                return;
            }
            if (aiQueueSection != null) aiQueueSection.setVisibility(View.VISIBLE);
            for (int index = 0; index < items.size(); index += 1) {
                final AndroidAiQueue.Entry item = items.get(index);
                if (!AndroidAiQueue.PENDING.equals(item.state)
                        && !AndroidAiQueue.IN_PROGRESS.equals(item.state)) continue;
                LinearLayout row = new LinearLayout(this);
                row.setOrientation(LinearLayout.HORIZONTAL);
                TextView label = new TextView(this);
                String prompt = item.prompt.length() > 72 ? item.prompt.substring(0, 69) + "..." : item.prompt;
                String detail = item.detail.isEmpty() ? "" : "\n" + item.detail;
                label.setText(item.state.replace('_', ' ') + " · " + item.source + " · " + prompt + detail);
                label.setTextColor(Color.rgb(73, 84, 100));
                label.setContentDescription("AI queue item " + item.state + " from " + item.source + ": " + item.prompt);
                row.addView(label, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1.0f));
                if (AndroidAiQueue.PENDING.equals(item.state)) {
                    Button cancel = new Button(this);
                    cancel.setText("Cancel");
                    cancel.setContentDescription("Cancel pending AI request " + item.prompt);
                    cancel.setOnClickListener(new View.OnClickListener() {
                        @Override public void onClick(View view) { cancelPendingAiItem(item); }
                    });
                    row.addView(cancel, new LinearLayout.LayoutParams(
                            LinearLayout.LayoutParams.WRAP_CONTENT, LinearLayout.LayoutParams.WRAP_CONTENT));
                }
                aiQueueBody.addView(row, fullWidth());
            }
        } catch (Exception error) {
            if (aiQueueSection != null) aiQueueSection.setVisibility(View.VISIBLE);
            TextView unavailable = new TextView(this);
            unavailable.setText("AI queue unavailable: " + error.getMessage());
            aiQueueBody.addView(unavailable, fullWidth());
        }
    }

    private void cancelPendingAiItem(AndroidAiQueue.Entry item) {
        try {
            if (!item.projectId.equals(activeRecoveryProjectId())
                    || !AndroidAiQueue.cancelPending(this, item.projectId, item.id)) {
                setStatusText("AI queue item is no longer pending");
            } else {
                setStatusText("Pending AI request cancelled before any API call");
            }
        } catch (Exception error) {
            setStatusText("AI queue cancellation failed: " + error.getMessage());
        }
        refreshAiQueue();
    }

    private void finishActiveAiQueueItem(String outcomeStatus, String detail) {
        AndroidAiQueue.Entry item = activeAiQueueEntry;
        if (item == null || "started".equals(outcomeStatus)) return;
        String terminal = ("complete".equals(outcomeStatus) || "applied".equals(outcomeStatus))
                ? AndroidAiQueue.COMPLETED
                : ("cancelled".equals(outcomeStatus) ? AndroidAiQueue.CANCELLED : AndroidAiQueue.FAILED);
        try {
            AndroidAiQueue.finish(this, item.projectId, item.id, terminal, detail);
        } catch (Exception error) {
            setStatusText("AI queue transition failed: " + error.getMessage());
            return;
        }
        activeAiQueueEntry = null;
        refreshAiQueue();
        gameLoopHandler.postDelayed(new Runnable() {
            @Override public void run() { startNextQueuedAiIfIdle(); }
        }, 100L);
    }

    private void startNextQueuedAiIfIdle() {
        if (restartLoopRecoveryActive) return;
        if (aiRunActive || WorkshopLongWorkCoordinator.isAnyActive()
                || activeAiQueueEntry != null || audioRecordingActive) return;
        try {
            boolean hasPending = false;
            for (AndroidAiQueue.Entry item : AndroidAiQueue.list(this, activeRecoveryProjectId())) {
                if (AndroidAiQueue.PENDING.equals(item.state)) {
                    hasPending = true;
                    break;
                }
            }
            if (!hasPending) {
                refreshAiQueue();
                return;
            }
            if (WorkshopBackgroundWorkPolicy.decide(true,
                    WorkshopConnectivity.hasUsableNetwork(this), false, false)
                    == WorkshopBackgroundWorkPolicy.Decision.WAIT_FOR_NETWORK) {
                setStatusText("AI work is waiting for an internet connection");
                refreshAiQueue();
                return;
            }
            AndroidAiQueue.Entry next = AndroidAiQueue.claimNext(this, activeRecoveryProjectId());
            if (next == null) {
                refreshAiQueue();
                return;
            }
            refreshAiQueue();
            runAiPatch(next.source, next);
        } catch (Exception error) {
            setStatusText("AI queue could not start the next item: " + error.getMessage());
            refreshAiQueue();
        }
    }

    private void failQueuedAiPreflight(AndroidAiQueue.Entry entry, String detail) {
        if (entry == null) return;
        try {
            AndroidAiQueue.finish(this, entry.projectId, entry.id, AndroidAiQueue.FAILED, detail);
        } catch (Exception ignored) {
            // The visible queue error remains available for recovery on the next app start.
        }
        if (activeAiQueueEntry != null && activeAiQueueEntry.id.equals(entry.id)) activeAiQueueEntry = null;
        refreshAiQueue();
        gameLoopHandler.postDelayed(new Runnable() {
            @Override public void run() { startNextQueuedAiIfIdle(); }
        }, 100L);
    }

    private void revokeOpenAiCredential() {
        if (aiRunActive) {
            setStatusText("OpenAI key revocation blocked until the active AI run is cancelled or complete");
            return;
        }
        new AlertDialog.Builder(this)
                .setTitle("Revoke OpenAI API Key?")
                .setMessage("The encrypted credential is removed from this installation. Project files are unchanged.")
                .setPositiveButton("Revoke", new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        SharedPreferences preferences = getSharedPreferences(AI_PREFS, MODE_PRIVATE);
                        if (!writeSecretPreference(preferences, AI_PREF_API_KEY, "")) return;
                        if (aiApiKeyEditor != null) aiApiKeyEditor.setText("");
                        setStatusText("OpenAI API key revoked from encrypted storage");
                    }
                })
                .setNegativeButton("Cancel", null)
                .show();
    }

    private void revokeGitHubCredential() {
        if (githubOperationActive) {
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

    private void clearPendingMediaConsent() {
        if (aiRunActive) {
            setStatusText("Pending media cannot change during an active AI run");
            return;
        }
        selectedImageAssets.clear();
        clearPendingPreviewCapture();
        if (allowAiImageGeneration != null) allowAiImageGeneration.setChecked(false);
        refreshImageAssetList();
        refreshAiAttachmentStatus();
        refreshScreenshotAttachmentStatus();
        setStatusText("Pending image, screenshot, logical snapshot, and generation consent cleared");
    }

    private void confirmEraseAiActivity() {
        if (aiRunActive) {
            setStatusText("AI history erase blocked until the active run is cancelled or complete");
            return;
        }
        new AlertDialog.Builder(this)
                .setTitle("Erase AI Histories and Trace?")
                .setMessage("This removes command/outcome history for every project, usage records, monthly spend history, and the local AI trace. Code and assets remain.")
                .setPositiveButton("Erase", new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        eraseAiActivity();
                    }
                })
                .setNegativeButton("Cancel", null)
                .show();
    }

    private void eraseAiActivity() {
        SharedPreferences preferences = getSharedPreferences(AI_PREFS, MODE_PRIVATE);
        SharedPreferences.Editor editor = preferences.edit()
                .remove(AI_PREF_LAST_USAGE)
                .remove(AI_PREF_MONTH_KEY)
                .remove(AI_PREF_MONTH_SPEND_USD);
        for (String key : preferences.getAll().keySet()) {
            if (key.startsWith(AI_PREF_COMMAND_HISTORY_PREFIX)
                    || key.startsWith(AI_PREF_OUTCOME_HISTORY_PREFIX)) editor.remove(key);
        }
        if (!editor.commit()) {
            setStatusText("AI history erase failed: preferences commit failed");
            return;
        }
        try {
            AndroidAiQueue.clearAll(this);
        } catch (Exception error) {
            setStatusText("AI histories erased but queued work deletion failed: " + error.getMessage());
            return;
        }
        File trace = aiTraceLogFile();
        if (!trace.delete() && trace.exists()) {
            setStatusText("AI histories erased but trace deletion failed");
            return;
        }
        clearPendingMediaConsent();
        refreshCommandHistory();
        refreshAiQueue();
        refreshAiBudgetStatus();
        setStatusText("AI histories, queue, usage records, monthly spend history, trace, and pending media erased");
    }

    private void requestSupportBundleExport() {
        if (aiRunActive || githubOperationActive || projectIoActive) {
            setStatusText("Support export blocked while background work is active");
            return;
        }
        new AlertDialog.Builder(this)
                .setTitle("Export Redacted Support Bundle?")
                .setMessage("Includes app/device versions, project file counts, compile/reload state, operation states, "
                        + "AI outcome statuses, up to 50 trace event names, and prior redacted crash type/class-method frames. Excludes credentials, source, prompts, "
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
        final JSONArray outcomes = aiOutcomeHistory();
        SharedPreferences github = getSharedPreferences(GITHUB_PREFS, MODE_PRIVATE);
        final String operation = github.getString(githubProjectPreferenceKey(GITHUB_PREF_OPERATION), "");
        final String state = github.getString(githubProjectPreferenceKey(GITHUB_PREF_OPERATION_STATE), "");
        if (!beginProjectIoWork("Building a redacted support bundle")) return;
        setStatusText("Building redacted support bundle");
        projectIoExecutor.submit(new Runnable() {
            @Override public void run() {
                try {
                    String bundle = AndroidSupportBundle.build(MainActivity.this, project, root, compile,
                            operation, state, outcomes, aiTraceLogFile());
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
        if (aiRunActive || githubOperationActive || projectIoActive || audioRecordingActive) {
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
                        + "trash, baseline, draft/recovery journal, and project-scoped AI/GitHub state. The bundled project and credentials remain.")
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
            AndroidAiQueue.clearProject(this, target.id);
            clearDeletedProjectPreferences(target);
            refreshProjectControls();
            setStatusText("Deleted project and scoped private data: " + target.name + "; Bundled Workshop is active");
        } catch (Exception error) {
            refreshProjectControls();
            setStatusText("Project deletion stopped with recovery context preserved where possible: " + error.getMessage());
        }
    }

    private void clearDeletedProjectPreferences(WorkshopProjectRegistry.ProjectInfo project) {
        String historyIdentity = Integer.toHexString(project.root.getAbsolutePath().hashCode());
        getSharedPreferences(AI_PREFS, MODE_PRIVATE).edit()
                .remove(AI_PREF_COMMAND_HISTORY_PREFIX + historyIdentity)
                .remove(AI_PREF_OUTCOME_HISTORY_PREFIX + historyIdentity)
                .apply();
        SharedPreferences github = getSharedPreferences(GITHUB_PREFS, MODE_PRIVATE);
        SharedPreferences.Editor editor = github.edit();
        String suffix = "_" + project.id;
        for (String key : github.getAll().keySet()) if (key.endsWith(suffix)) editor.remove(key);
        editor.apply();
    }

    private void showOnboardingGuide(boolean firstRun) {
        new AlertDialog.Builder(this)
                .setTitle("Welcome to Stasis Workshop")
                .setMessage("You can build and test a game entirely on-device without AI. In the Exploration Garden, tap a destination "
                        + "and collect a keepsake, then open the menu, "
                        + "expand Manual Symbols & Source, make a small edit, Apply it, and Run Tests. Projects and archive backup "
                        + "work without accounts. OpenAI, GitHub, media, and voice are optional and activate only when you choose them.")
                .setPositiveButton("Start Manual Tutorial", new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        markOnboardingSeen();
                        startManualTutorial();
                    }
                })
                .setNegativeButton("Got It", new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        markOnboardingSeen();
                        setStatusText("Welcome guide completed; Help & Onboarding remains available");
                    }
                })
                .setNeutralButton(firstRun ? "Remind Me Later" : "Close", null)
                .show();
    }

    private void markOnboardingSeen() {
        getSharedPreferences(ONBOARDING_PREFS, MODE_PRIVATE).edit()
                .putBoolean(ONBOARDING_COMPLETE, true).apply();
    }

    private void startManualTutorial() {
        markOnboardingSeen();
        if (editorPanel != null && editorPanel.getVisibility() != View.VISIBLE) toggleEditorPanel();
        if (manualEditBody != null) manualEditBody.setVisibility(View.VISIBLE);
        if (onboardingBody != null) onboardingBody.setVisibility(View.VISIBLE);
        if (selectedSymbol == null) {
            ProjectSnapshot project = loadBundledProject();
            if (project.firstSymbol != null) showSymbol(project.firstSymbol);
        }
        setStatusText("Manual tutorial: try MOVE_SPEED in src/config.stasis, tap Apply, then Run Tests; no API key is required");
        if (editorPanel != null && sourceEditor != null) {
            editorPanel.post(new Runnable() {
                @Override public void run() { editorPanel.smoothScrollTo(0, sourceEditor.getTop()); }
            });
        }
    }

    private void markInterruptedAiOutcomeIfNeeded() {
        try {
            int recovered = AndroidAiQueue.recoverInterrupted(this, activeRecoveryProjectId());
            if (recovered > 0) refreshAiQueue();
        } catch (Exception error) {
            setStatusText("AI queue recovery failed: " + error.getMessage());
        }
        JSONArray outcomes = aiOutcomeHistory();
        JSONObject latest = outcomes.optJSONObject(0);
        if (latest == null || !"started".equals(latest.optString("status", ""))) return;
        String request = latest.optString("request", "");
        recordAiOutcome(request, "interrupted",
                "App stopped before AI completion; Retry Last AI starts a new budget-checked run",
                "A paid in-flight call may have completed remotely");
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
        if (aiPromptEditor != null) aiPromptEditor.setText(state.getString("ai_prompt", ""));
        voiceTranscript = state.getString("voice_transcript", "");
        SymbolEntry restoredSymbol = findSymbolByIdentity(loadBundledProject(),
                state.getString("selected_kind", ""), state.getString("selected_file", ""),
                state.getString("selected_owner", ""), state.getString("selected_name", ""));
        if (restoredSymbol != null) showSymbol(restoredSymbol);
        restoreVisibility(manualEditBody, state.getBoolean("manual_open", false));
        restoreVisibility(projectSettingsBody, state.getBoolean("projects_open", false));
        restoreVisibility(commandHistoryBody, state.getBoolean("history_open", false));
        restoreVisibility(aiSettingsBody, state.getBoolean("ai_settings_open", false));
        restoreVisibility(githubSettingsBody, state.getBoolean("github_settings_open", false));
        restoreVisibility(privacySettingsBody, state.getBoolean("privacy_open", false));
        restoreVisibility(onboardingBody, state.getBoolean("onboarding_open", false));
        ArrayList<String> selectedPaths = state.getStringArrayList("selected_image_paths");
        selectedImageAssets.clear();
        if (selectedPaths != null) selectedImageAssets.addAll(selectedPaths);
        ArrayList<String> designSketchPaths = state.getStringArrayList("selected_design_sketch_paths");
        selectedDesignSketchAssets.clear();
        if (designSketchPaths != null) selectedDesignSketchAssets.addAll(designSketchPaths);
        selectedImageAssetProjectId = activeProject == null ? "" : activeProject.id;
        refreshImageAssetList();
        refreshAiAttachmentStatus();
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
        clearPendingPreviewCapture();
        if (allowAiImageGeneration != null) allowAiImageGeneration.setChecked(false);
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
        Map<String, String> before = sourcesByFile(baseline);
        Map<String, String> after = sourcesByFile(current);
        Map<String, String> changed = new LinkedHashMap<>();
        for (Map.Entry<String, String> entry : after.entrySet()) {
            if (!entry.getValue().equals(before.get(entry.getKey()))) {
                changed.put(entry.getKey(), entry.getValue());
            }
        }
        return changed;
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
                    ensureGitHubReviewBranch(token, repository, baseBranch, reviewBranch);
                    int completed = 0;
                    for (Map.Entry<String, String> entry : changes.entrySet()) {
                        completed += 1;
                        postGitHubOperationState("pull_request", "running", "GitHub pull request: uploading " + completed + "/" + changes.size());
                        uploadGitHubFile(token, repository, reviewBranch, entry.getKey(), entry.getValue());
                    }
                    String url = createOrFindGitHubPullRequest(token, repository, baseBranch, reviewBranch,
                            formatGitHubPullRequestBody(changes));
                    postGitHubOperationState("", "complete", "GitHub pull request: ready " + url);
                } catch (Exception error) {
                    postGitHubOperationState("pull_request", "error", "GitHub pull request error: " + error.getMessage());
                }
            }
        });
    }

    private static String githubChangeFingerprint(Map<String, String> changes) {
        StringBuilder fingerprint = new StringBuilder();
        for (Map.Entry<String, String> entry : changes.entrySet()) {
            fingerprint.append(entry.getKey()).append('\n').append(entry.getValue()).append('\n');
        }
        try {
            byte[] digest = MessageDigest.getInstance("SHA-256")
                    .digest(fingerprint.toString().getBytes(StandardCharsets.UTF_8));
            StringBuilder hex = new StringBuilder(digest.length * 2);
            String digits = "0123456789abcdef";
            for (byte value : digest) {
                int unsigned = value & 0xff;
                hex.append(digits.charAt(unsigned >>> 4));
                hex.append(digits.charAt(unsigned & 0x0f));
            }
            return hex.toString();
        } catch (NoSuchAlgorithmException unavailable) {
            return fingerprint.toString();
        }
    }

    private String githubReviewBranchName() {
        String identity = activeProject == null
                ? Integer.toHexString(projectRootPath().hashCode()) : activeProject.id;
        return "stasis-workshop-" + identity;
    }

    private static String formatGitHubPullRequestBody(Map<String, String> changes) {
        StringBuilder body = new StringBuilder("Updated from Stasis Workshop for Android.\n\nChanged files:");
        for (String path : changes.keySet()) body.append("\n- `").append(path).append('`');
        return body.toString();
    }

    private static void ensureGitHubReviewBranch(String token, String repository, String baseBranch, String reviewBranch) throws Exception {
        String reviewRefUrl = githubApiUrl(repository, "/git/ref/heads/" + encodeGitHubPath(reviewBranch));
        int reviewCode = githubGetCode(token, reviewRefUrl);
        if (reviewCode == 200) return;
        if (reviewCode != 404) throw new IOException("review branch HTTP " + reviewCode);

        JSONObject baseRef = githubGetJson(token,
                githubApiUrl(repository, "/git/ref/heads/" + encodeGitHubPath(baseBranch)));
        String baseSha = baseRef.optJSONObject("object") == null
                ? "" : baseRef.optJSONObject("object").optString("sha", "");
        if (baseSha.isEmpty()) throw new IOException("base branch has no commit SHA");
        githubWriteJson(token, "POST", githubApiUrl(repository, "/git/refs"),
                new JSONObject().put("ref", "refs/heads/" + reviewBranch).put("sha", baseSha), 201);
    }

    private static String createOrFindGitHubPullRequest(String token, String repository, String baseBranch,
            String reviewBranch, String body) throws Exception {
        String owner = repository.substring(0, repository.indexOf('/'));
        String query = "?state=open&head=" + encodeGitHubQuery(owner + ":" + reviewBranch)
                + "&base=" + encodeGitHubQuery(baseBranch);
        JSONArray existing = githubGetArray(token, githubApiUrl(repository, "/pulls" + query));
        if (existing.length() > 0) return existing.getJSONObject(0).optString("html_url", "existing PR");
        JSONObject created = githubWriteJson(token, "POST", githubApiUrl(repository, "/pulls"),
                new JSONObject().put("title", "Stasis Workshop Android changes")
                        .put("head", reviewBranch).put("base", baseBranch).put("body", body), 201);
        return created.optString("html_url", "created PR");
    }

    private static String githubApiUrl(String repository, String path) {
        return "https://api.github.com/repos/" + repository + path;
    }

    private static String encodeGitHubPath(String value) throws Exception {
        return encodeGitHubQuery(value).replace("%2F", "/");
    }

    private static String encodeGitHubQuery(String value) throws Exception {
        return URLEncoder.encode(value, "UTF-8").replace("+", "%20");
    }

    private static void configureGitHubConnection(HttpURLConnection connection, String token) {
        connection.setConnectTimeout(GITHUB_NETWORK_TIMEOUT_MS);
        connection.setReadTimeout(GITHUB_NETWORK_TIMEOUT_MS);
        connection.setRequestProperty("Accept", "application/vnd.github+json");
        connection.setRequestProperty("Authorization", "Bearer " + token);
        connection.setRequestProperty("X-GitHub-Api-Version", "2022-11-28");
    }

    private static int githubGetCode(String token, String url) throws Exception {
        HttpURLConnection connection = (HttpURLConnection)new URL(url).openConnection();
        configureGitHubConnection(connection, token);
        int code = connection.getResponseCode();
        connection.disconnect();
        return code;
    }

    private static JSONObject githubGetJson(String token, String url) throws Exception {
        return new JSONObject(githubRead(token, url));
    }

    private static JSONArray githubGetArray(String token, String url) throws Exception {
        return new JSONArray(githubRead(token, url));
    }

    private static String githubRead(String token, String url) throws Exception {
        HttpURLConnection connection = (HttpURLConnection)new URL(url).openConnection();
        configureGitHubConnection(connection, token);
        int code = connection.getResponseCode();
        if (code != 200) throw new IOException("GitHub read HTTP " + code);
        String response = readStreamStatic(connection.getInputStream());
        connection.disconnect();
        return response;
    }

    private static JSONObject githubWriteJson(String token, String method, String url, JSONObject body,
            int expectedCode) throws Exception {
        HttpURLConnection connection = (HttpURLConnection)new URL(url).openConnection();
        connection.setRequestMethod(method);
        connection.setDoOutput(true);
        configureGitHubConnection(connection, token);
        connection.setRequestProperty("Content-Type", "application/json");
        OutputStream output = connection.getOutputStream();
        output.write(body.toString().getBytes(StandardCharsets.UTF_8));
        output.close();
        int code = connection.getResponseCode();
        if (code != expectedCode) throw new IOException("GitHub write HTTP " + code);
        String response = readStreamStatic(connection.getInputStream());
        connection.disconnect();
        return new JSONObject(response);
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

    private void persistGitHubOperationState(String operation, String state, String detail) {
        getSharedPreferences(GITHUB_PREFS, MODE_PRIVATE).edit()
                .putString(githubProjectPreferenceKey(GITHUB_PREF_OPERATION), operation)
                .putString(githubProjectPreferenceKey(GITHUB_PREF_OPERATION_STATE), state)
                .putString(githubProjectPreferenceKey(GITHUB_PREF_OPERATION_DETAIL), detail)
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
    }

    private synchronized boolean beginGitHubOperation(String operation, String status) {
        if (githubOperationActive) {
            githubSyncStatus.setText("GitHub sync: another operation is already queued or running");
            return false;
        }
        if (!WorkshopLongWorkCoordinator.beginGitHub(this, "Syncing reviewed project files")) {
            githubSyncStatus.setText("GitHub sync: another foreground operation is active");
            return false;
        }
        githubOperationActive = true;
        postGitHubOperationState(operation, "queued", status);
        return true;
    }

    private void postGitHubOperationState(final String operation, final String state, final String status) {
        persistGitHubOperationState(operation, state, status);
        if ("complete".equals(state) || "error".equals(state)) {
            githubOperationActive = false;
            WorkshopLongWorkCoordinator.finishGitHub(this);
            if (activityDestroyed) githubSyncExecutor.shutdown();
        }
        runOnUiThread(new Runnable() {
            @Override public void run() { if (githubSyncStatus != null) githubSyncStatus.setText(status); }
        });
    }

    private static void uploadGitHubFile(String token, String repository, String branch, String path, String source) throws Exception {
        uploadGitHubFile(token, repository, branch, path, source.getBytes(StandardCharsets.UTF_8));
    }

    private static void uploadGitHubFile(String token, String repository, String branch, String path, byte[] content) throws Exception {
        String base = githubApiUrl(repository, "/contents/" + encodeGitHubPath(path));
        HttpURLConnection get = (HttpURLConnection)new URL(base + "?ref=" + encodeGitHubQuery(branch)).openConnection();
        configureGitHubConnection(get, token);
        String sha = "";
        int getCode = get.getResponseCode();
        if (getCode == 200) sha = new JSONObject(readStreamStatic(get.getInputStream())).optString("sha", "");
        else if (getCode != 404) throw new IOException("read " + path + " HTTP " + getCode);
        JSONObject body = new JSONObject().put("message", "stasis workshop sync: " + path)
                .put("content", Base64.encodeToString(content, Base64.NO_WRAP))
                .put("branch", branch);
        if (!sha.isEmpty()) body.put("sha", sha);
        HttpURLConnection put = (HttpURLConnection)new URL(base).openConnection();
        put.setRequestMethod("PUT"); put.setDoOutput(true);
        configureGitHubConnection(put, token);
        put.setRequestProperty("Content-Type", "application/json");
        OutputStream output = put.getOutputStream(); output.write(body.toString().getBytes(StandardCharsets.UTF_8)); output.close();
        int putCode = put.getResponseCode();
        if (putCode != 200 && putCode != 201) throw new IOException("write " + path + " HTTP " + putCode);
    }

    private void toggleAiSettings() {
        if (aiSettingsBody == null) {
            return;
        }
        aiSettingsBody.setVisibility(aiSettingsBody.getVisibility() == View.VISIBLE ? View.GONE : View.VISIBLE);
    }

    private String selectedAiProvider() {
        return aiProviderSelector != null && aiProviderSelector.getSelectedItemPosition() == 1
                ? AI_PROVIDER_API : AI_PROVIDER_CODEX;
    }

    private void updateAiProviderVisibility() {
        boolean codex = AI_PROVIDER_CODEX.equals(selectedAiProvider());
        if (aiMonthlyLimitUsdEditor != null) {
            aiMonthlyLimitUsdEditor.setVisibility(codex ? View.GONE : View.VISIBLE);
        }
        refreshAiBudgetStatus();
        refreshAiAttachmentStatus();
    }

    private String codexHomePath() {
        return new File(getFilesDir(), "codex").getAbsolutePath();
    }

    private void beginPhoneNativeCodexLogin() {
        if (!phoneNativeCodexReady) {
            if (codexAccountStatus != null) {
                codexAccountStatus.setText("Codex account: Android certificate verifier initialization failed");
            }
            return;
        }
        codexLoginLifecycle.onLoginStarted();
        gameLoopHandler.removeCallbacks(codexStatusPoll);
        if (codexAccountStatus != null) codexAccountStatus.setText("Codex account: requesting a device code...");
        codexExecutor.execute(new Runnable() {
            @Override public void run() {
                try {
                    final JSONObject status = new JSONObject(nativeCodexBeginDeviceLogin(codexHomePath()));
                    runOnUiThread(new Runnable() {
                        @Override public void run() {
                            if ("awaiting_user".equals(status.optString("status", ""))) {
                                showCodexDeviceCodeDialog(
                                        status.optString("verification_url", ""),
                                        status.optString("user_code", ""));
                            }
                            showPhoneNativeCodexStatus(status);
                            if (!"awaiting_user".equals(status.optString("status", ""))
                                    && showProjectChooserAfterCodexLogin) {
                                showProjectChooserAfterCodexLogin = false;
                                showProjectChooser();
                            }
                        }
                    });
                } catch (Exception error) {
                    final String message = error.getMessage();
                    runOnUiThread(new Runnable() {
                        @Override public void run() {
                            if (codexAccountStatus != null) codexAccountStatus.setText("Codex account error: " + message);
                            if (showProjectChooserAfterCodexLogin) {
                                showProjectChooserAfterCodexLogin = false;
                                showProjectChooser();
                            }
                        }
                    });
                }
            }
        });
    }

    private void refreshPhoneNativeCodexStatus() {
        if (!phoneNativeCodexReady || !codexLoginLifecycle.beginStatusRequest()) return;
        codexExecutor.execute(new Runnable() {
            @Override public void run() {
                try {
                    final JSONObject status = new JSONObject(nativeCodexAccountStatus(codexHomePath()));
                    runOnUiThread(new Runnable() {
                        @Override public void run() {
                            codexLoginLifecycle.finishStatusRequest();
                            showPhoneNativeCodexStatus(status);
                        }
                    });
                } catch (Exception error) {
                    final String message = error.getMessage();
                    runOnUiThread(new Runnable() {
                        @Override public void run() {
                            codexLoginLifecycle.finishStatusRequest();
                            if (codexAccountStatus != null) codexAccountStatus.setText("Codex account error: " + message);
                            if (codexLoginLifecycle.schedulePoll()) {
                                gameLoopHandler.removeCallbacks(codexStatusPoll);
                                gameLoopHandler.postDelayed(codexStatusPoll, 2000L);
                            }
                        }
                    });
                }
            }
        });
    }

    private void postAiWorkingNotes(final String notes) {
        final String display = WorkshopAiWorkingNotes.compactForDisplay(notes);
        runOnUiThread(new Runnable() {
            @Override public void run() {
                setStatusText("AI working notes: " + display);
            }
        });
    }

    private void showCodexDeviceCodeDialog(String verificationUrl, String userCode) {
        showCodexDeviceCodeDialog(verificationUrl, userCode, true);
    }

    private void showCodexDeviceCodeDialog(String verificationUrl, String userCode,
                                           boolean openBrowserAutomatically) {
        if (!isOfficialCodexVerificationUrl(verificationUrl) || userCode.trim().isEmpty()) {
            setStatusText("Codex sign-in returned an invalid verification link or code; request a new code");
            if (showProjectChooserAfterCodexLogin) {
                showProjectChooserAfterCodexLogin = false;
                showProjectChooser();
            }
            return;
        }
        codexLoginVerificationUrl = verificationUrl;
        codexLoginUserCode = userCode.trim();
        copyCodexLoginCode();

        LinearLayout content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        content.setPadding(dp(22), dp(8), dp(22), 0);
        TextView instructions = new TextView(this);
        instructions.setText("The one-time code is copied. In the browser, paste it, finish sign-in, then return here. Workshop will verify completion automatically.");
        instructions.setTextSize(14.0f);
        content.addView(instructions, fullWidth());
        TextView code = new TextView(this);
        code.setText(codexLoginUserCode);
        code.setTextSize(24.0f);
        code.setTypeface(Typeface.MONOSPACE, Typeface.BOLD);
        code.setTextIsSelectable(true);
        code.setGravity(Gravity.CENTER);
        code.setPadding(0, dp(16), 0, dp(12));
        code.setContentDescription("Codex one-time sign-in code " + codexLoginUserCode);
        content.addView(code, fullWidth());
        codexLoginDialogStatus = new TextView(this);
        codexLoginDialogStatus.setText("Waiting for browser sign-in...");
        codexLoginDialogStatus.setTextSize(13.0f);
        codexLoginDialogStatus.setAccessibilityLiveRegion(View.ACCESSIBILITY_LIVE_REGION_POLITE);
        content.addView(codexLoginDialogStatus, fullWidth());

        if (codexLoginDialog != null) codexLoginDialog.dismiss();
        codexLoginDialog = new AlertDialog.Builder(this)
                .setTitle("Sign in to Codex")
                .setView(content)
                .setPositiveButton("Open Browser", null)
                .setNeutralButton("Copy Code", null)
                .setNegativeButton("Continue", null)
                .create();
        codexLoginDialog.show();
        codexLoginDialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { openCodexVerificationUrl(); }
        });
        codexLoginDialog.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { copyCodexLoginCode(); }
        });
        codexLoginDialog.getButton(AlertDialog.BUTTON_NEGATIVE).setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) {
                codexLoginDialogStatus.setText("Checking whether browser sign-in finished...");
                refreshPhoneNativeCodexStatus();
            }
        });
        if (openBrowserAutomatically) {
            content.postDelayed(new Runnable() {
                @Override public void run() { openCodexVerificationUrl(); }
            }, 250L);
        }
    }

    private void registerNetworkMonitoring() {
        connectivityManager = (ConnectivityManager)getSystemService(CONNECTIVITY_SERVICE);
        if (connectivityManager == null || networkCallbackRegistered) return;
        networkCallback = new ConnectivityManager.NetworkCallback() {
            @Override public void onAvailable(Network network) { resumeQueuedAiAfterNetworkChange(); }
            @Override public void onCapabilitiesChanged(Network network, NetworkCapabilities capabilities) {
                resumeQueuedAiAfterNetworkChange();
            }
        };
        try {
            connectivityManager.registerDefaultNetworkCallback(networkCallback);
            networkCallbackRegistered = true;
        } catch (RuntimeException error) {
            networkCallback = null;
        }
    }

    private void resumeQueuedAiAfterNetworkChange() {
        if (!WorkshopConnectivity.hasUsableNetwork(this)) return;
        runOnUiThread(new Runnable() {
            @Override public void run() { startNextQueuedAiIfIdle(); }
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

    private static boolean isOfficialCodexVerificationUrl(String value) {
        try {
            Uri uri = Uri.parse(value);
            return "https".equalsIgnoreCase(uri.getScheme())
                    && "auth.openai.com".equalsIgnoreCase(uri.getHost());
        } catch (Exception ignored) {
            return false;
        }
    }

    private void openCodexVerificationUrl() {
        if (!isOfficialCodexVerificationUrl(codexLoginVerificationUrl)) {
            setStatusText("Codex verification link is unavailable; request a new code");
            return;
        }
        try {
            startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse(codexLoginVerificationUrl)));
        } catch (Exception error) {
            setStatusText("Open " + codexLoginVerificationUrl + " to finish Codex sign-in");
        }
    }

    private void copyCodexLoginCode() {
        if (codexLoginUserCode.isEmpty()) return;
        ClipboardManager clipboard = (ClipboardManager)getSystemService(CLIPBOARD_SERVICE);
        if (clipboard == null) {
            setStatusText("Clipboard is unavailable; press and hold the visible code to copy it");
            return;
        }
        clipboard.setPrimaryClip(ClipData.newPlainText("Codex sign-in code", codexLoginUserCode));
        Toast.makeText(this, "Codex code copied", Toast.LENGTH_SHORT).show();
    }

    private void clearCopiedCodexLoginCode() {
        if (codexLoginUserCode.isEmpty()) return;
        ClipboardManager clipboard = (ClipboardManager)getSystemService(CLIPBOARD_SERVICE);
        if (clipboard == null || !clipboard.hasPrimaryClip() || clipboard.getPrimaryClip() == null
                || clipboard.getPrimaryClip().getItemCount() == 0) return;
        CharSequence current = clipboard.getPrimaryClip().getItemAt(0).coerceToText(this);
        if (!codexLoginUserCode.contentEquals(current)) return;
        if (Build.VERSION.SDK_INT >= 28) clipboard.clearPrimaryClip();
        else clipboard.setPrimaryClip(ClipData.newPlainText("", ""));
    }

    private void refreshCodexRateLimitsAfterAction() {
        SharedPreferences preferences = getSharedPreferences(AI_PREFS, MODE_PRIVATE);
        long now = System.currentTimeMillis();
        long lastAttempt = preferences.getLong(AI_PREF_CODEX_LIMITS_REFRESH_ATTEMPT_MS, 0L);
        if (now - lastAttempt < CODEX_LIMIT_REFRESH_DEBOUNCE_MS) return;
        preferences.edit().putLong(AI_PREF_CODEX_LIMITS_REFRESH_ATTEMPT_MS, now).apply();
        codexExecutor.execute(new Runnable() {
            @Override public void run() {
                try {
                    final JSONObject limits = new JSONObject(nativeCodexAccountRateLimits(codexHomePath()));
                    if (!"ok".equals(limits.optString("status", ""))) return;
                    getSharedPreferences(AI_PREFS, MODE_PRIVATE).edit()
                            .putString(AI_PREF_CODEX_LIMITS_JSON, limits.toString()).apply();
                    runOnUiThread(new Runnable() {
                        @Override public void run() { refreshAiBudgetStatus(); }
                    });
                } catch (Exception ignored) {
                    // Keep the last successful snapshot; the next action retries after the debounce.
                }
            }
        });
    }

    private void showPhoneNativeCodexStatus(JSONObject status) {
        if (codexAccountStatus == null) return;
        String state = status.optString("status", "error");
        if ("signed_in".equals(state)) {
            codexSignedIn = true;
            SharedPreferences preferences = getSharedPreferences(AI_PREFS, MODE_PRIVATE);
            boolean activeLogin = !codexLoginUserCode.isEmpty()
                    || (codexLoginDialog != null && codexLoginDialog.isShowing());
            if (WorkshopAiProviderPolicy.promoteCodexAfterSignIn(activeLogin,
                    preferences.getBoolean(AI_PREF_CODEX_PRIMARY_MIGRATION, false))) {
                preferences.edit()
                        .putString(AI_PREF_PROVIDER, AI_PROVIDER_CODEX)
                        .putBoolean(AI_PREF_CODEX_PRIMARY_MIGRATION, true)
                        .apply();
                if (aiProviderSelector != null && aiProviderSelector.getSelectedItemPosition() != 0) {
                    aiProviderSelector.setSelection(0);
                }
            }
            String plan = status.optString("plan_type", "");
            codexAccountStatus.setText("Codex account: signed in on this phone"
                    + (plan.isEmpty() ? "" : " (" + plan + ")"));
            boolean handleCompletion = codexLoginLifecycle.onSignedIn();
            gameLoopHandler.removeCallbacks(codexStatusPoll);
            if (codexLoginDialogStatus != null) {
                codexLoginDialogStatus.setText("Signed in successfully. Returning to Workshop...");
            }
            clearCopiedCodexLoginCode();
            refreshAiBudgetStatus();
            if (!handleCompletion) return;
            final boolean showProjects = showProjectChooserAfterCodexLogin;
            showProjectChooserAfterCodexLogin = false;
            gameLoopHandler.postDelayed(new Runnable() {
                @Override public void run() {
                    if (codexLoginDialog != null) codexLoginDialog.dismiss();
                    codexLoginDialog = null;
                    codexLoginDialogStatus = null;
                    codexLoginUserCode = "";
                    codexLoginVerificationUrl = "";
                    if (showProjects) showProjectChooser();
                }
            }, 750L);
            return;
        }
        if ("awaiting_user".equals(state)) {
            codexLoginLifecycle.onAwaitingUser();
            codexSignedIn = false;
            String userCode = status.optString("user_code", "").trim();
            String verificationUrl = status.optString(
                    "verification_url", "https://auth.openai.com/codex/device");
            codexLoginUserCode = userCode;
            codexLoginVerificationUrl = verificationUrl;
            codexAccountStatus.setText("Codex sign-in code: " + userCode + "\n" + verificationUrl);
            if (codexLoginDialogStatus != null) {
                codexLoginDialogStatus.setText("Waiting for browser sign-in...");
            }
            boolean dialogShowing = codexLoginDialog != null && codexLoginDialog.isShowing();
            boolean validCode = !userCode.isEmpty() && isOfficialCodexVerificationUrl(verificationUrl);
            if (codexLoginLifecycle.shouldPresentDialog(dialogShowing, validCode)) {
                showCodexDeviceCodeDialog(verificationUrl, userCode, false);
            }
            if (codexLoginLifecycle.schedulePoll()) {
                gameLoopHandler.removeCallbacks(codexStatusPoll);
                gameLoopHandler.postDelayed(codexStatusPoll, 2000L);
            }
            return;
        }
        if ("signed_out".equals(state)) {
            codexLoginLifecycle.onTerminalFailure();
            gameLoopHandler.removeCallbacks(codexStatusPoll);
            codexSignedIn = false;
            codexAccountStatus.setText("Codex account: signed out on this phone");
            refreshAiBudgetStatus();
            return;
        }
        codexLoginLifecycle.onTerminalFailure();
        gameLoopHandler.removeCallbacks(codexStatusPoll);
        codexSignedIn = false;
        String error = status.optString("error", state);
        codexAccountStatus.setText("Codex sign-in failed: " + error);
        if (codexLoginDialogStatus != null) {
            codexLoginDialogStatus.setText("Sign-in failed: " + error
                    + "\nClose this message and request a new code to try again.");
        }
        refreshAiBudgetStatus();
    }

    private void saveAiSettingsFromEditors() {
        String provider = selectedAiProvider();
        String apiKey = aiApiKeyEditor == null ? "" : aiApiKeyEditor.getText().toString().trim();
        String model = aiModelEditor == null ? "" : aiModelEditor.getText().toString().trim();
        String monthlyLimitText = aiMonthlyLimitUsdEditor == null ? "5.00" : aiMonthlyLimitUsdEditor.getText().toString().trim();
        if (AI_PROVIDER_API.equals(provider) && apiKey.isEmpty()) {
            setStatusText("AI settings need an API key before a run can start");
            return;
        }
        if (parseNonNegativeUsd(monthlyLimitText) < 0.0) {
            setStatusText("The device monthly AI limit must be a non-negative USD value");
            return;
        }
        if (!apiKey.isEmpty() && !saveAiSettings(apiKey, model.isEmpty() ? DEFAULT_AI_MODEL : model)) return;
        getSharedPreferences(AI_PREFS, MODE_PRIVATE).edit()
                .putString(AI_PREF_PROVIDER, provider)
                .putString(AI_PREF_MONTHLY_LIMIT_USD, monthlyLimitText)
                .apply();
        refreshAiBudgetStatus();
        setStatusText(AI_PROVIDER_CODEX.equals(provider)
                ? "Phone-native Codex selected as the primary provider"
                : "OpenAI API fallback selected");
    }

    private static double parseNonNegativeUsd(String value) {
        try {
            double parsed = Double.parseDouble(value);
            return Double.isFinite(parsed) && parsed >= 0.0 ? parsed : -1.0;
        } catch (Exception ignored) {
            return -1.0;
        }
    }

    private static String currentMonthKey() {
        Calendar now = Calendar.getInstance();
        return now.get(Calendar.YEAR) + "-" + (now.get(Calendar.MONTH) + 1);
    }

    private double monthlyAiSpendUsd() {
        SharedPreferences prefs = getSharedPreferences(AI_PREFS, MODE_PRIVATE);
        if (!currentMonthKey().equals(prefs.getString(AI_PREF_MONTH_KEY, ""))) return 0.0;
        return Math.max(0.0, parseNonNegativeUsd(prefs.getString(AI_PREF_MONTH_SPEND_USD, "0")));
    }

    private void recordMonthlyAiSpend(double costUsd) {
        if (costUsd <= 0.0) return;
        SharedPreferences prefs = getSharedPreferences(AI_PREFS, MODE_PRIVATE);
        double updated = monthlyAiSpendUsd() + costUsd;
        prefs.edit().putString(AI_PREF_MONTH_KEY, currentMonthKey())
                .putString(AI_PREF_MONTH_SPEND_USD, Double.toString(updated)).apply();
        runOnUiThread(new Runnable() { @Override public void run() { refreshAiBudgetStatus(); } });
    }

    private double configuredAiLimit(String key, String fallback) {
        double value = parseNonNegativeUsd(getSharedPreferences(AI_PREFS, MODE_PRIVATE).getString(key, fallback));
        return value < 0.0 ? Double.parseDouble(fallback) : value;
    }

    private void refreshAiBudgetStatus() {
        if (aiBudgetStatus == null) return;
        if (AI_PROVIDER_CODEX.equals(selectedAiProvider())) {
            aiBudgetStatus.setText(cachedCodexLimitText());
            return;
        }
        double monthlyLimit = configuredAiLimit(AI_PREF_MONTHLY_LIMIT_USD, "5.00");
        double spent = monthlyAiSpendUsd();
        aiBudgetStatus.setText("AI budget: " + formatAiCostUsd(spent) + " / " + formatAiCostUsd(monthlyLimit) + " this month");
    }

    private String cachedCodexLimitText() {
        String cached = getSharedPreferences(AI_PREFS, MODE_PRIVATE)
                .getString(AI_PREF_CODEX_LIMITS_JSON, "");
        if (!codexSignedIn) return "Codex limits: sign in to view";
        if (cached.isEmpty()) return "Codex limits: refresh after the next AI action";
        try {
            JSONObject limits = new JSONObject(cached);
            String primary = formatCodexLimitWindow(limits.optJSONObject("primary"));
            String secondary = formatCodexLimitWindow(limits.optJSONObject("secondary"));
            if (primary.isEmpty() && secondary.isEmpty()) return "Codex limits: unavailable";
            if (primary.isEmpty()) return "Codex: " + secondary;
            if (secondary.isEmpty()) return "Codex: " + primary;
            return "Codex: " + primary + " | " + secondary;
        } catch (Exception error) {
            return "Codex limits: refresh after the next AI action";
        }
    }

    private static String formatCodexLimitWindow(JSONObject window) {
        if (window == null) return "";
        return WorkshopCodexLimits.formatWindow(
                window.optLong("window_duration_mins", 0L),
                window.optDouble("used_percent", 0.0));
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
                refreshChangeSummary(loadBundledProject());
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
        long tickStartNanos = System.nanoTime();
        int frameStatus = nativeRunFrameInto(
                projectRootPath(),
                touchX,
                touchY,
                touchActive,
                screenWidth,
                screenHeight,
                nativeFrameValues);
        long tickEndNanos = System.nanoTime();
        tickMetric.add(tickEndNanos, tickEndNanos - tickStartNanos);
        if (frameStatus != 0 || nativeFrameValues[0] != 0) {
            compileReady = false;
            compileAttempted = true;
            setStatusText("RunError: native frame tick failed");
            return;
        }
        if (gamePreview != null) {
            gamePreview.setRenderFrameValues(nativeFrameValues);
        }
        updateGameDebugText();
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

    private void runAiPatch() {
        runAiPatch("text", null);
    }

    private void runAiPatch(String queueSource, AndroidAiQueue.Entry queuedEntry) {
        if (audioRecordingActive) {
            setStatusText("Finish or cancel audio recording before running AI");
            failQueuedAiPreflight(queuedEntry, "Audio recording was active");
            return;
        }
        if (aiRunActive) {
            enqueuePendingAiRequest(queueSource);
            return;
        }
        String apiKey = aiApiKeyEditor == null ? "" : aiApiKeyEditor.getText().toString().trim();
        String prompt = queuedEntry == null
                ? (aiPromptEditor == null ? "" : aiPromptEditor.getText().toString().trim())
                : queuedEntry.prompt;
        final boolean useCodex = AI_PROVIDER_CODEX.equals(selectedAiProvider());
        if (useCodex) {
            if (prompt.isEmpty()) {
                setStatusText("AI run needs a request");
                failQueuedAiPreflight(queuedEntry, "Request was missing at execution time");
                return;
            }
            try {
                JSONObject account = new JSONObject(nativeCodexAccountStatus(codexHomePath()));
                if (!account.optBoolean("signed_in", false)) {
                    setStatusText("Sign in to ChatGPT under AI Settings to use phone-native Codex");
                    failQueuedAiPreflight(queuedEntry, "Phone-native Codex is not signed in");
                    return;
                }
                refreshCodexRateLimitsAfterAction();
            } catch (Exception error) {
                setStatusText("Phone-native Codex status failed: " + error.getMessage());
                failQueuedAiPreflight(queuedEntry, "Phone-native Codex status failed");
                return;
            }
        }
        String model = aiModelEditor == null ? "" : aiModelEditor.getText().toString().trim();
        if ((!useCodex && apiKey.isEmpty()) || prompt.isEmpty()) {
            setStatusText("AI run needs both a request and an API key; open AI Settings if the key is not saved");
            updateAiProgress(0, 0, "needs input");
            failQueuedAiPreflight(queuedEntry, "API key or request was missing at execution time");
            return;
        }
        if (!useCodex && model.isEmpty()) {
            model = DEFAULT_AI_MODEL;
        }
        if (useCodex) model = "codex-default";
        double monthlyLimitUsd = configuredAiLimit(AI_PREF_MONTHLY_LIMIT_USD, "5.00");
        final boolean requestImageGeneration = queuedEntry == null
                ? allowAiImageGeneration != null && allowAiImageGeneration.isChecked()
                : queuedEntry.imageGeneration;
        if (!useCodex && !hasKnownAiPricing(model)) {
            setStatusText("AI run blocked: pricing is unavailable for " + model);
            updateAiProgress(0, 0, "budget blocked");
            failQueuedAiPreflight(queuedEntry, "Model pricing was unavailable at execution time");
            return;
        }
        if (!useCodex && !WorkshopAiBudgetPolicy.canStart(monthlyLimitUsd, monthlyAiSpendUsd())) {
            setStatusText("AI run blocked by the device monthly spending limit; open AI Settings");
            updateAiProgress(0, 0, "budget blocked");
            failQueuedAiPreflight(queuedEntry, "Device monthly AI limit blocked execution");
            return;
        }
        recordCommandHistory(prompt);
        if (!useCodex && !saveAiSettings(apiKey, model)) {
            failQueuedAiPreflight(queuedEntry, "AI settings could not be saved");
            return;
        }
        final SymbolEntry symbol = selectedSymbol;
        final String selectedSource = symbol == null || sourceEditor == null ? "" : sourceEditor.getText().toString().trim();
        final ProjectSnapshot aiProject = loadBundledProject();
        final List<WorkshopImageAssets.AssetInfo> requestImageInfos;
        final JSONArray requestImageMetadata;
        final Bitmap requestPreviewPixels;
        final JSONObject requestLogicalSnapshot;
        try {
            requestImageInfos = queuedEntry == null ? selectedAiImageInfos() : aiImageInfosForQueueEntry(queuedEntry);
            requestImageMetadata = queuedEntry == null ? aiImageMetadata(requestImageInfos)
                    : new JSONArray(queuedEntry.imageAttachments.toString());
            boolean includePreview = queuedEntry == null ? attachPreviewPixels : !queuedEntry.previewFile.isEmpty();
            if (includePreview) {
                if (requestImageInfos.size() >= MAX_AI_IMAGE_ATTACHMENTS) {
                    throw new IOException("preview plus project images exceed the four-image request limit");
                }
                if (queuedEntry == null) {
                    if (pendingPreviewScreenshot == null || pendingPreviewScreenshot.isRecycled()) {
                        throw new IOException("selected preview pixels are no longer available");
                    }
                    requestPreviewPixels = pendingPreviewScreenshot.copy(Bitmap.Config.ARGB_8888, false);
                } else {
                    byte[] previewBytes = AndroidAiQueue.loadPreview(this, queuedEntry);
                    requestPreviewPixels = android.graphics.BitmapFactory.decodeByteArray(
                            previewBytes, 0, previewBytes.length);
                    if (requestPreviewPixels == null || requestPreviewPixels.getWidth() != queuedEntry.previewWidth
                            || requestPreviewPixels.getHeight() != queuedEntry.previewHeight) {
                        throw new IOException("queued preview pixels failed decode or dimensions");
                    }
                }
                requestImageMetadata.put(new JSONObject()
                        .put("kind", "captured_preview_pixels")
                        .put("width", requestPreviewPixels.getWidth())
                        .put("height", requestPreviewPixels.getHeight())
                        .put("detail", "original")
                        .put("estimated_patch_tokens", ((requestPreviewPixels.getWidth() + 31L) / 32L)
                                * ((requestPreviewPixels.getHeight() + 31L) / 32L)));
            } else {
                requestPreviewPixels = null;
            }
            requestLogicalSnapshot = queuedEntry == null
                    ? (attachPreviewLogicalSnapshot && pendingPreviewLogicalSnapshot != null
                            ? new JSONObject(pendingPreviewLogicalSnapshot.toString()) : null)
                    : (queuedEntry.logicalSnapshot == null ? null
                            : new JSONObject(queuedEntry.logicalSnapshot.toString()));
        } catch (Exception error) {
            setStatusText("AI run blocked by image attachments: " + error.getMessage());
            updateAiProgress(0, 0, "attachment blocked");
            failQueuedAiPreflight(queuedEntry, "Attachment snapshot failed validation: " + error.getMessage());
            return;
        }
        if (!useCodex && requestImageGeneration && WorkshopAiBudgetPolicy.remainingUsd(
                monthlyLimitUsd, monthlyAiSpendUsd()) < GPT_IMAGE_2_LOW_1024_USD) {
            setStatusText("AI image generation blocked: the device monthly limit does not cover the reserved image output");
            updateAiProgress(0, 0, "image budget blocked");
            failQueuedAiPreflight(queuedEntry, "Image generation reserve exceeded the device monthly AI limit");
            return;
        }
        final String requestJson = buildAiCodeRequestJson(prompt, symbol, selectedSource, aiProject,
                requestImageMetadata, requestLogicalSnapshot);
        final String requestModel = model;
        final String requestApiKey = apiKey;
        try {
            if (queuedEntry == null) {
                AndroidAiQueue.Entry submitted = AndroidAiQueue.enqueue(this, activeRecoveryProjectId(), queueSource, prompt,
                        requestImageMetadata, requestLogicalSnapshot, requestImageGeneration,
                        requestPreviewPixels == null ? null : encodeBitmapPng(requestPreviewPixels),
                        requestPreviewPixels == null ? 0 : requestPreviewPixels.getWidth(),
                        requestPreviewPixels == null ? 0 : requestPreviewPixels.getHeight());
                if (!WorkshopConnectivity.hasUsableNetwork(this)) {
                    if (requestPreviewPixels != null && !requestPreviewPixels.isRecycled()) requestPreviewPixels.recycle();
                    refreshAiQueue();
                    setStatusText("AI request queued and waiting for an internet connection");
                    return;
                }
                if (WorkshopLongWorkCoordinator.isAnyActive()) {
                    if (requestPreviewPixels != null && !requestPreviewPixels.isRecycled()) requestPreviewPixels.recycle();
                    refreshAiQueue();
                    setStatusText("AI request queued behind the active foreground operation");
                    return;
                }
                activeAiQueueEntry = AndroidAiQueue.claimNext(this, activeRecoveryProjectId());
                if (activeAiQueueEntry != null && !submitted.id.equals(activeAiQueueEntry.id)) {
                    if (requestPreviewPixels != null && !requestPreviewPixels.isRecycled()) requestPreviewPixels.recycle();
                    AndroidAiQueue.Entry older = activeAiQueueEntry;
                    activeAiQueueEntry = null;
                    refreshAiQueue();
                    runAiPatch(older.source, older);
                    return;
                }
            } else {
                activeAiQueueEntry = queuedEntry;
            }
            if (activeAiQueueEntry == null) throw new IOException("queued AI request could not be claimed");
        } catch (Exception error) {
            setStatusText("AI queue failed: " + error.getMessage());
            refreshAiQueue();
            return;
        }
        refreshAiQueue();
        activeAiPrompt = prompt;
        aiCancelRequested = false;
        aiRunActive = true;
        if (!WorkshopLongWorkCoordinator.beginAi(this, "Running queued game change", new Runnable() {
            @Override public void run() { cancelAiRun(); }
        })) {
            aiRunActive = false;
            failQueuedAiPreflight(activeAiQueueEntry,
                    "Android could not start the required foreground work service");
            setStatusText("AI work could not start its Android foreground service");
            return;
        }
        if (aiCancelButton != null) aiCancelButton.setVisibility(View.VISIBLE);
        recordAiOutcome(activeAiPrompt, "started", "AI run started", "");
        aiStartedAtNanos = System.nanoTime();
        appendAiTraceFields("request", "model", requestModel, "request_json", requestJson, null, null);
        setStatusText("AI run started: preparing workspace and command context");
        updateAiProgress(0, 0, "preparing");
        new Thread(new Runnable() {
            @Override
            public void run() {
                try {
                    activeAiImageAttachments = loadAiImageAttachments(
                            requestImageInfos, requestImageMetadata, requestPreviewPixels);
                    final AiAgentResult aiResult = runAiAgentLoop(
                            requestApiKey, requestModel, requestJson, requestImageGeneration, useCodex);
                    throwIfAiCancelled();
                    runOnUiThread(new Runnable() {
                        @Override
                        public void run() {
                            applyAiCodeResponse(aiResult, symbol);
                        }
                    });
                } catch (final Exception error) {
                    if (aiCancelRequested || error instanceof AiCancelledException) {
                        runOnUiThread(new Runnable() {
                            @Override public void run() {
                                updateAiProgress(aiProgressStep, aiProgressActions, "cancelled");
                                appendAiTraceFields("cancelled", "elapsed", currentAiElapsedText(), null, null, null, null);
                                recordAiOutcome(activeAiPrompt, "cancelled", "Cancelled by user", "completed calls retained in budget totals");
                                setStatusText("AI run cancelled; completed calls remain in usage totals");
                            }
                        });
                        return;
                    }
                    runOnUiThread(new Runnable() {
                        @Override
                        public void run() {
                            String elapsed = currentAiElapsedText();
                            updateAiProgress(aiProgressStep, aiProgressActions, "failed");
                            appendAiTraceFields("fatal_error", "error", error.getMessage(), "elapsed", elapsed, "trace_path", aiTraceLogPath());
                            recordAiOutcome(activeAiPrompt, "failed", error.getMessage(), "");
                            setStatusText("AI edit failed: elapsed=" + elapsed + " - " + error.getMessage() + " - trace=" + aiTraceLogPath());
                        }
                    });
                } finally {
                    if (requestPreviewPixels != null && !requestPreviewPixels.isRecycled()) requestPreviewPixels.recycle();
                    activeAiImageAttachments = Collections.emptyList();
                    if (requestImageGeneration) {
                        runOnUiThread(new Runnable() {
                            @Override public void run() { allowAiImageGeneration.setChecked(false); }
                        });
                    }
                }
            }
        }).start();
    }

    private void cancelAiRun() {
        if (!aiRunActive) {
            setStatusText("No AI run is active");
            return;
        }
        aiCancelRequested = true;
        nativeCodexCancelResponse();
        HttpURLConnection connection = activeAiConnection;
        if (connection != null) connection.disconnect();
        updateAiProgress(aiProgressStep, aiProgressActions, "cancelling");
        setStatusText("AI cancellation requested; finishing any active call or atomic write batch");
    }

    private void throwIfAiCancelled() throws AiCancelledException {
        if (aiCancelRequested) throw new AiCancelledException();
    }

    private boolean saveAiSettings(String apiKey, String model) {
        SharedPreferences preferences = getSharedPreferences(AI_PREFS, MODE_PRIVATE);
        if (!writeSecretPreference(preferences, AI_PREF_API_KEY, apiKey)) return false;
        preferences.edit().putString(AI_PREF_MODEL, model)
                .putInt(AI_PREF_MODEL_DEFAULT_VERSION, DEFAULT_AI_MODEL_VERSION).apply();
        return true;
    }

    private static JSONArray aiProjectGlobals(ProjectSnapshot project) throws Exception {
        JSONArray globals = new JSONArray();
        if (project == null) {
            return globals;
        }
        for (SymbolSection section : project.sections) {
            for (SymbolGroup group : section.groups) {
                for (SymbolEntry symbol : group.symbols) {
                    if (!"global".equals(symbol.kind)) {
                        continue;
                    }
                    globals.put(new JSONObject()
                            .put("kind", "global")
                            .put("name", symbol.name)
                            .put("file", symbol.file)
                            .put("backing_struct_type", symbol.name)
                            .put("backing_struct_source", symbol.backingStructSource));
                }
            }
        }
        return globals;
    }

    private static JSONObject aiProjectSymbolIndex(ProjectSnapshot project) throws Exception {
        JSONArray symbols = new JSONArray();
        int availableCount = 0;
        int serializedChars = 2;
        boolean accepting = true;
        if (project != null) {
            for (SymbolSection section : project.sections) {
                for (SymbolGroup group : section.groups) {
                    for (SymbolEntry symbol : group.symbols) {
                        availableCount += 1;
                        if (!accepting) continue;
                        JSONObject compact = symbolToJson(symbol, false);
                        int candidateChars = compact.toString().length();
                        if (!WorkshopAiInitialContextPolicy.canAppend(
                                serializedChars, candidateChars, symbols.length())) {
                            accepting = false;
                            continue;
                        }
                        if (symbols.length() > 0) serializedChars += 1;
                        serializedChars += candidateChars;
                        symbols.put(compact);
                    }
                }
            }
        }
        return new JSONObject()
                .put("symbols", symbols)
                .put("included_count", symbols.length())
                .put("available_count", availableCount)
                .put("truncated", symbols.length() < availableCount);
    }

    private static JSONObject aiFastPathContext(String prompt, ProjectSnapshot project,
            JSONArray imageAttachments) throws Exception {
        boolean enabled = (imageAttachments == null || imageAttachments.length() == 0)
                && WorkshopAiFastPathPolicy.isSimpleTuningPrompt(prompt);
        JSONArray symbols = new JSONArray();
        int availableCount = 0;
        int serializedChars = 2;
        boolean accepting = enabled;
        if (enabled && project != null) {
            TreeSet<String> added = new TreeSet<>();
            for (int wantedScore = 32; wantedScore >= 1; wantedScore -= 1) {
                for (SymbolSection section : project.sections) {
                    for (SymbolGroup group : section.groups) {
                        for (SymbolEntry symbol : group.symbols) {
                            int score = WorkshopAiFastPathPolicy.relevanceScore(
                                    prompt, symbol.name, symbol.kind);
                            String key = symbol.kind + "|" + symbol.file + "|" + symbol.name;
                            if (score <= 0 || added.contains(key)) continue;
                            if (wantedScore == 32) availableCount += 1;
                            if (score != wantedScore || !accepting) continue;
                            JSONObject source = symbolToJson(symbol, true);
                            int candidateChars = source.toString().length();
                            if (!WorkshopAiFastPathPolicy.canAppendSource(
                                    serializedChars, candidateChars, symbols.length())) {
                                accepting = false;
                                continue;
                            }
                            if (symbols.length() > 0) serializedChars += 1;
                            serializedChars += candidateChars;
                            symbols.put(source);
                            added.add(key);
                        }
                    }
                }
            }
        }
        return new JSONObject()
                .put("enabled", enabled)
                .put("reason", enabled ? "short tuning request" : "standard agent path")
                .put("source_symbols", symbols)
                .put("included_count", symbols.length())
                .put("available_count", availableCount)
                .put("truncated", enabled && symbols.length() < availableCount)
                .put("instruction", enabled
                        ? "Use these bounded current sources to write the complete change and a behavior test in the first response; do not spend a read-only turn when the needed source is present."
                        : "Use the normal inspect-write-test loop.");
    }

    private static JSONObject aiStasisBasics() throws Exception {
        return new JSONObject()
                .put("language", new JSONArray()
                        .put("Declare typed arguments and returns with function name(arg_name: Type, other: Type): ReturnType { ... }; use void when there is no return value.")
                        .put("Declare value types with struct TypeName { field_name: Type; ... } and access fields as value.field_name.")
                        .put("A persistent instance of a struct is normally declared with global instance_name: StructType; for example global state: GameState; then access state.score.")
                        .put("A direct named global block, global Name { field_name: Type; ... }, is also valid and is accessed as Name.field_name; do not confuse it with a struct type declaration.")
                        .put("Arithmetic, comparison, and assignment operators are infix.")
                        .put("Receiver calls value.method(args) are preferred when the function's first parameter is self: Type; function-form calls remain valid.")
                        .put("Common types are bool, i32, u8, u16, u32, f32, f64, void, fixed arrays Type[N], and bounded text ascii[N] or utf8[N]."))
                .put("runtime", new JSONArray()
                        .put("main() initializes state, tick() advances deterministic fixed-tick simulation, and render() projects current state into render commands.")
                        .put("on_code_swap() is only for migration or reinitialization required after a hot swap.")
                        .put("Gameplay progression is tick-based rather than dt-based."))
                .put("editing", new JSONArray()
                        .put("Functions, structs, globals, imports, and tests are the editable units; inspect a symbol before replacing it.")
                        .put("Function-body and tuning changes can fast reload; struct/global layout changes require reset compatibility handling."))
                .put("tests", new JSONArray()
                        .put("Behavior tests use test `name`(): bool and return true or false."));
    }

    private void runNativeTests() {
        try {
            JSONObject result = aiToolRunTests(new AiAgentSession());
            captureFirstTestFailureDiagnostic(result);
            setStatusText(testSummaryText(result));
        } catch (Exception error) {
            setStatusText("Tests failed: " + error.getMessage());
        }
    }

    private static String buildAiCodeRequestJson(String prompt, SymbolEntry symbol, String selectedSource,
            ProjectSnapshot project, JSONArray imageAttachments, JSONObject logicalSnapshot) {
        try {
            JSONArray selectedSymbols = new JSONArray();
            if (symbol != null) {
                JSONObject selected = new JSONObject();
                selected.put("kind", symbol.kind);
                selected.put("name", symbol.name);
                selected.put("owner", symbol.owner);
                selected.put("file", symbol.file);
                selected.put("source", selectedSource);
                selectedSymbols.put(selected);
            }

            JSONObject rules = new JSONObject();
            rules.put("use_function_keyword", true);
            rules.put("use_receiver_style_when_possible", true);
            rules.put("do_not_use_rust_references", true);
            rules.put("struct_functions_live_with_struct", true);
            rules.put("lifecycle_functions_live_in_main", true);
            rules.put("no_owner_functions_live_in_root", true);

            JSONObject gameRules = new JSONObject();
            gameRules.put("prefer_lifecycle_local_state", true);
            gameRules.put("time_since_spawn_uses_entity_or_event_state", true);
            gameRules.put("reset_lifecycle_timers_when_entity_is_created", true);
            gameRules.put("avoid_global_tick_for_per_entity_progression", true);
            gameRules.put("inspect_creation_and_update_paths_together", true);

            JSONArray architectureRecommendations = new JSONArray()
                    .put("Keep host assumptions out of Stasis game edits; Stasis owns simulation state, rules, and render commands.")
                    .put("Use tick() as the deterministic simulation step, render() as a projection of current state, and on_code_swap() for post-hot-swap state migration/reinitialization.")
                    .put("Put persistent gameplay state in explicit Stasis globals or structs with plain inspectable fields.")
                    .put("Use lifecycle-local state for entity/event timing; reset counters in creation/reset functions and increment them during tick.")
                    .put("Prefer feature-owned data and functions; put cross-entity rules in systems/*.stasis.")
                    .put("As features grow, split toward files named for durable gameplay concepts, such as actors, projectiles, abilities, resources, objectives, camera, score, encounters, and systems/<system>.stasis.")
                    .put("Preserve hot reload when possible by preferring function-body changes and tuning constants; call out struct/global layout changes as ResetRequired.")
                    .put("Use command/event-style functions for lifecycle boundaries, such as spawn_actor(), reset_encounter(), award_resource(kind, amount), start_phase(mode), and fire_projectile().")
                    .put("For multi-symbol behavior changes, inspect the relevant state, lifecycle, update, render, and test paths; for small constant or dimension edits, inspect only the target symbol and its test.")
                    .put("Keep mobile input abstracted through Stasis Input globals and helper functions so logic can move across platforms.")
                    .put("Add or use testable invariants by setting input/state, running ticks, and checking state or render output.")
                    .put("Avoid broad rewrites; make the smallest structural change that gives the feature a clear owner.")
                    .put("Prefer data-oriented clarity over deep abstractions: arrays, IDs, counters, and explicit update loops.")
                    .put("Avoid per-tick allocation/object churn and keep new systems within the visible 60 fps budget.");

            JSONObject request = new JSONObject();
            request.put("cache_layout", "Stable request context is first. Volatile tool observations are sent after the prompt cache breakpoint.");
            request.put("scope", "entire_workspace");
            request.put("stasis_basics", aiStasisBasics());
            request.put("response_contract", aiResponseContract());
            request.put("available_tools", supportedAiTools());
            request.put("tool_specs", aiToolSpecs());
            request.put("stasis_style_rules", rules);
            request.put("game_design_rules", gameRules);
            request.put("architecture_recommendations", architectureRecommendations);
            request.put("project_globals", aiProjectGlobals(project));
            request.put("project_symbol_index", aiProjectSymbolIndex(project));
            request.put("fast_path", aiFastPathContext(prompt, project, imageAttachments));
            request.put("user_prompt", prompt);
            request.put("selected_symbols", selectedSymbols);
            request.put("selected_symbols_are_context_only", true);
            request.put("selected_image_attachments", imageAttachments);
            request.put("selected_images_are_explicit_project_assets_only", true);
            if (logicalSnapshot != null) request.put("selected_preview_logical_snapshot", logicalSnapshot);
            return request.toString();
        } catch (Exception error) {
            return "{}";
        }
    }

    private static JSONObject aiResponseContract() throws Exception {
        JSONArray acceptedShapes = new JSONArray()
                .put(new JSONObject()
                        .put("mode", "tool_calls")
                        .put("working_notes", "Intent: inspect the target. Observed: current facts. Next: one concrete action. Blocker: none.")
                        .put("summary", "short optional status")
                        .put("tool_calls", new JSONArray().put(new JSONObject()
                                .put("tool", "read_symbol")
                                .put("args", new JSONObject().put("name", "tick")))))
                .put(new JSONObject()
                        .put("mode", "done")
                        .put("working_notes", "Intent: finish. Observed: requested behavior is verified. Next: none. Blocker: none.")
                        .put("summary", "what was verified"))
                .put(new JSONObject()
                        .put("mode", "edits")
                        .put("working_notes", "Intent: finish. Observed: writes compiled and tests passed. Next: apply final edits. Blocker: none.")
                        .put("summary", "short change summary")
                        .put("edits", new JSONArray().put(new JSONObject()
                                .put("kind", "replace_function")
                                .put("owner", "Player")
                                .put("name", "jump")
                                .put("file", "src/player.stasis")
                                .put("new_source", "function jump(self: Player): void {\n}"))));
        return new JSONObject()
                .put("required", "Return exactly one JSON object. The top-level object must match one accepted_response_shape and include concise working_notes of at most 2000 characters.")
                .put("accepted_response_shapes", acceptedShapes)
                .put("tool_call_rules", new JSONArray()
                        .put("Use the exact top-level property tool_calls for tool use.")
                        .put("Each tool call must contain exactly tool and args.")
                        .put("tool must be a non-empty string matching one entry in tool_specs.")
                        .put("args must be an object containing that tool's documented arguments."))
                .put("invalid_aliases", new JSONObject()
                        .put("calls", "Use tool_calls instead.")
                        .put("name", "Inside each tool call, use tool instead.")
                        .put("function", "Inside each tool call, use tool instead.")
                        .put("arguments", "Inside each tool call, use args instead.")
                        .put("type", "Do not use type for tool calls.")
                        .put("source", "For write_symbol, use new_source instead."));
    }

    private static JSONArray unsupportedJsonKeys(JSONObject object, String... allowed) {
        JSONArray unsupported = new JSONArray();
        if (object == null) {
            return unsupported;
        }
        HashSet<String> allowedSet = new HashSet<>();
        for (String name : allowed) {
            allowedSet.add(name);
        }
        Iterator<String> keys = object.keys();
        while (keys.hasNext()) {
            String key = keys.next();
            if (!allowedSet.contains(key)) {
                unsupported.put(key);
            }
        }
        return unsupported;
    }

    private static JSONArray validateAiResponseShape(JSONObject response) throws Exception {
        JSONArray errors = new JSONArray();
        String mode = response.optString("mode", "");
        Object rawWorkingNotes = response.opt("working_notes");
        if (!(rawWorkingNotes instanceof String)
                || !WorkshopAiWorkingNotes.isValid((String)rawWorkingNotes)) {
            errors.put(new JSONObject()
                    .put("kind", "validation_error")
                    .put("error", "response requires nonempty string working_notes within 2000 characters")
                    .put("maximum_characters", WorkshopAiWorkingNotes.MAX_CHARS)
                    .put("accepted_shape", "Intent: ... Observed: ... Next: ... Blocker: ..."));
            return errors;
        }
        if (!"tool_calls".equals(mode) && !"done".equals(mode) && !"edits".equals(mode)) {
            errors.put(new JSONObject()
                    .put("kind", "validation_error")
                    .put("error", "response requires top-level mode equal to tool_calls, done, or edits")
                    .put("received_mode", mode)
                    .put("received_keys", response.names() == null ? new JSONArray() : response.names())
                    .put("response_contract", aiResponseContract()));
            return errors;
        }
        JSONArray unsupported = "tool_calls".equals(mode)
                ? unsupportedJsonKeys(response, "mode", "working_notes", "summary", "tool_calls")
                : ("done".equals(mode)
                        ? unsupportedJsonKeys(response, "mode", "working_notes", "summary")
                        : unsupportedJsonKeys(response, "mode", "working_notes", "summary", "edits"));
        if (unsupported.length() > 0) {
            errors.put(new JSONObject()
                    .put("kind", "validation_error")
                    .put("error", "response contains unsupported top-level properties for this mode")
                    .put("mode", mode)
                    .put("unsupported_properties", unsupported)
                    .put("response_contract", aiResponseContract()));
            return errors;
        }
        if ("tool_calls".equals(mode)) {
            JSONArray toolCalls = response.optJSONArray("tool_calls");
            if (toolCalls == null) {
                errors.put(new JSONObject()
                        .put("kind", "validation_error")
                        .put("error", "mode=tool_calls requires top-level tool_calls array")
                        .put("response_contract", aiResponseContract()));
                return errors;
            }
            if (toolCalls.length() > MAX_AI_TOOL_CALLS_PER_BATCH) {
                errors.put(new JSONObject()
                        .put("kind", "validation_error")
                        .put("error", "tool-call batch exceeds the bounded per-turn limit")
                        .put("received", toolCalls.length())
                        .put("maximum", MAX_AI_TOOL_CALLS_PER_BATCH)
                        .put("correction_instruction", "Request only the minimum tools needed for the next decision."));
                return errors;
            }
            for (int index = 0; index < toolCalls.length(); index += 1) {
                JSONObject call = toolCalls.optJSONObject(index);
                if (call == null) {
                    errors.put(new JSONObject()
                            .put("kind", "validation_error")
                            .put("index", index)
                            .put("error", "each tool call must be an object with tool and args"));
                    continue;
                }
                JSONArray callUnsupported = unsupportedJsonKeys(call, "tool", "args");
                if (callUnsupported.length() > 0) {
                    errors.put(new JSONObject()
                            .put("kind", "validation_error")
                            .put("index", index)
                            .put("error", "tool call contains unsupported top-level properties")
                            .put("unsupported_properties", callUnsupported)
                            .put("accepted_shape", new JSONObject().put("tool", "read_symbol").put("args", new JSONObject().put("name", "tick"))));
                } else if (call.optString("tool", "").trim().isEmpty() || call.optJSONObject("args") == null) {
                    errors.put(new JSONObject()
                            .put("kind", "validation_error")
                            .put("index", index)
                            .put("error", "tool call requires non-empty string property tool and object property args"));
                }
            }
        } else if ("edits".equals(mode) && response.optJSONArray("edits") == null) {
            errors.put(new JSONObject()
                    .put("kind", "validation_error")
                    .put("error", "mode=edits requires top-level edits array")
                    .put("response_contract", aiResponseContract()));
        }
        return errors;
    }
    private AiAgentResult runAiAgentLoop(String apiKey, String model, String initialRequestJson,
            boolean allowImageGeneration, boolean useCodex) throws Exception {
        String currentRequestJson = initialRequestJson;
        AiAgentSession session = new AiAgentSession();
        AiUsageAccumulator usage = new AiUsageAccumulator();
        ArrayList<AiGeneratedImageCandidate> generatedImages = new ArrayList<>();
        String previousToolCallBatch = "";
        for (int turn = 0; turn < MAX_AI_AGENT_TURNS; turn += 1) {
            throwIfAiCancelled();
            double monthlyLimitUsd = configuredAiLimit(AI_PREF_MONTHLY_LIMIT_USD, "5.00");
            if (!useCodex && !WorkshopAiBudgetPolicy.canStart(monthlyLimitUsd, monthlyAiSpendUsd())) {
                throw new IOException("Device monthly AI spending limit reached before agent turn " + (turn + 1));
            }
            session.currentStep = turn + 1;
            postAiProgress(session.currentStep, session.actionCount, "calling AI");
            appendAiTrace("llm_request", new JSONObject()
                    .put("turn", session.currentStep)
                    .put("model", model)
                    .put("summary", summarizeAiRequestForTrace(currentRequestJson)));
            double remainingUsd = WorkshopAiBudgetPolicy.remainingUsd(monthlyLimitUsd, monthlyAiSpendUsd());
            boolean allowImageOnThisTurn = !useCodex && allowImageGeneration && turn == 0;
            AiApiResponse apiResponse;
            if (useCodex) {
                apiResponse = callCodexResponses(currentRequestJson);
                usage.addUnpriced(apiResponse.model, apiResponse.usage);
            } else {
                int maxOutputTokens = maxOutputTokensForBudget(
                        model, currentRequestJson, remainingUsd, allowImageOnThisTurn);
                apiResponse = callOpenAiResponsesApi(
                        apiKey, model, currentRequestJson, maxOutputTokens, allowImageOnThisTurn);
                usage.add(model, apiResponse.usage);
                if (usage.lastCallCostAvailable) recordMonthlyAiSpend(usage.lastCallEstimatedCostUsd);
            }
            List<AiGeneratedImageCandidate> callImages = extractAiGeneratedImages(apiResponse.body);
            if (!callImages.isEmpty()) {
                generatedImages.addAll(callImages);
                double imageCost = callImages.size() * GPT_IMAGE_2_LOW_1024_USD;
                usage.addImageGenerationCost(imageCost, callImages.size());
                recordMonthlyAiSpend(imageCost);
            }
            throwIfAiCancelled();
            String aiJson = extractAiJsonResponse(apiResponse.body);
            JSONObject response = new JSONObject(aiJson);
            appendAiTrace("llm_response", new JSONObject()
                    .put("turn", session.currentStep)
                    .put("summary", summarizeAiResponseForTrace(apiResponse.body, response)));
            appendAiTrace("llm_json", new JSONObject().put("turn", session.currentStep).put("response", response));
            JSONArray responseValidationErrors = validateAiResponseShape(response);
            if (responseValidationErrors.length() > 0) {
                postAiProgress(session.currentStep, session.actionCount, "invalid response");
                appendAiTrace("response_validation_errors", new JSONObject().put("turn", session.currentStep).put("errors", responseValidationErrors));
                if (turn + 1 >= MAX_AI_AGENT_TURNS) {
                    throw new IOException("AI response shape invalid: " + responseValidationErrors.toString());
                }
                JSONObject followup = new JSONObject();
                followup.put("original_request", new JSONObject(initialRequestJson));
                followup.put("tool_observations", responseValidationErrors);
                followup.put("response_contract", aiResponseContract());
                if (!session.workingNotes.isEmpty()) {
                    followup.put("working_notes", session.workingNotes);
                }
                followup.put("instruction", "Your previous JSON response shape was invalid. Return exactly one JSON object matching the stable request response_contract, including nonempty working_notes within 2000 characters. For tool use, use mode=tool_calls and a top-level tool_calls array. Each call must be {\"tool\":\"name\",\"args\":{...}} with no aliases such as calls, name, function, arguments, type, or source.");
                currentRequestJson = followup.toString();
                continue;
            }
            session.workingNotes = WorkshopAiWorkingNotes.normalize(
                    response.getString("working_notes"));
            postAiWorkingNotes(session.workingNotes);
            appendAiTrace("working_notes", new JSONObject()
                    .put("turn", session.currentStep)
                    .put("notes", session.workingNotes));
            String mode = response.getString("mode");
            JSONArray toolCalls = response.optJSONArray("tool_calls");
            if (!"tool_calls".equals(mode) || toolCalls == null || toolCalls.length() == 0) {
                postAiProgress(session.currentStep, session.actionCount, "finalizing");
                return new AiAgentResult(aiJson, usage.toJson(model),
                        useCodex ? usage.subscriptionSummary() : usage.summary(), session.currentStep,
                        session.actionCount, generatedImages);
            }
            String currentToolCallBatch = toolCalls.toString();
            if (currentToolCallBatch.equals(previousToolCallBatch)) {
                postAiProgress(session.currentStep, session.actionCount, "repeated tools");
                JSONObject repeated = new JSONObject()
                        .put("mode", "done")
                        .put("working_notes", session.workingNotes)
                        .put("summary", "Stopped after repeated identical tool calls")
                        .put("tool_calls", new JSONArray())
                        .put("edits", new JSONArray())
                        .put("expected_reload", reloadKind(lastCompileResult))
                        .put("reason", "The model returned the exact same tool-call batch twice in a row.")
                        .put("warning", "repeated_tool_calls")
                        .put("repeated_tool_calls", toolCalls)
                        .put("successful_writes", session.successfulWriteCount)
                        .put("rolled_back_writes", session.rolledBackWriteCount)
                        .put("last_tool", session.lastToolSummary)
                        .put("last_error", session.lastToolError);
                appendAiTrace("repeated_tool_calls", repeated);
                if (session.successfulWriteCount > 0 && compileReady && session.latestRunnableTestsPassed()) {
                    return new AiAgentResult(repeated.toString(), usage.toJson(model),
                            useCodex ? usage.subscriptionSummary() : usage.summary(), session.currentStep,
                            session.actionCount, generatedImages);
                }
                throw new IOException("AI repeated identical tool calls; actions=" + session.actionCount + " successful_writes=" + session.successfulWriteCount + " rolled_back_writes=" + session.rolledBackWriteCount + " last_tool=" + session.lastToolSummary + " last_error=" + session.lastToolError);
            }
            previousToolCallBatch = currentToolCallBatch;
            postAiProgress(session.currentStep, session.actionCount, "tools " + toolCalls.length());
            appendAiTrace("tool_calls", new JSONObject().put("turn", session.currentStep).put("tool_calls", toolCalls));
            boolean batchHasWrites = aiToolCallsContainWrites(toolCalls);
            boolean blockedReadOnlyBatch = !session.toolLoopPolicy.shouldExecute(batchHasWrites);
            JSONArray observations;
            if (blockedReadOnlyBatch) {
                observations = new JSONArray().put(new JSONObject()
                        .put("kind", "progress_policy")
                        .put("status", "read_only_batch_not_executed")
                        .put("error", "Inspection is complete; the next response must write the intended change or return done.")
                        .put("retained_observation_count", session.observationMemory.size()));
            } else {
                observations = executeAiToolCalls(toolCalls, session);
                session.rememberToolObservations(observations);
            }
            session.toolLoopPolicy.recordBatch(batchHasWrites);
            throwIfAiCancelled();
            appendAiTrace("tool_observations", new JSONObject().put("turn", session.currentStep).put("observations", observations));
            JSONObject testObservation;
            if (batchHasWrites) {
                testObservation = runAiTestsAfterBatch(session);
                session.latestTestObservation = testObservation;
            } else {
                testObservation = new JSONObject()
                        .put("kind", "test_run")
                        .put("status", "not_run_for_read_only_batch");
            }
            appendAiTrace("test_observation", new JSONObject().put("turn", session.currentStep).put("result", testObservation));
            if (batchHasWrites && WorkshopAiFastPathPolicy.canAutoFinalize(
                    aiToolCallsContainTestWrite(toolCalls), session.successfulWriteCount,
                    compileReady, session.latestRunnableTestsPassed())) {
                JSONObject completed = new JSONObject()
                        .put("mode", "done")
                        .put("working_notes", session.workingNotes)
                        .put("summary", "Applied and tested " + session.successfulWriteCount + " tool write(s)")
                        .put("reason", "Successful tool writes compiled and all runnable tests passed; skipped a redundant final model call.");
                appendAiTrace("auto_finalize_tested_writes", completed);
                return new AiAgentResult(completed.toString(), usage.toJson(model),
                        useCodex ? usage.subscriptionSummary() : usage.summary(), session.currentStep,
                        session.actionCount, generatedImages);
            }
            JSONObject followup = new JSONObject();
            followup.put("original_request", new JSONObject(initialRequestJson));
            followup.put("tool_observations", session.retainedToolObservations());
            followup.put("latest_tool_observations", observations);
            followup.put("test_observation", testObservation);
            followup.put("tool_specs", aiToolSpecs());
            followup.put("working_notes", session.workingNotes);
            String instruction = "Use the retained tool_observations and working_notes as cumulative memory; update working_notes with concise Intent, Observed, Next, and Blocker facts on this response. Do not expose private chain-of-thought. Do not read targets already present in retained observations. Inspect only the minimum missing context needed for the requested change. Apply code changes with write_symbol, delete_symbol, write_imports, write_test_file, or delete_test_file before final edits so compile failures and test results return observations you can correct. Tool errors, validation_error observations, and test failures are not final; correct them. Return mode=edits only after the intended code has been written, compiled, and the latest runnable tests pass. If no further action is needed, return mode=done.";
            if (session.toolLoopPolicy.requiresWriteOrDone()) {
                instruction += " You have completed the maximum read-only inspection batches. Your next response must contain at least one write tool call or mode=done; do not request list/read/diagnostic tools.";
            }
            followup.put("instruction", instruction);
            currentRequestJson = followup.toString();
        }
        postAiProgress(MAX_AI_AGENT_TURNS, session.actionCount, "limit hit");
        if (session.successfulWriteCount > 0 && compileReady && session.latestRunnableTestsPassed()) {
            String summary = "Applied " + session.successfulWriteCount + " tool write(s) before response limit";
            JSONObject synthetic = new JSONObject()
                    .put("mode", "done")
                    .put("working_notes", session.workingNotes)
                    .put("summary", summary)
                    .put("tool_calls", new JSONArray())
                    .put("edits", new JSONArray())
                    .put("expected_reload", reloadKind(lastCompileResult))
                    .put("reason", "The model reached the tool-call limit after successful write_symbol calls with passing runnable tests; accepted tested tool writes.")
                    .put("warning", "tool_call_limit_after_successful_tested_writes")
                    .put("successful_writes", session.successfulWriteCount)
                    .put("rolled_back_writes", session.rolledBackWriteCount)
                    .put("last_tool", session.lastToolSummary)
                    .put("last_error", session.lastToolError);
            appendAiTrace("limit_after_successful_tested_writes", synthetic);
            return new AiAgentResult(synthetic.toString(), usage.toJson(model),
                    useCodex ? usage.subscriptionSummary() : usage.summary(), MAX_AI_AGENT_TURNS,
                    session.actionCount, generatedImages);
        }
        throw new IOException("AI agent reached tool-call limit before returning edits; actions=" + session.actionCount + " successful_writes=" + session.successfulWriteCount + " rolled_back_writes=" + session.rolledBackWriteCount + " last_tool=" + session.lastToolSummary + " last_error=" + session.lastToolError);
    }

    private JSONArray executeAiToolCalls(JSONArray toolCalls, AiAgentSession session) throws Exception {
        JSONArray observations = new JSONArray();
        boolean batchHasWrites = false;
        ArrayList<Integer> pendingRunTestObservationIndexes = new ArrayList<>();
        for (int index = 0; index < toolCalls.length(); index += 1) {
            JSONObject call = toolCalls.getJSONObject(index);
            String tool = call.optString("tool", "");
            if (isAiWriteTool(tool)) {
                batchHasWrites = true;
                break;
            }
        }

        Map<String, String> batchOriginalSources = batchHasWrites ? snapshotProjectSources(session.project()) : null;
        throwIfAiCancelled();
        session.deferBatchCompile = batchHasWrites;
        try {
            for (int index = 0; index < toolCalls.length(); index += 1) {
                if (!batchHasWrites) throwIfAiCancelled();
                JSONObject call = toolCalls.getJSONObject(index);
                JSONObject observation = new JSONObject();
                String tool = call.optString("tool", "");
                JSONObject args = call.optJSONObject("args");
                if (args == null) {
                    args = new JSONObject();
                }
                observation.put("tool", tool);
                observation.put("args", args);
                session.actionCount += 1;
                postAiProgress(session.currentStep, session.actionCount, tool.isEmpty() ? "tool" : tool);
                JSONObject validationError = validateAiToolCall(tool, args);
                if (validationError != null) {
                    session.lastToolSummary = tool.isEmpty() ? "invalid_tool_call" : "invalid_tool_call " + tool;
                    session.lastToolError = validationError.optString("error", "invalid tool call");
                    observation.put("error", session.lastToolError);
                    observation.put("validation", validationError);
                    observations.put(observation);
                    continue;
                }
                if (batchHasWrites && "run_tests".equals(tool)) {
                    observation.put("result", new JSONObject().put("status", "pending_batch_compile"));
                    observations.put(observation);
                    pendingRunTestObservationIndexes.add(observations.length() - 1);
                    continue;
                }
                try {
                    JSONObject result = executeAiToolCall(tool, args, session);
                    observation.put("result", result);
                    if (!batchHasWrites || !isAiWriteTool(tool)) {
                        recordAiToolResult(session, tool, result);
                    }
                } catch (Exception error) {
                    session.lastToolError = error.getMessage();
                    observation.put("error", error.getMessage());
                }
                observations.put(observation);
            }
        } finally {
            session.deferBatchCompile = false;
        }

        if (!batchHasWrites) {
            return observations;
        }

        String compileResult = nativeCompileProject(projectRootPath());
        lastCompileResult = compileResult;
        compileReady = isRunnableCompile(compileResult);
        compileAttempted = true;
        JSONObject diagnostics = compileResultToJson(compileResult);
        if (!compileReady) {
            restoreProjectSources(batchOriginalSources);
            session.invalidateProject();
            String restoredCompile = nativeCompileProject(projectRootPath());
            lastCompileResult = restoredCompile;
            compileReady = isRunnableCompile(restoredCompile);
            compileAttempted = true;
            JSONObject restoredDiagnostics = compileResultToJson(restoredCompile);
            annotateAiBatchWriteResults(observations, "rolled_back", diagnostics, restoredDiagnostics, session);
            annotatePendingRunTestsBlocked(observations, pendingRunTestObservationIndexes, diagnostics);
            return observations;
        }

        annotateAiBatchWriteResults(observations, "compiled", diagnostics, null, session);
        runPendingBatchTests(observations, pendingRunTestObservationIndexes, session);
        return observations;
    }

    private void annotatePendingRunTestsBlocked(JSONArray observations, ArrayList<Integer> indexes, JSONObject diagnostics) throws Exception {
        for (int index : indexes) {
            observations.getJSONObject(index).put("result", new JSONObject()
                    .put("status", "blocked_by_compile_failure")
                    .put("diagnostics", diagnostics));
        }
    }

    private void runPendingBatchTests(JSONArray observations, ArrayList<Integer> indexes, AiAgentSession session) throws Exception {
        for (int index : indexes) {
            observations.getJSONObject(index).put("result", aiToolRunTests(session));
        }
    }
    private static boolean isAiWriteTool(String tool) {
        return "write_symbol".equals(tool) || "delete_symbol".equals(tool) || "write_imports".equals(tool) || "write_test_file".equals(tool) || "delete_test_file".equals(tool);
    }

    private static boolean aiToolCallsContainWrites(JSONArray toolCalls) {
        for (int index = 0; index < toolCalls.length(); index += 1) {
            JSONObject call = toolCalls.optJSONObject(index);
            if (call != null && isAiWriteTool(call.optString("tool", ""))) return true;
        }
        return false;
    }

    private static boolean aiToolCallsContainTestWrite(JSONArray toolCalls) {
        for (int index = 0; index < toolCalls.length(); index += 1) {
            JSONObject call = toolCalls.optJSONObject(index);
            if (call != null && "write_test_file".equals(call.optString("tool", ""))) return true;
        }
        return false;
    }

    private void annotateAiBatchWriteResults(JSONArray observations, String batchStatus, JSONObject diagnostics, JSONObject restoredDiagnostics, AiAgentSession session) throws Exception {
        for (int index = 0; index < observations.length(); index += 1) {
            JSONObject observation = observations.getJSONObject(index);
            String tool = observation.optString("tool", "");
            if (!isAiWriteTool(tool)) {
                continue;
            }
            JSONObject result = observation.optJSONObject("result");
            if (result == null) {
                continue;
            }
            String status = result.optString("status", "");
            if (!"written".equals(status) && !"created".equals(status)) {
                continue;
            }
            result.put("diagnostics", diagnostics);
            if ("rolled_back".equals(batchStatus)) {
                result.put("status", "rolled_back");
                if (restoredDiagnostics != null) {
                    result.put("restored_diagnostics", restoredDiagnostics);
                }
            }
            recordAiToolResult(session, tool, result);
        }
    }
    private static JSONObject validateAiToolCall(String tool, JSONObject args) throws Exception {
        if (tool == null || tool.trim().isEmpty()) {
            return aiToolValidationError(tool, args, "Tool call is missing required string field: tool", new JSONArray().put("tool").put("args"));
        }
        JSONArray required = requiredArgsForAiTool(tool);
        if (required == null) {
            return aiToolValidationError(tool, args, "Unsupported AI tool: " + tool, supportedAiTools());
        }
        for (int index = 0; index < required.length(); index += 1) {
            String name = required.getString(index);
            if ("new_source".equals(name)) {
                if (!hasTextArg(args, "new_source")) {
                    return aiToolValidationError(tool, args, "Tool " + tool + " requires arg: new_source", required);
                }
            } else if ("imports".equals(name)) {
                if (!args.has("imports")) {
                    return aiToolValidationError(tool, args, "Tool " + tool + " requires arg: imports", required);
                }
            } else if (!hasTextArg(args, name)) {
                return aiToolValidationError(tool, args, "Tool " + tool + " requires arg: " + name, required);
            }
        }
        return null;
    }

    private static boolean hasTextArg(JSONObject args, String name) {
        return args != null && args.has(name) && !args.optString(name, "").trim().isEmpty();
    }

    private static JSONArray requiredArgsForAiTool(String tool) {
        if ("list_symbols".equals(tool)
                || "get_diagnostics".equals(tool)
                || "run_frame".equals(tool)
                || "inspect_runtime_state".equals(tool)
                || "take_screenshot".equals(tool)
                || "set_input_state".equals(tool)
                || "list_tests".equals(tool)
                || "run_tests".equals(tool)) {
            return new JSONArray();
        }
        if ("read_symbol".equals(tool)) {
            return new JSONArray().put("name");
        }
        if ("list_owner_symbols".equals(tool)) {
            return new JSONArray().put("owner");
        }
        if ("read_imports".equals(tool) || "read_test_file".equals(tool)) {
            return new JSONArray().put("file");
        }
        if ("write_test_file".equals(tool)) {
            return new JSONArray().put("file").put("source");
        }
        if ("write_imports".equals(tool)) {
            return new JSONArray().put("file").put("imports");
        }
        if ("write_symbol".equals(tool)) {
            return new JSONArray().put("file").put("name").put("new_source");
        }
        if ("delete_symbol".equals(tool)) {
            return new JSONArray().put("name");
        }
        if ("delete_test_file".equals(tool)) {
            return new JSONArray().put("file");
        }

        return null;
    }

    private static JSONArray supportedAiTools() {
        return new JSONArray()
                .put("list_symbols")
                .put("list_owner_symbols")
                .put("read_symbol")
                .put("read_imports")
                .put("write_imports")
                .put("write_symbol")
                .put("delete_symbol")
                .put("get_diagnostics")
                .put("set_input_state")
                .put("run_frame")
                .put("inspect_runtime_state")
                .put("take_screenshot")
                .put("list_tests")
                .put("read_test_file")
                .put("write_test_file")
                .put("delete_test_file")
                .put("run_tests");
    }

    private static JSONArray aiToolSpecs() throws Exception {
        JSONArray specs = new JSONArray();
        specs.put(aiToolSpec("list_symbols", "List all editable symbols, including globals.", new JSONArray(), new JSONArray(), new JSONObject()));
        specs.put(aiToolSpec("list_owner_symbols", "List compact symbols owned by a type/group such as Player, Main, Root, Globals, or a system owner. Use this to discover receiver-style functions available for a type.", new JSONArray().put("owner"), new JSONArray(), new JSONObject().put("owner", "Player")));
        specs.put(aiToolSpec("read_symbol", "Read one function, struct, or global symbol. Globals include backing_struct_source.", new JSONArray().put("name"), new JSONArray().put("kind").put("file").put("owner"), new JSONObject().put("name", "GameState").put("kind", "global")));
        specs.put(aiToolSpec("read_imports", "Read one file's import block as import paths.", new JSONArray().put("file"), new JSONArray(), new JSONObject().put("file", "src/main.stasis")));
        specs.put(aiToolSpec("write_imports", "Replace one file's top import block. Writes in one tool-call batch compile together and roll back together on failure.", new JSONArray().put("file").put("imports"), new JSONArray(), new JSONObject().put("file", "src/main.stasis").put("imports", new JSONArray().put("game_state.stasis").put("systems/collision.stasis"))));
        specs.put(aiToolSpec("write_symbol", "Create or replace a function/struct symbol. Writes in one tool-call batch compile together and roll back together on failure.", new JSONArray().put("file").put("name").put("new_source"), new JSONArray().put("kind").put("owner"), new JSONObject().put("file", "src/main.stasis").put("name", "tick").put("kind", "replace_function").put("owner", "Main").put("new_source", "function tick(): void {\n    // ...\n}")));
        specs.put(aiToolSpec("delete_symbol", "Delete one obsolete function/struct/global symbol by name, with optional file/kind/owner disambiguation. Deletes compile with the current batch and roll back on failure.", new JSONArray().put("name"), new JSONArray().put("file").put("kind").put("owner"), new JSONObject().put("file", "src/main.stasis").put("name", "unused_helper").put("kind", "function")));
        specs.put(aiToolSpec("get_diagnostics", "Return the last compile diagnostics.", new JSONArray(), new JSONArray(), new JSONObject()));
        specs.put(aiToolSpec("set_input_state", "Set simulated mobile input for tests.", new JSONArray(), new JSONArray().put("x").put("y").put("active").put("screen_w").put("screen_h"), new JSONObject().put("x", 180).put("y", 320).put("active", 1)));
        specs.put(aiToolSpec("run_frame", "Run one tick/render frame with current simulated input.", new JSONArray(), new JSONArray(), new JSONObject()));
        specs.put(aiToolSpec("inspect_runtime_state", "Read compact runtime state and last frame.", new JSONArray(), new JSONArray(), new JSONObject()));
        specs.put(aiToolSpec("take_screenshot", "Return a logical render snapshot, decoded commands, runtime state, and input.", new JSONArray(), new JSONArray(), new JSONObject()));
        specs.put(aiToolSpec("list_tests", "List Stasis .test.stasis files.", new JSONArray(), new JSONArray(), new JSONObject()));
        specs.put(aiToolSpec("read_test_file", "Read one Stasis test file under tests/.", new JSONArray().put("file"), new JSONArray(), new JSONObject().put("file", "tests/paddle.test.stasis")));
        specs.put(aiToolSpec("write_test_file", "Create or replace a Stasis test under tests/. Use test `name`(): bool and return true or false; assert_runtime helpers and JSON scenarios are not Stasis syntax.", new JSONArray().put("file").put("source"), new JSONArray(), new JSONObject().put("file", "tests/paddle.test.stasis").put("source", "import \"../src/main.stasis\";\n\ntest `paddle follows touch`(): bool {\n    return true;\n}")));
        specs.put(aiToolSpec("delete_test_file", "Delete one obsolete or duplicate Stasis test file under tests/.", new JSONArray().put("file"), new JSONArray(), new JSONObject().put("file", "tests/obsolete.test.stasis")));
        specs.put(aiToolSpec("run_tests", "Compile the Android project and report Stasis test files for host JIT execution.", new JSONArray(), new JSONArray(), new JSONObject()));
        return specs;
    }

    private static JSONObject aiToolSpec(String tool, String purpose, JSONArray requiredArgs, JSONArray optionalArgs, JSONObject exampleArgs) throws Exception {
        return new JSONObject()
                .put("tool", tool)
                .put("purpose", purpose)
                .put("required_args", requiredArgs)
                .put("optional_args", optionalArgs)
                .put("example", new JSONObject().put("tool", tool).put("args", exampleArgs));
    }
    private static JSONObject aiToolValidationError(String tool, JSONObject args, String error, JSONArray requiredArgs) throws Exception {
        JSONObject acceptedArgs = new JSONObject();
        String normalizedTool = tool == null ? "" : tool;
        if ("list_owner_symbols".equals(normalizedTool)) {
            acceptedArgs.put("owner", "Player");
        } else if ("read_symbol".equals(normalizedTool)) {
            acceptedArgs.put("name", "symbol_name").put("kind", "function_struct_or_global_optional").put("file", "src/main.stasis_optional").put("owner", "owner_optional");
        } else if ("read_imports".equals(normalizedTool)) {
            acceptedArgs.put("file", "src/main.stasis");
        } else if ("write_imports".equals(normalizedTool)) {
            acceptedArgs.put("file", "src/main.stasis").put("imports", new JSONArray().put("game_state.stasis").put("systems/collision.stasis"));
        } else if ("read_test_file".equals(normalizedTool)) {
            acceptedArgs.put("file", "tests/paddle.test.stasis");
        } else if ("write_test_file".equals(normalizedTool)) {
            acceptedArgs.put("file", "tests/paddle.test.stasis").put("source", "import \"../src/main.stasis\";\n\ntest `paddle follows touch`(): bool {\n    return true;\n}");
        } else if ("delete_test_file".equals(normalizedTool)) {
            acceptedArgs.put("file", "tests/obsolete.test.stasis");
        } else if ("delete_symbol".equals(normalizedTool)) {
            acceptedArgs.put("file", "src/main.stasis").put("name", "unused_helper").put("kind", "function").put("owner", "Root");
        } else if ("write_symbol".equals(normalizedTool)) {
            acceptedArgs.put("file", "src/main.stasis").put("name", "function_name").put("kind", "replace_function").put("owner", "Root").put("new_source", "function function_name(): void {\n    // ...\n}");
        } else if ("set_input_state".equals(normalizedTool)) {
            acceptedArgs.put("x", 180).put("y", 320).put("active", 1).put("screen_w", 360).put("screen_h", 640);
        }
        return new JSONObject()
                .put("kind", "validation_error")
                .put("error", error)
                .put("tool", normalizedTool)
                .put("received_args", args == null ? new JSONObject() : args)
                .put("required_args", requiredArgs)
                .put("accepted_shape", new JSONObject().put("tool", normalizedTool.isEmpty() ? "tool_name" : normalizedTool).put("args", acceptedArgs))
                .put("correction_instruction", "Return another mode=tool_calls response with corrected JSON for this tool, or mode=done with no actions if no work remains.");
    }

    private void recordAiToolResult(AiAgentSession session, String tool, JSONObject result) {
        session.lastToolSummary = tool;
        if (result == null) {
            return;
        }
        String status = result.optString("status", "");
        String file = result.optString("file", "");
        String name = result.optString("name", "");
        if (!file.isEmpty() || !name.isEmpty() || !status.isEmpty()) {
            session.lastToolSummary = tool + " " + file + " " + name + " " + status;
        }
        if ("write_symbol".equals(tool) || "delete_symbol".equals(tool) || "write_imports".equals(tool) || "write_test_file".equals(tool) || "delete_test_file".equals(tool)) {
            if ("written".equals(status) || "created".equals(status) || "deleted".equals(status)) {
                session.successfulWriteCount += 1;
                session.lastToolError = "";
            } else if ("rolled_back".equals(status)) {
                session.rolledBackWriteCount += 1;
                JSONObject diagnostics = result.optJSONObject("diagnostics");
                session.lastToolError = diagnostics == null ? "rolled_back" : diagnostics.optString("raw", "rolled_back");
            }
        }
    }
    private JSONObject executeAiToolCall(String tool, JSONObject args, AiAgentSession session) throws Exception {
        if ("list_symbols".equals(tool)) {
            return aiToolListSymbols(session);
        }
        if ("list_owner_symbols".equals(tool)) {
            return aiToolListOwnerSymbols(session, args);
        }
        if ("read_symbol".equals(tool)) {
            return aiToolReadSymbol(session, args);
        }
        if ("read_imports".equals(tool)) {
            return aiToolReadImports(session, args);
        }
        if ("write_imports".equals(tool)) {
            return aiToolWriteImports(session, args);
        }
        if ("write_symbol".equals(tool)) {
            return aiToolWriteSymbol(session, args);
        }
        if ("delete_symbol".equals(tool)) {
            return aiToolDeleteSymbol(session, args);
        }

        if ("get_diagnostics".equals(tool)) {
            return aiToolGetDiagnostics();
        }
        if ("set_input_state".equals(tool)) {
            return aiToolSetInputState(args);
        }

        if ("run_frame".equals(tool)) {
            return aiToolRunFrame();
        }

        if ("inspect_runtime_state".equals(tool)) {
            return aiToolInspectRuntimeState();
        }
        if ("take_screenshot".equals(tool)) {
            return aiToolTakeScreenshot();
        }
        if ("list_tests".equals(tool)) {
            return aiToolListTests();
        }
        if ("read_test_file".equals(tool)) {
            return aiToolReadTestFile(args);
        }
        if ("write_test_file".equals(tool)) {
            return aiToolWriteTestFile(session, args);
        }
        if ("delete_test_file".equals(tool)) {
            return aiToolDeleteTestFile(session, args);
        }
        if ("run_tests".equals(tool)) {
            return aiToolRunTests(session);
        }
        throw new IOException("Unsupported AI tool: " + tool);
    }

    private JSONObject aiToolListSymbols(AiAgentSession session) throws Exception {
        ProjectSnapshot project = session.project();
        JSONArray symbols = new JSONArray();
        for (SymbolSection section : project.sections) {
            for (SymbolGroup group : section.groups) {
                for (SymbolEntry symbol : group.symbols) {
                    symbols.put(symbolToJson(symbol, false));
                }
            }
        }
        return new JSONObject()
                .put("symbol_count", symbols.length())
                .put("symbols", symbols);
    }

    private JSONObject aiToolListOwnerSymbols(AiAgentSession session, JSONObject call) throws Exception {
        ProjectSnapshot project = session.project();
        String owner = call.optString("owner", "").trim();
        if (owner.isEmpty()) {
            throw new IOException("list_owner_symbols requires owner");
        }
        JSONArray structs = new JSONArray();
        JSONArray globals = new JSONArray();
        JSONArray functions = new JSONArray();
        JSONArray others = new JSONArray();
        for (SymbolSection section : project.sections) {
            for (SymbolGroup group : section.groups) {
                for (SymbolEntry symbol : group.symbols) {
                    if (!owner.equals(symbol.owner) && !owner.equals(symbol.name)) {
                        continue;
                    }
                    JSONObject entry = symbolToJson(symbol, false);
                    if ("function".equals(symbol.kind)) {
                        functions.put(entry);
                    } else if ("struct".equals(symbol.kind)) {
                        structs.put(entry);
                    } else if ("global".equals(symbol.kind)) {
                        globals.put(entry);
                    } else {
                        others.put(entry);
                    }
                }
            }
        }
        return new JSONObject()
                .put("owner", owner)
                .put("structs", structs)
                .put("globals", globals)
                .put("functions", functions)
                .put("others", others)
                .put("symbol_count", structs.length() + globals.length() + functions.length() + others.length());
    }
    private JSONObject aiToolReadSymbol(AiAgentSession session, JSONObject call) throws Exception {
        ProjectSnapshot project = session.project();
        String kind = call.optString("kind", "");
        SymbolEntry target;
        if (kind.isEmpty()) {
            target = findAnySymbolForAiLookup(project, call, selectedSymbol);
        } else {
            String expectedKind = aiLookupExpectedKind(kind);
            target = findSymbolForAiEdit(project, expectedKind, call, selectedSymbol);
        }
        return symbolToJson(target, true);
    }

    private JSONObject aiToolReadFile(AiAgentSession session, JSONObject call) throws Exception {
        ProjectSnapshot project = session.project();
        SourceFile sourceFile = findProjectFile(project, call.optString("file", ""));
        return new JSONObject()
                .put("file", sourceFile.path)
                .put("source", sourceFile.source);
    }

    private JSONObject runAiTestsAfterBatch(AiAgentSession session) throws Exception {
        try {
            return aiToolRunTests(session);
        } catch (Exception error) {
            return new JSONObject()
                    .put("kind", "test_run")
                    .put("status", "error")
                    .put("error", error.getMessage());
        }
    }
    private JSONObject aiToolListTests() throws Exception {
        JSONArray files = new JSONArray();
        List<File> testFiles = listProjectTestFiles();
        for (File file : testFiles) {
            String relative = relativeProjectPath(file);
            files.put(new JSONObject()
                    .put("file", relative)
                    .put("kind", relative.endsWith(".ai_test.json") ? "ai_scenario" : "stasis_test")
                    .put("runnable_on_android", relative.endsWith(".ai_test.json")));
        }
        return new JSONObject()
                .put("kind", "tests")
                .put("test_count", files.length())
                .put("files", files);
    }

    private JSONObject aiToolReadTestFile(JSONObject call) throws Exception {
        File file = testFileForAiPath(call.optString("file", ""));
        return new JSONObject()
                .put("file", relativeProjectPath(file))
                .put("source", file.isFile() ? readTextFile(file) : "")
                .put("exists", file.isFile());
    }

    private JSONObject aiToolWriteTestFile(AiAgentSession session, JSONObject call) throws Exception {
        File file = testFileForAiPath(call.optString("file", ""));
        String source = call.optString("source", "");
        if (source.trim().isEmpty()) {
            throw new IOException("write_test_file requires non-empty source");
        }
        writeTextFile(file, source);
        session.invalidateProject();
        return new JSONObject()
                .put("file", relativeProjectPath(file))
                .put("kind", file.getName().endsWith(".ai_test.json") ? "ai_scenario" : "stasis_test")
                .put("status", "written")
                .put("runnable_on_android", file.getName().endsWith(".ai_test.json"));
    }

    private JSONObject aiToolDeleteTestFile(AiAgentSession session, JSONObject call) throws Exception {
        File file = testFileForAiPath(call.optString("file", ""));
        boolean existed = file.isFile();
        if (existed && !file.delete()) {
            throw new IOException("Failed to delete test file: " + relativeProjectPath(file));
        }
        session.invalidateProject();
        return new JSONObject()
                .put("file", relativeProjectPath(file))
                .put("status", existed ? "deleted" : "not_found");
    }
    private JSONObject aiToolRunTests(AiAgentSession session) throws Exception {
        String compileResult = nativeCompileProject(projectRootPath());
        lastCompileResult = compileResult;
        compileReady = isRunnableCompile(compileResult);
        compileAttempted = true;
        JSONObject compileJson = compileResultToJson(compileResult);

        JSONArray stasisTests = new JSONArray();
        TreeSet<String> passingKeys = new TreeSet<>();
        int passed = 0;
        int failed = 0;
        int pending = 0;
        JSONObject bridgeTestRun = null;
        for (File file : listProjectTestFiles()) {
            String relative = relativeProjectPath(file);
            if (relative.endsWith(".test.stasis")) {
                if (bridgeTestRun == null) {
                    bridgeTestRun = new JSONObject(nativeRunTests(projectRootPath()));
                    stasisTests.put(bridgeTestRun);
                    passed += bridgeTestRun.optInt("passed", 0);
                    failed += bridgeTestRun.optInt("failed", 0);
                }
            }
        }

        JSONArray newPassing = new JSONArray();
        for (String key : passingKeys) {
            if (!session.lastPassingTestKeys.contains(key)) {
                newPassing.put(key);
            }
        }
        session.lastPassingTestKeys = passingKeys;

        return new JSONObject()
                .put("kind", "test_run")
                .put("compile", compileJson)
                .put("passed", passed)
                .put("failed", failed)
                .put("pending", pending)
                .put("stasis_test_files", stasisTests)
                .put("new_passing_tests", newPassing)
                .put("all_runnable_tests_passed", passed > 0 && failed == 0 && compileJson.optBoolean("ok", false));
    }

    private JSONObject aiToolReadImports(AiAgentSession session, JSONObject call) throws Exception {
        ProjectSnapshot project = session.project();
        SourceFile sourceFile = findProjectFile(project, call.optString("file", ""));
        return importsToJson(sourceFile);
    }

    private JSONObject aiToolWriteImports(AiAgentSession session, JSONObject call) throws Exception {
        ProjectSnapshot project = session.project();
        SourceFile sourceFile = findProjectFile(project, call.optString("file", ""));
        Map<String, String> originalSources = snapshotProjectSources(project);
        JSONArray imports = normalizedImportPaths(call);
        String updatedSource = replaceImportBlock(sourceFile.source, imports);
        try {
            sourceFile.source = updatedSource;
            writeTextFile(sourceFile.diskFile, updatedSource);
            session.invalidateProject();

            if (session.deferBatchCompile) {
                return importsToJson(sourceFile)
                        .put("status", "written")
                        .put("diagnostics", new JSONObject().put("status", "pending_batch_compile"));
            }

            String compileResult = nativeCompileProject(projectRootPath());
            lastCompileResult = compileResult;
            compileReady = isRunnableCompile(compileResult);
            compileAttempted = true;
            JSONObject diagnostics = compileResultToJson(compileResult);
            if (!compileReady) {
                restoreProjectSources(originalSources);
                session.invalidateProject();
                String restoredCompile = nativeCompileProject(projectRootPath());
                lastCompileResult = restoredCompile;
                compileReady = isRunnableCompile(restoredCompile);
                compileAttempted = true;
                return importsToJson(new SourceFile(sourceFile.path, sourceFile.diskFile, originalSources.get(sourceFile.path)))
                        .put("status", "rolled_back")
                        .put("diagnostics", diagnostics)
                        .put("restored_diagnostics", compileResultToJson(restoredCompile));
            }
            return importsToJson(sourceFile)
                    .put("status", "written")
                    .put("diagnostics", diagnostics);
        } catch (Exception error) {
            restoreProjectSources(originalSources);
            session.invalidateProject();
            throw error;
        }
    }

    private static JSONObject importsToJson(SourceFile sourceFile) throws Exception {
        JSONArray imports = parseImportPaths(sourceFile.source);
        return new JSONObject()
                .put("file", sourceFile.path)
                .put("kind", "imports")
                .put("imports", imports)
                .put("source", importBlockSource(imports));
    }

    private static JSONArray normalizedImportPaths(JSONObject call) throws Exception {
        JSONArray out = new JSONArray();
        JSONArray imports = call.optJSONArray("imports");
        if (imports != null) {
            for (int index = 0; index < imports.length(); index += 1) {
                String path = normalizeImportPath(imports.getString(index));
                if (!path.isEmpty()) {
                    out.put(path);
                }
            }
            return out;
        }
        throw new IOException("write_imports requires imports array");
    }

    private static JSONArray parseImportPaths(String source) throws Exception {
        JSONArray imports = new JSONArray();
        String[] lines = source.split("\\r?\\n");
        for (String line : lines) {
            String trimmed = line.trim();
            if (trimmed.isEmpty()) {
                continue;
            }
            if (!trimmed.startsWith("import ")) {
                break;
            }
            String path = normalizeImportPath(trimmed);
            if (!path.isEmpty()) {
                imports.put(path);
            }
        }
        return imports;
    }

    private static String normalizeImportPath(String value) throws IOException {
        String trimmed = value == null ? "" : value.trim();
        if (trimmed.isEmpty()) {
            return "";
        }
        if (trimmed.startsWith("import ")) {
            int firstQuote = trimmed.indexOf('"');
            int secondQuote = firstQuote < 0 ? -1 : trimmed.indexOf('"', firstQuote + 1);
            if (firstQuote < 0 || secondQuote <= firstQuote + 1) {
                throw new IOException("Invalid import line: " + trimmed);
            }
            return trimmed.substring(firstQuote + 1, secondQuote);
        }
        if (trimmed.indexOf('"') >= 0 || trimmed.indexOf(';') >= 0) {
            throw new IOException("Import paths should not include quotes or semicolons: " + trimmed);
        }
        return trimmed;
    }

    private static String importBlockSource(JSONArray imports) throws Exception {
        StringBuilder builder = new StringBuilder();
        for (int index = 0; index < imports.length(); index += 1) {
            builder.append("import \"").append(imports.getString(index)).append("\";\n");
        }
        return builder.toString();
    }

    private static String replaceImportBlock(String source, JSONArray imports) throws Exception {
        int blockEnd = importBlockEnd(source);
        String rest = source.substring(blockEnd);
        while (rest.startsWith("\r\n") || rest.startsWith("\n")) {
            rest = rest.startsWith("\r\n") ? rest.substring(2) : rest.substring(1);
        }
        String importBlock = importBlockSource(imports);
        if (!importBlock.isEmpty() && !rest.isEmpty()) {
            importBlock += "\n";
        }
        return importBlock + rest;
    }

    private static int importBlockEnd(String source) {
        int offset = 0;
        while (offset < source.length()) {
            int lineEnd = source.indexOf('\n', offset);
            int nextOffset = lineEnd < 0 ? source.length() : lineEnd + 1;
            String line = source.substring(offset, lineEnd < 0 ? source.length() : lineEnd).trim();
            if (line.isEmpty() || line.startsWith("import ")) {
                offset = nextOffset;
                continue;
            }
            break;
        }
        return offset;
    }
    private JSONObject aiToolDeleteSymbol(AiAgentSession session, JSONObject call) throws Exception {
        ProjectSnapshot project = session.project();
        Map<String, String> originalSources = snapshotProjectSources(project);
        String kind = call.optString("kind", "");
        SymbolEntry target = kind.isEmpty()
                ? findAnySymbolForAiLookup(project, call, selectedSymbol)
                : findSymbolForAiEdit(project, aiLookupExpectedKind(kind), call, selectedSymbol);
        try {
            String before = target.sourceFile.source.substring(0, target.start).replaceFirst("\\s+$", "");
            String after = target.sourceFile.source.substring(target.end).replaceFirst("^\\s+", "");
            String updatedSource = before.isEmpty() ? after : after.isEmpty() ? before + "\n" : before + "\n\n" + after;
            target.sourceFile.source = updatedSource;
            writeTextFile(target.sourceFile.diskFile, updatedSource);
            session.invalidateProject();

            if (session.deferBatchCompile) {
                return new JSONObject()
                        .put("file", target.file)
                        .put("kind", target.kind)
                        .put("name", target.name)
                        .put("owner", target.owner)
                        .put("status", "deleted")
                        .put("diagnostics", new JSONObject().put("status", "pending_batch_compile"));
            }

            String compileResult = nativeCompileProject(projectRootPath());
            lastCompileResult = compileResult;
            compileReady = isRunnableCompile(compileResult);
            compileAttempted = true;
            JSONObject diagnostics = compileResultToJson(compileResult);
            if (!compileReady) {
                restoreProjectSources(originalSources);
                session.invalidateProject();
                String restoredCompile = nativeCompileProject(projectRootPath());
                lastCompileResult = restoredCompile;
                compileReady = isRunnableCompile(restoredCompile);
                compileAttempted = true;
                return new JSONObject()
                        .put("file", target.file)
                        .put("kind", target.kind)
                        .put("name", target.name)
                        .put("owner", target.owner)
                        .put("status", "rolled_back")
                        .put("diagnostics", diagnostics)
                        .put("restored_diagnostics", compileResultToJson(restoredCompile));
            }
            return new JSONObject()
                    .put("file", target.file)
                    .put("kind", target.kind)
                    .put("name", target.name)
                    .put("owner", target.owner)
                    .put("status", "deleted")
                    .put("diagnostics", diagnostics);
        } catch (Exception error) {
            restoreProjectSources(originalSources);
            session.invalidateProject();
            throw error;
        }
    }
    private JSONObject aiToolWriteSymbol(AiAgentSession session, JSONObject call) throws Exception {
        ProjectSnapshot project = session.project();
        Map<String, String> originalSources = snapshotProjectSources(project);
        String kind = call.optString("kind", "replace_function");
        String expectedKind = "replace_struct".equals(kind) || "struct".equals(kind) ? "struct" : "function";
        String editKind = "struct".equals(expectedKind) ? "replace_struct" : "replace_function";
        String newSource = call.optString("new_source", "").trim();
        if (newSource.isEmpty()) {
            throw new IOException("No value for new_source");
        }
        boolean existed = findSymbolForAiEditOrNull(project, expectedKind, call, selectedSymbol) != null;
        try {
            SymbolEntry target = resolveAiEditTarget(project, editKind, expectedKind, call, selectedSymbol, newSource);
            validateAiReplacementSource(editKind, target.name, newSource);
            persistSelectedEdit(target, newSource);
            session.invalidateProject();

            if (session.deferBatchCompile) {
                return new JSONObject()
                        .put("file", target.file)
                        .put("kind", target.kind)
                        .put("name", target.name)
                        .put("owner", target.owner)
                        .put("status", existed ? "written" : "created")
                        .put("diagnostics", new JSONObject().put("status", "pending_batch_compile"));
            }

            String compileResult = nativeCompileProject(projectRootPath());
            lastCompileResult = compileResult;
            compileReady = isRunnableCompile(compileResult);
            compileAttempted = true;
            JSONObject diagnostics = compileResultToJson(compileResult);
            if (!compileReady) {
                restoreProjectSources(originalSources);
                session.invalidateProject();
                String restoredCompile = nativeCompileProject(projectRootPath());
                lastCompileResult = restoredCompile;
                compileReady = isRunnableCompile(restoredCompile);
                compileAttempted = true;
                return new JSONObject()
                        .put("file", target.file)
                        .put("kind", target.kind)
                        .put("name", target.name)
                        .put("owner", target.owner)
                        .put("status", "rolled_back")
                        .put("diagnostics", diagnostics)
                        .put("restored_diagnostics", compileResultToJson(restoredCompile));
            }

            return new JSONObject()
                    .put("file", target.file)
                    .put("kind", target.kind)
                    .put("name", target.name)
                    .put("owner", target.owner)
                    .put("status", existed ? "written" : "created")
                    .put("diagnostics", diagnostics);
        } catch (Exception error) {
            restoreProjectSources(originalSources);
            session.invalidateProject();
            throw error;
        }
    }

    private JSONObject writeSymbolTransaction(AiAgentSession session, SymbolEntry target, String newSource) throws Exception {
        SourceFile sourceFile = target.sourceFile;
        String originalFileSource = sourceFile.source;
        String originalSymbolSource = target.source;
        int originalEnd = target.end;

        persistSelectedEdit(target, newSource);
        session.invalidateProject();

        String compileResult = nativeCompileProject(projectRootPath());
        lastCompileResult = compileResult;
        compileReady = isRunnableCompile(compileResult);
        compileAttempted = true;
        JSONObject diagnostics = compileResultToJson(compileResult);
        if (!compileReady) {
            sourceFile.source = originalFileSource;
            target.source = originalSymbolSource;
            target.end = originalEnd;
            writeTextFile(sourceFile.diskFile, originalFileSource);
            session.invalidateProject();
            String restoredCompile = nativeCompileProject(projectRootPath());
            lastCompileResult = restoredCompile;
            compileReady = isRunnableCompile(restoredCompile);
            compileAttempted = true;
            return new JSONObject()
                    .put("file", target.file)
                    .put("kind", target.kind)
                    .put("name", target.name)
                    .put("owner", target.owner)
                    .put("status", "rolled_back")
                    .put("diagnostics", diagnostics)
                    .put("restored_diagnostics", compileResultToJson(restoredCompile));
        }

        return new JSONObject()
                .put("file", target.file)
                .put("kind", target.kind)
                .put("name", target.name)
                .put("owner", target.owner)
                .put("status", "written")
                .put("diagnostics", diagnostics);
    }
    private JSONObject aiToolCompileProject() throws Exception {
        String compileResult = nativeCompileProject(projectRootPath());
        lastCompileResult = compileResult;
        compileReady = isRunnableCompile(compileResult);
        compileAttempted = true;
        return compileResultToJson(compileResult);
    }

    private JSONObject aiToolGetDiagnostics() throws Exception {
        return compileResultToJson(lastCompileResult);
    }

    private static JSONObject compileResultToJson(String compileResult) throws Exception {
        String result = compileResult == null || compileResult.isEmpty() ? "CompileNotRun" : compileResult;
        return new JSONObject()
                .put("ok", isRunnableCompile(result))
                .put("raw", result)
                .put("kind", result.startsWith("CompileError") ? "compile_error" : "compile_result");
    }
    private JSONObject aiToolSetInputState(JSONObject call) throws Exception {
        aiSimTouchX = call.optInt("x", aiSimTouchX);
        aiSimTouchY = call.optInt("y", aiSimTouchY);
        aiSimTouchActive = call.optInt("active", aiSimTouchActive);
        aiSimScreenWidth = call.optInt("screen_w", currentPreviewWidth());
        aiSimScreenHeight = call.optInt("screen_h", currentPreviewHeight());
        return currentInputStateJson();
    }

    private JSONObject aiToolSetRuntimeI32(JSONObject call) throws Exception {
        ensureAiTestCompileReady();
        String path = call.getString("path");
        int value = call.optInt("value", 0);
        String result = nativeSetRuntimeI32(projectRootPath(), path, value);
        return runtimeI32ResultToJson(result, path);
    }

    private JSONObject aiToolGetRuntimeI32(JSONObject call) throws Exception {
        ensureAiTestCompileReady();
        String path = call.getString("path");
        String result = nativeGetRuntimeI32(projectRootPath(), path);
        return runtimeI32ResultToJson(result, path);
    }

    private static JSONObject runtimeI32ResultToJson(String result, String path) throws Exception {
        String raw = result == null ? "StateError: empty result" : result;
        return new JSONObject()
                .put("ok", !raw.startsWith("StateError"))
                .put("path", path)
                .put("value", extractIntField(raw, "value", 0))
                .put("raw", raw);
    }
    private JSONObject aiToolRunFrame() throws Exception {
        ensureAiTestCompileReady();
        int[] frame = new int[RENDER_FRAME_I32_CAPACITY];
        int status = nativeRunFrameInto(
                projectRootPath(),
                aiSimTouchX,
                aiSimTouchY,
                aiSimTouchActive,
                currentAiScreenWidth(),
                currentAiScreenHeight(),
                frame);
        System.arraycopy(frame, 0, nativeFrameValues, 0, RENDER_FRAME_I32_CAPACITY);
        if (gamePreview != null) {
            gamePreview.setRenderFrameValues(nativeFrameValues);
        }
        return new JSONObject()
                .put("status", status)
                .put("input", currentInputStateJson())
                .put("frame", frameValuesToJson(frame))
                .put("runtime_state", runtimeStateJson());
    }

    private JSONObject aiToolInspectRuntimeState() throws Exception {
        return new JSONObject()
                .put("input", currentInputStateJson())
                .put("runtime_state", runtimeStateJson())
                .put("frame", frameValuesToJson(nativeFrameValues));
    }

    private void ensureAiTestCompileReady() throws Exception {
        if (compileReady) {
            return;
        }
        JSONObject diagnostics = aiToolCompileProject();
        if (!diagnostics.optBoolean("ok", false)) {
            throw new IOException("compile_project failed: " + diagnostics.optString("raw", "unknown error"));
        }
    }

    private int currentPreviewWidth() {
        return gamePreview == null || gamePreview.getWidth() <= 0 ? 360 : gamePreview.getWidth();
    }

    private int currentPreviewHeight() {
        return gamePreview == null || gamePreview.getHeight() <= 0 ? 640 : gamePreview.getHeight();
    }

    private int currentAiScreenWidth() {
        return aiSimScreenWidth > 0 ? aiSimScreenWidth : currentPreviewWidth();
    }

    private int currentAiScreenHeight() {
        return aiSimScreenHeight > 0 ? aiSimScreenHeight : currentPreviewHeight();
    }

    private JSONObject currentInputStateJson() throws Exception {
        return new JSONObject()
                .put("touch_x", aiSimTouchX)
                .put("touch_y", aiSimTouchY)
                .put("touch_active", aiSimTouchActive)
                .put("screen_w", currentAiScreenWidth())
                .put("screen_h", currentAiScreenHeight());
    }

    private JSONObject runtimeStateJson() throws Exception {
        File stateFile = new File(projectRoot(), "build/runtime_state.txt");
        JSONObject values = new JSONObject();
        String raw = "";
        if (stateFile.isFile()) {
            raw = readTextFile(stateFile);
            String[] lines = raw.split("\\r?\\n");
            for (String line : lines) {
                int equals = line.indexOf('=');
                if (equals > 0) {
                    values.put(line.substring(0, equals), line.substring(equals + 1));
                }
            }
        }
        return new JSONObject()
                .put("file", "build/runtime_state.txt")
                .put("exists", stateFile.isFile())
                .put("raw", raw)
                .put("values", values);
    }

    private static JSONObject frameValuesToJson(int[] frame) throws Exception {
        JSONArray values = new JSONArray();
        for (int index = 0; index < frame.length; index += 1) {
            values.put(frame[index]);
        }
        JSONArray commands = new JSONArray();
        int commandCount = frame.length > 5 ? Math.max(0, Math.min(MAX_RENDER_COMMANDS, frame[5])) : 0;
        for (int index = 0; index < commandCount; index += 1) {
            int base = RENDER_FRAME_HEADER_SIZE + index * RENDER_COMMAND_STRIDE;
            commands.put(new JSONObject()
                    .put("kind", frame[base])
                    .put("x", frame[base + 1])
                    .put("y", frame[base + 2])
                    .put("w", frame[base + 3])
                    .put("h", frame[base + 4])
                    .put("color", frame[base + 5])
                    .put("asset", frame[base + 6]));
        }
        return new JSONObject()
                .put("status", frame.length > 0 ? frame[0] : -1)
                .put("tick_count", frame.length > 1 ? frame[1] : 0)
                .put("game_tick_count", frame.length > 2 ? frame[2] : 0)
                .put("command_count", commandCount)
                .put("commands", commands)
                .put("raw_values", values);
    }
    private JSONObject aiToolTakeScreenshot() throws Exception {
        return logicalRenderSnapshot(nativeFrameValues);
    }

    private JSONObject logicalRenderSnapshot(int[] capturedFrame) throws Exception {
        int width = gamePreview == null ? 0 : gamePreview.getWidth();
        int height = gamePreview == null ? 0 : gamePreview.getHeight();
        JSONArray frame = new JSONArray();
        for (int index = 0; index < capturedFrame.length; index += 1) {
            frame.put(capturedFrame[index]);
        }
        return new JSONObject()
                .put("kind", "logical_render_snapshot")
                .put("width", width)
                .put("height", height)
                .put("touch_x", gamePreview == null ? 0 : gamePreview.touchX())
                .put("touch_y", gamePreview == null ? 0 : gamePreview.touchY())
                .put("touch_active", gamePreview != null && gamePreview.touchActive() == 1)
                .put("input", currentInputStateJson())
                .put("runtime_state", runtimeStateJson())
                .put("frame", frameValuesToJson(capturedFrame))
                .put("frame_values", frame);
    }

    private static SourceFile findProjectFile(ProjectSnapshot project, String file) throws Exception {
        for (SourceFile sourceFile : project.files) {
            if (sourceFile.path.equals(file)) {
                return sourceFile;
            }
        }
        throw new IOException("AI file target not found: " + file);
    }

    private static JSONObject symbolToJson(SymbolEntry symbol, boolean includeSource) throws Exception {
        JSONObject json = new JSONObject()
                .put("kind", symbol.kind)
                .put("name", symbol.name)
                .put("owner", symbol.owner)
                .put("file", symbol.file)
                .put("signature", symbol.signature);
        if ("global".equals(symbol.kind)) {
            json.put("backing_kind", "struct");
            json.put("backing_struct_name", symbol.name);
        }
        if (includeSource) {
            json.put("source", symbol.source);
            if ("global".equals(symbol.kind)) {
                json.put("backing_struct_source", symbol.backingStructSource);
            }
        }
        return json;
    }

    private static String aiLookupExpectedKind(String kind) {
        if ("replace_struct".equals(kind) || "struct".equals(kind)) {
            return "struct";
        }
        if ("global".equals(kind)) {
            return "global";
        }
        return "function";
    }

    private JSONArray buildAiOpenAiInput(String requestJson, boolean includeImages,
            boolean explicitCacheBreakpoints) throws Exception {
        JSONObject request = new JSONObject(requestJson);
        JSONObject stableRequest = request.optJSONObject("original_request");
        JSONObject volatileRequest = new JSONObject();
        if (stableRequest == null) {
            stableRequest = request;
            volatileRequest.put("phase", "initial");
        } else {
            Iterator<String> keys = request.keys();
            while (keys.hasNext()) {
                String key = keys.next();
                if (!"original_request".equals(key)) {
                    volatileRequest.put(key, request.get(key));
                }
            }
        }
        String stableInstruction = "Return only one JSON object. Follow the cached stasis_basics as the authoritative language/runtime orientation. You may inspect and edit any Stasis symbol in the workspace; selected_symbols are optional context only. When fast_path.enabled is true, its source_symbols are current bounded source: write the complete small tuning change and a behavior test in the first response without redundant reads. The initial project_symbol_index is a compact source-free inventory; use it to choose a direct read_symbol target and do not call list_symbols when the index already identifies the target. You may use mode=tool_calls with tool_calls to inspect or write the Stasis workspace using only these tools: list_symbols, list_owner_symbols, read_symbol, read_imports, write_imports, write_symbol, delete_symbol, list_tests, read_test_file, write_test_file, delete_test_file, run_tests, get_diagnostics, set_input_state, run_frame, inspect_runtime_state, take_screenshot. take_screenshot returns a compact logical render snapshot with decoded commands, runtime state, and input. set_input_state controls simulated test input; run_frame advances one frame and returns runtime/render state. Before writing, inspect only the minimum target symbols or tests needed for the request; use either the initial index, a compact list tool, or a direct read when possible, not every inspection tool. Never reread a target already present in selected_symbols or retained tool_observations. Small constant, size, color, position, or tuning changes should normally move from one focused inspection batch to a write. Do not use read_file; the workshop edits symbols, imports, and tests rather than whole source files. For behavior-changing requests, add or update a tests/*.test.stasis test before returning done. A valid test uses test `name`(): bool and returns true or false; do not create .ai_test.json files or use assert_runtime helpers, which are not Stasis syntax. run_tests executes the native bridge tests on the Android device. Apply code changes with write_symbol, delete_symbol, write_imports, write_test_file, or delete_test_file before final edits so failed writes and automatic compile/test_observation results return observations you can correct. The app compiles once after each tool-call batch that contains writes; read-only inspection batches do not rerun tests. Use write_test_file/run_tests or take_screenshot for validation instead of direct runtime pokes. Use on_code_swap() only for post-hot-swap migration, reinitialization, or compatibility work when a running game actually needs state adjusted after code changes; do not inspect it by default. Use tool_specs in the request for required_args, optional_args, and examples. Each tool call must use {\"tool\":\"name\",\"args\":{...}}; include only args relevant to that tool. Return mode=edits with replace_function/replace_struct edits only after write_symbol/delete_symbol/write_imports has successfully written, compiled, and the latest test_observation has passed runnable tests, including any new or updated behavior test for the request. If the requested work is already complete or no code changes are needed, return mode=done with a summary only. A replace_function edit for a missing function in an existing file is treated as an added helper. Do not use markdown.";
        stableInstruction += " Every response must include working_notes as a concise user-visible state summary of at most 2000 characters using Intent, Observed, Next, and Blocker. Report decisions and evidence, not private chain-of-thought. Update working_notes from the retained prior note and current observations on every call.";
        stableInstruction += " write_symbol creates or replaces a symbol. Before writing, inspect the current target. Follow game_design_rules, prefer_lifecycle_local_state, avoid_global_tick_for_per_entity_progression, and architecture_recommendations. Follow architecture_recommendations. Use command/event-style functions for durable gameplay concepts. Tool errors, validation_error observations, and test_observation failures are not final; correct them before returning mode=done. A failed write batch rolls back the whole batch and returns diagnostics.";
        JSONArray input = new JSONArray()
                .put(aiInputMessage("system", stableInstruction, false))
                .put(aiInputMessage("user", "Stable request context: " + stableRequest.toString(), explicitCacheBreakpoints));
        if (includeImages && !activeAiImageAttachments.isEmpty()) {
            input.put(aiImageInputMessage(activeAiImageAttachments));
        }
        input.put(aiInputMessage("user", "Volatile turn context: " + volatileRequest.toString(), false));
        return input;
    }

    private static JSONObject aiInputMessage(String role, String text, boolean cacheBreakpoint) throws Exception {
        JSONObject content = new JSONObject().put("type", "input_text").put("text", text);
        if (cacheBreakpoint) {
            content.put("prompt_cache_breakpoint", new JSONObject().put("mode", "explicit"));
        }
        return new JSONObject().put("role", role).put("content", new JSONArray().put(content));
    }

    private static JSONObject aiImageInputMessage(List<AiImageAttachment> attachments) throws Exception {
        JSONArray content = new JSONArray();
        StringBuilder paths = new StringBuilder("Explicitly selected app-private project images: ");
        boolean hasDesignSketch = false;
        for (AiImageAttachment attachment : attachments) {
            if (paths.charAt(paths.length() - 1) != ' ') paths.append(", ");
            paths.append(attachment.projectPath).append(" (").append(attachment.contextKind).append(")");
            hasDesignSketch = hasDesignSketch
                    || WorkshopAiImageContext.DESIGN_SKETCH.equals(attachment.contextKind);
        }
        if (hasDesignSketch) {
            paths.append(". Design sketches are rough layout guidance: follow their structure and intent, not their draft art quality.");
        }
        content.put(new JSONObject().put("type", "input_text").put("text", paths.toString()));
        for (AiImageAttachment attachment : attachments) {
            String dataUrl = "data:" + attachment.mimeType + ";base64,"
                    + Base64.encodeToString(attachment.bytes, Base64.NO_WRAP);
            content.put(new JSONObject()
                    .put("type", "input_image")
                    .put("image_url", dataUrl)
                    .put("detail", "original"));
        }
        return new JSONObject().put("role", "user").put("content", content);
    }

    private int maxOutputTokensForBudget(String model, String requestJson, double remainingUsd,
            boolean reserveImageGeneration) throws Exception {
        WorkshopAiPricing.Rates pricing = WorkshopAiPricing.forModel(model);
        if (pricing == null) throw new IOException("AI pricing is unavailable for " + model);
        byte[] inputBytes = buildAiOpenAiInput(requestJson, false, pricing.explicitCacheBreakpoints)
                .toString().getBytes(StandardCharsets.UTF_8);
        long imageTokens = 0L;
        for (AiImageAttachment attachment : activeAiImageAttachments) imageTokens += attachment.estimatedPatchTokens();
        long conservativeInputTokens = inputBytes.length + imageTokens;
        double conservativeInputCost = pricing.conservativeInputCostUsd(conservativeInputTokens);
        double outputBudget = remainingUsd - conservativeInputCost
                - (reserveImageGeneration ? GPT_IMAGE_2_LOW_1024_USD : 0.0);
        int outputTokens = (int)Math.floor(outputBudget * 1000000.0
                / pricing.effectiveOutputUsdPerMillion(conservativeInputTokens));
        if (outputTokens < 64) {
            throw new IOException("Device monthly AI limit leaves insufficient budget for another response");
        }
        return Math.min(MAX_AI_OUTPUT_TOKENS, outputTokens);
    }

    private AiApiResponse callOpenAiResponsesApi(String apiKey, String model, String requestJson,
            int maxOutputTokens, boolean allowImageGeneration) throws Exception {
        WorkshopAiPricing.Rates pricing = WorkshopAiPricing.forModel(model);
        if (pricing == null) throw new IOException("AI pricing is unavailable for " + model);
        JSONObject payload = new JSONObject();
        payload.put("model", model);
        payload.put("reasoning", new JSONObject().put("effort",
                isFastPathRequest(requestJson) ? "low" : pricing.reasoningEffort));
        payload.put("max_output_tokens", maxOutputTokens);
        payload.put("prompt_cache_key", AI_PROMPT_CACHE_KEY);
        if (pricing.explicitCacheBreakpoints) {
            payload.put("prompt_cache_options", new JSONObject().put("mode", "explicit").put("ttl", "30m"));
        }
        if (pricing.structuredOutputs) payload.put("text", buildAiResponseTextFormat());
        payload.put("input", buildAiOpenAiInput(requestJson, true, pricing.explicitCacheBreakpoints));
        if (allowImageGeneration) {
            payload.put("tools", new JSONArray().put(new JSONObject()
                    .put("type", "image_generation")
                    .put("action", "auto")
                    .put("quality", "low")
                    .put("size", "1024x1024")
                    .put("output_format", "png")));
            payload.put("tool_choice", "auto");
        }
        byte[] body = payload.toString().getBytes(StandardCharsets.UTF_8);

        HttpURLConnection connection = (HttpURLConnection)new URL("https://api.openai.com/v1/responses").openConnection();
        activeAiConnection = connection;
        try {
            connection.setConnectTimeout(AI_CONNECT_TIMEOUT_MS);
            connection.setReadTimeout(AI_READ_TIMEOUT_MS);
            connection.setRequestMethod("POST");
            connection.setRequestProperty("Authorization", "Bearer " + apiKey);
            connection.setRequestProperty("Content-Type", "application/json");
            connection.setDoOutput(true);
            OutputStream output = connection.getOutputStream();
            try {
                output.write(body);
            } finally {
                output.close();
            }

            int status = connection.getResponseCode();
            InputStream input = status >= 200 && status < 300 ? connection.getInputStream() : connection.getErrorStream();
            String response = input == null ? "" : readStreamStatic(input);
            if (status < 200 || status >= 300) {
                throw new IOException("OpenAI HTTP " + status + ": " + response);
            }
            return new AiApiResponse(response, extractAiUsage(response), model);
        } finally {
            connection.disconnect();
            if (activeAiConnection == connection) activeAiConnection = null;
        }
    }

    private static JSONObject buildAiResponseTextFormat() throws Exception {
        JSONObject editProperties = new JSONObject();
        editProperties.put("kind", new JSONObject().put("type", "string").put("enum", new JSONArray()
                .put("replace_function")
                .put("replace_struct")));
        editProperties.put("owner", new JSONObject().put("type", "string"));
        editProperties.put("name", new JSONObject().put("type", "string"));
        editProperties.put("file", new JSONObject().put("type", "string"));
        editProperties.put("new_source", new JSONObject().put("type", "string"));

        JSONObject editSchema = new JSONObject();
        editSchema.put("type", "object");
        editSchema.put("additionalProperties", false);
        editSchema.put("required", new JSONArray()
                .put("kind")
                .put("owner")
                .put("name")
                .put("file")
                .put("new_source"));
        editSchema.put("properties", editProperties);

        JSONObject toolArgsSchema = new JSONObject();
        toolArgsSchema.put("type", "object");
        toolArgsSchema.put("additionalProperties", true);
        toolArgsSchema.put("properties", new JSONObject());

        JSONObject toolProperties = new JSONObject();
        toolProperties.put("tool", new JSONObject().put("type", "string").put("enum", new JSONArray()
                .put("list_symbols")
                .put("list_owner_symbols")
                .put("read_symbol")
                .put("read_imports")
                .put("write_imports")
                .put("write_symbol")
                .put("delete_symbol")
                .put("get_diagnostics")
                .put("set_input_state")
                .put("run_frame")
                .put("inspect_runtime_state")
                .put("take_screenshot")
                .put("list_tests")
                .put("read_test_file")
                .put("write_test_file")
                .put("delete_test_file")
                .put("run_tests")));
        toolProperties.put("args", toolArgsSchema);

        JSONObject toolSchema = new JSONObject();
        toolSchema.put("type", "object");
        toolSchema.put("additionalProperties", false);
        toolSchema.put("required", new JSONArray()
                .put("tool")
                .put("args"));
        toolSchema.put("properties", toolProperties);
        JSONObject responseProperties = new JSONObject();
        responseProperties.put("mode", new JSONObject().put("type", "string").put("enum", new JSONArray()
                .put("tool_calls")
                .put("edits")
                .put("done")));
        responseProperties.put("working_notes", new JSONObject()
                .put("type", "string")
                .put("minLength", 1)
                .put("maxLength", WorkshopAiWorkingNotes.MAX_CHARS));
        responseProperties.put("summary", new JSONObject().put("type", "string"));
        responseProperties.put("tool_calls", new JSONObject().put("type", "array").put("items", toolSchema));
        responseProperties.put("edits", new JSONObject().put("type", "array").put("items", editSchema));

        JSONObject schema = new JSONObject();
        schema.put("type", "object");
        schema.put("additionalProperties", false);
        schema.put("required", new JSONArray()
                .put("mode")
                .put("working_notes"));
        schema.put("properties", responseProperties);

        JSONObject format = new JSONObject();
        format.put("type", "json_schema");
        format.put("name", "stasis_ai_code_response");
        format.put("strict", false);
        format.put("schema", schema);
        return new JSONObject().put("format", format);
    }

    private static String extractAiJsonResponse(String responseBody) throws Exception {
        JSONObject response = new JSONObject(responseBody);
        if (response.has("edits")) {
            return response.toString();
        }
        String text = response.optString("output_text", "");
        if (text.isEmpty()) {
            JSONArray output = response.optJSONArray("output");
            if (output != null) {
                StringBuilder builder = new StringBuilder();
                for (int index = 0; index < output.length(); index += 1) {
                    JSONObject item = output.optJSONObject(index);
                    if (item == null) {
                        continue;
                    }
                    JSONArray content = item.optJSONArray("content");
                    if (content == null) {
                        continue;
                    }
                    for (int contentIndex = 0; contentIndex < content.length(); contentIndex += 1) {
                        JSONObject part = content.optJSONObject(contentIndex);
                        if (part != null) {
                            builder.append(part.optString("text", ""));
                            builder.append(part.optString("output_text", ""));
                        }
                    }
                }
                text = builder.toString();
            }
        }
        int start = text.indexOf('{');
        int end = text.lastIndexOf('}');
        if (start < 0 || end < start) {
            if (!extractAiGeneratedImages(responseBody).isEmpty()) {
                return new JSONObject().put("mode", "done")
                        .put("summary", "Generated image ready for review").toString();
            }
            throw new IOException("AI response did not include JSON edits");
        }
        return text.substring(start, end + 1);
    }

    private static List<AiGeneratedImageCandidate> extractAiGeneratedImages(String responseBody) throws Exception {
        ArrayList<AiGeneratedImageCandidate> images = new ArrayList<>();
        JSONArray output = new JSONObject(responseBody).optJSONArray("output");
        if (output == null) return images;
        int totalBytes = 0;
        for (int index = 0; index < output.length(); index++) {
            JSONObject item = output.optJSONObject(index);
            if (item == null || !"image_generation_call".equals(item.optString("type", ""))) continue;
            if (images.size() >= 1) throw new IOException("AI returned more generated images than requested");
            String result = item.optString("result", "");
            if (result.isEmpty() || result.length() > MAX_AI_GENERATED_BASE64_CHARS) {
                throw new IOException("AI generated image result is empty or exceeds the review limit");
            }
            byte[] encoded;
            try {
                encoded = Base64.decode(result, Base64.DEFAULT);
            } catch (IllegalArgumentException error) {
                throw new IOException("AI generated image result is not valid Base64");
            }
            totalBytes += encoded.length;
            if (totalBytes > WorkshopImageAssets.MAX_IMPORT_BYTES) {
                throw new IOException("AI generated image exceeds the 8 MiB review limit");
            }
            android.graphics.BitmapFactory.Options bounds = new android.graphics.BitmapFactory.Options();
            bounds.inJustDecodeBounds = true;
            android.graphics.BitmapFactory.decodeByteArray(encoded, 0, encoded.length, bounds);
            long pixels = (long)bounds.outWidth * (long)bounds.outHeight;
            if (bounds.outWidth <= 0 || bounds.outHeight <= 0 || bounds.outWidth > 4096
                    || bounds.outHeight > 4096 || pixels > 16_000_000L
                    || !"image/png".equals(bounds.outMimeType)) {
                throw new IOException("AI generated image is not a bounded PNG");
            }
            images.add(new AiGeneratedImageCandidate(encoded, bounds.outWidth, bounds.outHeight));
        }
        return images;
    }


    private static JSONObject extractAiUsage(String responseBody) {
        try {
            JSONObject response = new JSONObject(responseBody);
            JSONObject usage = response.optJSONObject("usage");
            return usage == null ? new JSONObject() : usage;
        } catch (Exception error) {
            return new JSONObject();
        }
    }

    private static JSONObject summarizeAiRequestForTrace(String requestJson) throws Exception {
        JSONObject request = new JSONObject(requestJson);
        JSONObject stable = request.optJSONObject("original_request");
        JSONObject volatileContext = new JSONObject();
        boolean followup = stable != null;
        if (stable == null) {
            stable = request;
            volatileContext.put("phase", "initial");
        } else {
            Iterator<String> keys = request.keys();
            while (keys.hasNext()) {
                String key = keys.next();
                if (!"original_request".equals(key)) {
                    volatileContext.put(key, request.get(key));
                }
            }
        }
        JSONArray globals = stable.optJSONArray("project_globals");
        JSONObject symbolIndex = stable.optJSONObject("project_symbol_index");
        JSONObject fastPath = stable.optJSONObject("fast_path");
        JSONArray selected = stable.optJSONArray("selected_symbols");
        JSONArray tools = stable.optJSONArray("available_tools");
        return new JSONObject()
                .put("followup", followup)
                .put("cache_key", AI_PROMPT_CACHE_KEY)
                .put("cache_breakpoint_after", "stable_request_context")
                .put("stable_keys", sortedJsonKeys(stable))
                .put("volatile_keys", sortedJsonKeys(volatileContext))
                .put("project_global_count", globals == null ? 0 : globals.length())
                .put("project_symbol_index_count", symbolIndex == null ? 0 : symbolIndex.optInt("included_count", 0))
                .put("project_symbol_index_truncated", symbolIndex != null && symbolIndex.optBoolean("truncated", false))
                .put("fast_path_enabled", fastPath != null && fastPath.optBoolean("enabled", false))
                .put("fast_path_source_count", fastPath == null ? 0 : fastPath.optInt("included_count", 0))
                .put("selected_symbol_count", selected == null ? 0 : selected.length())
                .put("available_tool_count", tools == null ? 0 : tools.length());
    }

    private static JSONObject summarizeAiResponseForTrace(String responseBody, JSONObject parsedResponse) throws Exception {
        JSONObject response = new JSONObject(responseBody);
        return new JSONObject()
                .put("response_id", response.optString("id", ""))
                .put("response_model", response.optString("model", ""))
                .put("status", response.optString("status", ""))
                .put("usage", aiUsageSummary(extractAiUsage(responseBody)))
                .put("mode", parsedResponse.optString("mode", ""))
                .put("summary", parsedResponse.optString("summary", ""))
                .put("tool_call_count", parsedResponse.optJSONArray("tool_calls") == null ? 0 : parsedResponse.optJSONArray("tool_calls").length())
                .put("edit_count", parsedResponse.optJSONArray("edits") == null ? 0 : parsedResponse.optJSONArray("edits").length())
                .put("response_keys", sortedJsonKeys(parsedResponse));
    }

    private static JSONArray sortedJsonKeys(JSONObject object) {
        TreeSet<String> keys = new TreeSet<>();
        if (object != null) {
            Iterator<String> iterator = object.keys();
            while (iterator.hasNext()) {
                keys.add(iterator.next());
            }
        }
        JSONArray result = new JSONArray();
        for (String key : keys) {
            result.put(key);
        }
        return result;
    }

    private static JSONObject aiUsageSummary(JSONObject usage) throws Exception {
        long inputTokens = usageTokenCount(usage, "input_tokens", "prompt_tokens");
        long cachedInputTokens = cachedInputTokenCount(usage);
        long cacheWriteTokens = cacheWriteInputTokenCount(usage);
        long outputTokens = usageTokenCount(usage, "output_tokens", "completion_tokens");
        return new JSONObject()
                .put("input_tokens", inputTokens)
                .put("cached_input_tokens", cachedInputTokens)
                .put("cache_write_input_tokens", cacheWriteTokens)
                .put("uncached_input_tokens", Math.max(0L, inputTokens - cachedInputTokens - cacheWriteTokens))
                .put("output_tokens", outputTokens);
    }

    private void saveLastAiUsage(JSONObject usageJson) {
        getSharedPreferences(AI_PREFS, MODE_PRIVATE)
                .edit()
                .putString(AI_PREF_LAST_USAGE, usageJson.toString())
                .apply();
    }

    private static long usageTokenCount(JSONObject usage, String primaryName, String fallbackName) {
        if (usage == null) {
            return 0L;
        }
        if (usage.has(primaryName)) {
            return usage.optLong(primaryName, 0L);
        }
        return usage.optLong(fallbackName, 0L);
    }

    private static long cachedInputTokenCount(JSONObject usage) {
        if (usage == null) {
            return 0L;
        }
        JSONObject details = usage.optJSONObject("input_tokens_details");
        if (details == null) {
            details = usage.optJSONObject("prompt_tokens_details");
        }
        return details == null ? 0L : details.optLong("cached_tokens", 0L);
    }

    private static long cacheWriteInputTokenCount(JSONObject usage) {
        if (usage == null) {
            return 0L;
        }
        JSONObject details = usage.optJSONObject("input_tokens_details");
        if (details == null) {
            details = usage.optJSONObject("prompt_tokens_details");
        }
        return details == null ? 0L : details.optLong("cache_write_tokens", 0L);
    }

    private static boolean hasKnownAiPricing(String model) {
        return WorkshopAiPricing.isKnown(model);
    }

    private static double estimateAiCostUsd(String model, long inputTokens, long cachedInputTokens, long cacheWriteInputTokens, long outputTokens) {
        WorkshopAiPricing.Rates pricing = WorkshopAiPricing.forModel(model);
        return pricing == null ? 0.0
                : pricing.estimate(inputTokens, cachedInputTokens, cacheWriteInputTokens, outputTokens);
    }

    private static String formatAiCostUsd(double costUsd) {
        return WorkshopMoney.formatUsd(costUsd);
    }

    private String selectedAiModelForPricing() {
        String model = aiModelEditor == null ? "" : aiModelEditor.getText().toString().trim();
        return model.isEmpty() ? DEFAULT_AI_MODEL : model;
    }

    private double selectedAiInputCostUsd(long tokens) {
        WorkshopAiPricing.Rates pricing = WorkshopAiPricing.forModel(selectedAiModelForPricing());
        return pricing == null ? 0.0 : pricing.estimate(tokens, 0L, 0L, 0L);
    }

    private void applyAiCodeResponse(AiAgentResult aiResult, SymbolEntry fallbackSymbol) {
        Map<String, String> originalSources = null;
        try {
            saveLastAiUsage(aiResult.usageJson);
            if (!aiResult.generatedImages.isEmpty()) reviewAiGeneratedImage(aiResult.generatedImages.get(0));
            JSONObject response = new JSONObject(aiResult.aiJson);
            String mode = response.optString("mode", "edits");
            JSONArray edits = response.optJSONArray("edits");
            if (edits == null) {
                edits = new JSONArray();
            }
            if ("done".equals(mode) || edits.length() == 0) {
                ProjectSnapshot currentProject = loadBundledProject();
                rebuildSymbolList(currentProject);
                refreshChangeSummary(currentProject);
                String compileResult = nativeCompileProject(projectRootPath());
                lastCompileResult = compileResult;
                compileReady = isRunnableCompile(compileResult);
                compileAttempted = true;
                String elapsed = currentAiElapsedText();
                updateAiProgress(aiResult.finalStep, aiResult.finalActionCount, aiReloadPhase(compileResult));
                JSONObject testRun = aiToolRunTests(new AiAgentSession());
                appendAiTrace("apply_done", new JSONObject().put("summary", response.optString("summary", "no actions")).put("compile", compileResult).put("tests", testRun).put("elapsed", elapsed));
                recordAiOutcome(activeAiPrompt, "complete", response.optString("summary", "no actions"), aiResult.usageSummary);
                setStatusText("AI edit complete: " + response.optString("summary", "no actions") + " - no actions - " + aiReloadSummary(compileResult) + " - " + testSummaryText(testRun) + " - elapsed=" + elapsed + " - " + compileResult + " - " + aiResult.usageSummary + " - trace=" + aiTraceLogPath());
                return;
            }
            ProjectSnapshot project = loadBundledProject();
            originalSources = snapshotProjectSources(project);
            SymbolEntry lastEdited = fallbackSymbol;
            for (int index = 0; index < edits.length(); index += 1) {
                JSONObject edit = edits.getJSONObject(index);
                String kind = edit.getString("kind");
                if (!"replace_function".equals(kind) && !"replace_struct".equals(kind)) {
                    throw new IOException("Unsupported AI edit kind: " + kind);
                }
                String expectedKind = "replace_struct".equals(kind) ? "struct" : "function";
                String newSource = edit.getString("new_source").trim();
                SymbolEntry target = resolveAiEditTarget(project, kind, expectedKind, edit, fallbackSymbol, newSource);
                validateAiReplacementSource(kind, target.name, newSource);
                persistSelectedEdit(target, newSource);
                lastEdited = target;
                project = loadBundledProject();
            }

            String compileResult = nativeCompileProject(projectRootPath());
            lastCompileResult = compileResult;
            compileReady = isRunnableCompile(compileResult);
            compileAttempted = true;
            if (!compileReady) {
                throw new IOException("AI edit compile failed: " + compileResult);
            }

            rebuildSymbolList(project);
            if (lastEdited != null) {
                SymbolEntry refreshed = findMatchingSymbol(project, lastEdited);
                if (refreshed != null) {
                    showSymbol(refreshed);
                }
            }
            refreshChangeSummary(project);
            String elapsed = currentAiElapsedText();
            updateAiProgress(
                    aiResult.finalStep,
                    aiResult.finalActionCount,
                    WorkshopAiCompletionStatus.afterEdits(aiReloadPhase(compileResult)));
            JSONObject testRun = aiToolRunTests(new AiAgentSession());
            appendAiTrace("apply_edits", new JSONObject().put("summary", response.optString("summary", "updated workspace")).put("compile", compileResult).put("tests", testRun).put("elapsed", elapsed));
            recordAiOutcome(activeAiPrompt, "applied", response.optString("summary", "updated workspace"), aiResult.usageSummary);
            setStatusText("AI edit applied: " + response.optString("summary", "updated workspace") + " - " + aiReloadSummary(compileResult) + " - " + testSummaryText(testRun) + " - elapsed=" + elapsed + " - " + compileResult + " - " + aiResult.usageSummary + " - trace=" + aiTraceLogPath());
        } catch (Exception error) {
            if (originalSources != null) {
                try {
                    restoreProjectSources(originalSources);
                    ProjectSnapshot restoredProject = loadBundledProject();
                    rebuildSymbolList(restoredProject);
                    refreshChangeSummary(restoredProject);
                    String restoredCompile = nativeCompileProject(projectRootPath());
                    lastCompileResult = restoredCompile;
                    compileReady = isRunnableCompile(restoredCompile);
                    compileAttempted = true;
                } catch (Exception restoreError) {
                    String elapsed = currentAiElapsedText();
                    updateAiProgress(aiResult.finalStep, aiResult.finalActionCount, "rollback failed");
                    appendAiTraceFields("rollback_failed", "error", error.getMessage(), "restore_error", restoreError.getMessage(), "elapsed", elapsed);
                    recordAiOutcome(activeAiPrompt, "rollback_failed", error.getMessage() + " / " + restoreError.getMessage(), aiResult.usageSummary);
                    setStatusText("AI edit apply failed and rollback failed: elapsed=" + elapsed + " - " + error.getMessage() + " / " + restoreError.getMessage() + " - trace=" + aiTraceLogPath());
                    return;
                }
            }
            String elapsed = currentAiElapsedText();
            updateAiProgress(aiResult.finalStep, aiResult.finalActionCount, "rolled back");
            appendAiTraceFields("apply_failed_rolled_back", "error", error.getMessage(), "elapsed", elapsed, null, null);
            recordAiOutcome(activeAiPrompt, "rolled_back", error.getMessage(), aiResult.usageSummary);
            setStatusText("AI edit apply failed and rolled back: elapsed=" + elapsed + " - " + error.getMessage() + " - trace=" + aiTraceLogPath());
        }
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
    private SymbolEntry resolveAiEditTarget(ProjectSnapshot project, String editKind, String expectedKind, JSONObject edit, SymbolEntry fallback, String newSource) throws Exception {
        SymbolEntry target = findSymbolForAiEditOrNull(project, expectedKind, edit, fallback);
        if (target != null) {
            return target;
        }
        if (!"replace_function".equals(editKind)) {
            throw new IOException("AI edit target not found: " + edit.optString("file", "") + " " + edit.optString("name", ""));
        }

        String file = edit.optString("file", fallback == null ? "" : fallback.file);
        String name = edit.optString("name", extractDeclarationName(newSource, "function"));
        if (file.isEmpty() || name.isEmpty()) {
            throw new IOException("AI add function target requires file and name");
        }
        validateAiReplacementSource(editKind, name, newSource);
        appendAiFunction(project, file, newSource);
        ProjectSnapshot refreshedProject = loadBundledProject();
        JSONObject lookup = new JSONObject()
                .put("kind", "replace_function")
                .put("file", file)
                .put("name", name);
        return findSymbolForAiEdit(refreshedProject, expectedKind, lookup, null);
    }

    private void appendAiFunction(ProjectSnapshot project, String file, String newSource) throws Exception {
        SourceFile sourceFile = findProjectFile(project, file);
        String separator = sourceFile.source.endsWith("\n") ? "\n" : "\n\n";
        sourceFile.source = sourceFile.source + separator + newSource.trim() + "\n";
        writeTextFile(sourceFile.diskFile, sourceFile.source);
    }

    private static Map<String, String> snapshotProjectSources(ProjectSnapshot project) {
        Map<String, String> sources = new LinkedHashMap<>();
        for (SourceFile sourceFile : project.files) {
            sources.put(sourceFile.path, sourceFile.source);
        }
        return sources;
    }

    private void restoreProjectSources(Map<String, String> sources) throws IOException {
        File root = projectRoot();
        for (Map.Entry<String, String> entry : sources.entrySet()) {
            writeTextFile(new File(root, entry.getKey()), entry.getValue());
        }
    }

    private static SymbolEntry findSymbolForAiEdit(ProjectSnapshot project, String expectedKind, JSONObject edit, SymbolEntry fallback) throws Exception {
        SymbolEntry symbol = findSymbolForAiEditOrNull(project, expectedKind, edit, fallback);
        if (symbol != null) {
            return symbol;
        }
        String file = edit.optString("file", fallback == null ? "" : fallback.file);
        String name = edit.optString("name", fallback == null ? "" : fallback.name);
        if (file.isEmpty() || name.isEmpty()) {
            throw new IOException("AI edit target requires file and name when no symbol is selected");
        }
        throw new IOException("AI edit target not found: " + file + " " + name);
    }

    private static SymbolEntry findSymbolForAiEditOrNull(ProjectSnapshot project, String expectedKind, JSONObject edit, SymbolEntry fallback) {
        String file = edit.optString("file", fallback == null ? "" : fallback.file);
        String name = edit.optString("name", fallback == null ? "" : fallback.name);
        String owner = edit.optString("owner", fallback == null ? "" : fallback.owner);
        if (file.isEmpty() || name.isEmpty()) {
            return null;
        }

        SymbolEntry fileNameMatch = null;
        int fileNameMatches = 0;
        for (SymbolSection section : project.sections) {
            for (SymbolGroup group : section.groups) {
                for (SymbolEntry symbol : group.symbols) {
                    if (symbol.kind.equals(expectedKind)
                            && symbol.file.equals(file)
                            && symbol.name.equals(name)) {
                        if (owner.isEmpty() || symbol.owner.equals(owner)) {
                            return symbol;
                        }
                        fileNameMatch = symbol;
                        fileNameMatches += 1;
                    }
                }
            }
        }
        if (fileNameMatches == 1) {
            return fileNameMatch;
        }

        SymbolEntry globalNameMatch = null;
        int globalNameMatches = 0;
        for (SymbolSection section : project.sections) {
            for (SymbolGroup group : section.groups) {
                for (SymbolEntry symbol : group.symbols) {
                    if (symbol.kind.equals(expectedKind) && symbol.name.equals(name)) {
                        if (owner.isEmpty() || symbol.owner.equals(owner)) {
                            globalNameMatch = symbol;
                            globalNameMatches += 1;
                        }
                    }
                }
            }
        }
        return globalNameMatches == 1 ? globalNameMatch : null;
    }

    private static SymbolEntry findAnySymbolForAiLookup(ProjectSnapshot project, JSONObject edit, SymbolEntry fallback) throws Exception {
        String file = edit.optString("file", fallback == null ? "" : fallback.file);
        String name = edit.optString("name", fallback == null ? "" : fallback.name);
        String owner = edit.optString("owner", fallback == null ? "" : fallback.owner);
        if (name.isEmpty()) {
            throw new IOException("AI read_symbol requires a name when kind is omitted");
        }

        SymbolEntry exactMatch = null;
        int exactMatches = 0;
        SymbolEntry nameMatch = null;
        int nameMatches = 0;
        for (SymbolSection section : project.sections) {
            for (SymbolGroup group : section.groups) {
                for (SymbolEntry symbol : group.symbols) {
                    if (!symbol.name.equals(name)) {
                        continue;
                    }
                    if (!owner.isEmpty() && !symbol.owner.equals(owner)) {
                        continue;
                    }
                    if (!file.isEmpty() && symbol.file.equals(file)) {
                        exactMatch = symbol;
                        exactMatches += 1;
                    }
                    nameMatch = symbol;
                    nameMatches += 1;
                }
            }
        }
        if (exactMatches == 1) {
            return exactMatch;
        }
        if (exactMatches > 1) {
            throw new IOException("AI read_symbol target ambiguous: " + file + " " + name);
        }
        if (nameMatches == 1) {
            return nameMatch;
        }
        if (nameMatches > 1) {
            throw new IOException("AI read_symbol target ambiguous: " + file + " " + name);
        }
        throw new IOException("AI read_symbol target not found: " + file + " " + name);
    }
    private static void validateAiReplacementSource(String editKind, String expectedName, String newSource) throws Exception {
        if (newSource.contains("&mut") || newSource.contains("->") || newSource.contains("fn ")) {
            throw new IOException("AI edit must use Stasis syntax, not Rust syntax");
        }
        if ("replace_struct".equals(editKind)) {
            validateSingleReplacementDeclaration(newSource, "struct", expectedName);
            return;
        }
        validateSingleReplacementDeclaration(newSource, "function", expectedName);
    }

    private static void validateSingleReplacementDeclaration(String source, String keyword, String expectedName) throws Exception {
        String trimmed = source.trim();
        if (!trimmed.startsWith(keyword + " ") || !expectedName.equals(extractDeclarationName(trimmed, keyword))) {
            throw new IOException("AI replace_" + keyword + " source does not define expected " + keyword + ": " + expectedName);
        }
        int bodyStart = trimmed.indexOf('{');
        int bodyEnd = bodyStart < 0 ? -1 : findMatchingBrace(trimmed, bodyStart);
        if (bodyStart < 0 || bodyEnd != trimmed.length()) {
            throw new IOException("AI replace_" + keyword + " source must contain exactly one top-level " + keyword + " declaration");
        }
        String body = trimmed.substring(bodyStart + 1, bodyEnd - 1);
        if (body.contains("function ") || body.contains("struct ") || body.contains("global ")) {
            throw new IOException("AI replace_" + keyword + " body must not contain nested function, struct, or global declarations");
        }
    }
    private static String extractDeclarationName(String source, String keyword) {
        String trimmed = source.trim();
        String prefix = keyword + " ";
        if (!trimmed.startsWith(prefix)) {
            return "";
        }
        int cursor = prefix.length();
        while (cursor < trimmed.length() && Character.isWhitespace(trimmed.charAt(cursor))) {
            cursor += 1;
        }
        int start = cursor;
        while (cursor < trimmed.length()) {
            char value = trimmed.charAt(cursor);
            if (!Character.isLetterOrDigit(value) && value != '_') {
                break;
            }
            cursor += 1;
        }
        return trimmed.substring(start, cursor);
    }

    private void applySelectedEdit() {
        if (selectedSymbol == null) {
            return;
        }

        SymbolEntry editedSymbol = selectedSymbol;
        String editedSource = sourceEditor.getText().toString().trim();
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
                diagnosticStatus.setText("Compile passed - " + reload);
                setStatusText("Saved to .stasis file - " + reload + " - " + compileResult);
            } else {
                diagnosticFile = editedSymbol.file;
                diagnosticSymbol = editedSymbol.name;
                diagnosticLine = 0;
                selectedRecoveryEntry = AndroidEditRecoveryStore.record(this, activeRecoveryProjectId(), editedSymbol.file,
                        editedSymbol.name, beforeFileSource, editedSymbol.sourceFile.source, compileResult);
                diagnosticStatus.setText("Compile failed\nfile=" + diagnosticFile
                        + "\nsymbol=" + diagnosticSymbol + "\nreload=" + reload + "\n" + compileResult);
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
            diagnosticFile = entry.path;
            diagnosticSymbol = entry.symbol;
            diagnosticLine = 0;
            diagnosticStatus.setText("Recoverable failed apply history: " + entries.length + " entries\nselected="
                    + (selectedIndex + 1) + "/" + entries.length + "\nfile=" + entry.path
                    + "\nsymbol=" + entry.symbol + "\n" + entry.diagnostic);
        } catch (Exception error) {
            diagnosticStatus.setText("Recovery history unavailable: " + error.getMessage());
        }
    }

    private List<WorkshopImageAssets.AssetInfo> selectedAiImageInfos() throws IOException {
        if (activeProject == null || selectedImageAssets.isEmpty()) return Collections.emptyList();
        ArrayList<WorkshopImageAssets.AssetInfo> selected = new ArrayList<>();
        int totalBytes = 0;
        for (WorkshopImageAssets.AssetInfo asset : WorkshopImageAssets.list(activeProject.root)) {
            if (!selectedImageAssets.contains(asset.relativePath)) continue;
            if (selected.size() >= MAX_AI_IMAGE_ATTACHMENTS) {
                throw new IOException("select no more than " + MAX_AI_IMAGE_ATTACHMENTS + " images");
            }
            if (asset.bytes > MAX_AI_IMAGE_ATTACHMENT_BYTES - totalBytes) {
                throw new IOException("selected images exceed the 12 MiB request limit");
            }
            selected.add(asset);
            totalBytes += (int)asset.bytes;
        }
        if (selected.size() != selectedImageAssets.size()) {
            throw new IOException("one or more selected images no longer exist in this project");
        }
        return selected;
    }

    private static long estimatedImagePatchTokens(List<WorkshopImageAssets.AssetInfo> images) {
        long patches = 0L;
        for (WorkshopImageAssets.AssetInfo image : images) {
            patches += ((image.width + 31L) / 32L) * ((image.height + 31L) / 32L);
        }
        return patches;
    }

    private JSONArray aiImageMetadata(List<WorkshopImageAssets.AssetInfo> images) throws Exception {
        JSONArray metadata = new JSONArray();
        for (WorkshopImageAssets.AssetInfo image : images) {
            byte[] bytes = WorkshopImageAssets.readForSync(image);
            boolean designSketch = selectedDesignSketchAssets.contains(image.relativePath);
            metadata.put(new JSONObject()
                    .put("kind", WorkshopAiImageContext.kind(designSketch))
                    .put("purpose", designSketch
                            ? "rough visual layout guidance; interpret structure and intent, not final art quality"
                            : "project art reference")
                    .put("project_path", image.relativePath)
                    .put("width", image.width)
                    .put("height", image.height)
                    .put("bytes", image.bytes)
                    .put("sha256", sha256Bytes(bytes))
                    .put("detail", "original")
                    .put("estimated_patch_tokens", ((image.width + 31L) / 32L) * ((image.height + 31L) / 32L)));
        }
        return metadata;
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

    private static List<AiImageAttachment> loadAiImageAttachments(
            List<WorkshopImageAssets.AssetInfo> images, JSONArray metadata, Bitmap previewPixels)
            throws IOException {
        ArrayList<AiImageAttachment> attachments = new ArrayList<>();
        int totalBytes = 0;
        for (WorkshopImageAssets.AssetInfo image : images) {
            byte[] bytes = WorkshopImageAssets.readForSync(image);
            totalBytes += bytes.length;
            attachments.add(new AiImageAttachment(image.relativePath, imageMimeType(image.file.getName()),
                    bytes, image.width, image.height, attachmentKind(metadata, image.relativePath)));
        }
        if (previewPixels != null) {
            ByteArrayOutputStream encoded = new ByteArrayOutputStream();
            if (!previewPixels.compress(Bitmap.CompressFormat.PNG, 100, encoded)) {
                throw new IOException("could not encode captured preview pixels");
            }
            byte[] bytes = encoded.toByteArray();
            if (bytes.length > MAX_AI_IMAGE_ATTACHMENT_BYTES - totalBytes) {
                throw new IOException("project images plus preview exceed the 12 MiB request limit");
            }
            attachments.add(new AiImageAttachment("captured-preview.png", "image/png", bytes,
                    previewPixels.getWidth(), previewPixels.getHeight(), "captured_preview"));
        }
        return Collections.unmodifiableList(attachments);
    }

    private static String attachmentKind(JSONArray metadata, String projectPath) {
        if (metadata == null) return WorkshopAiImageContext.PROJECT_ASSET;
        for (int index = 0; index < metadata.length(); index += 1) {
            JSONObject item = metadata.optJSONObject(index);
            if (item != null && projectPath.equals(item.optString("project_path", ""))) {
                return item.optString("kind", WorkshopAiImageContext.PROJECT_ASSET);
            }
        }
        return WorkshopAiImageContext.PROJECT_ASSET;
    }

    private static String imageMimeType(String name) throws IOException {
        String lower = name.toLowerCase(Locale.US);
        if (lower.endsWith(".png")) return "image/png";
        if (lower.endsWith(".jpg") || lower.endsWith(".jpeg")) return "image/jpeg";
        if (lower.endsWith(".webp")) return "image/webp";
        throw new IOException("selected image has an unsupported format");
    }

    private void requestImageImport() {
        if (activeProject == null) {
            setStatusText("Image import needs a registered active project");
            return;
        }
        if (aiRunActive || githubOperationActive || projectIoActive || hasPendingSourceEdit()) {
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
        stopVoiceRecognition();
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
        if (aiRunActive || githubOperationActive || projectIoActive || hasPendingSourceEdit()) {
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

    private void reviewAiGeneratedImage(final AiGeneratedImageCandidate candidate) {
        final File reviewProjectRoot = activeProject == null ? null : activeProject.root;
        final Bitmap generated = android.graphics.BitmapFactory.decodeByteArray(
                candidate.pngBytes, 0, candidate.pngBytes.length);
        if (generated == null) {
            setStatusText("AI generated image could not be decoded for review");
            return;
        }
        final ArrayList<Bitmap> reviewBitmaps = new ArrayList<>();
        reviewBitmaps.add(generated);
        LinearLayout content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        content.setPadding(dp(8), dp(8), dp(8), dp(8));
        LinearLayout comparison = new LinearLayout(this);
        comparison.setOrientation(LinearLayout.HORIZONTAL);
        ImageView before = new ImageView(this);
        before.setScaleType(ImageView.ScaleType.FIT_CENTER);
        try {
            List<WorkshopImageAssets.AssetInfo> selected = selectedAiImageInfos();
            if (!selected.isEmpty()) {
                Bitmap prior = WorkshopImageAssets.decodePreview(selected.get(0));
                reviewBitmaps.add(prior);
                before.setImageBitmap(prior);
                before.setContentDescription("Selected project image before AI generation or edit");
            } else {
                before.setBackgroundColor(Color.rgb(225, 228, 234));
                before.setContentDescription("No reference image selected");
            }
        } catch (Exception ignored) {
            before.setBackgroundColor(Color.rgb(225, 228, 234));
        }
        ImageView after = new ImageView(this);
        after.setImageBitmap(generated);
        after.setScaleType(ImageView.ScaleType.FIT_CENTER);
        after.setContentDescription("Temporary AI-generated image after result");
        comparison.addView(before, new LinearLayout.LayoutParams(0, dp(300), 1.0f));
        comparison.addView(after, new LinearLayout.LayoutParams(0, dp(300), 1.0f));
        content.addView(comparison, fullWidth());
        TextView labels = new TextView(this);
        labels.setText("Before / selected reference                         AI result");
        labels.setTextSize(11.0f);
        content.addView(labels, fullWidth());
        final EditText name = new EditText(this);
        name.setHint("Accepted asset name");
        name.setSingleLine(true);
        name.setText("ai_generated_image");
        content.addView(name, fullWidth());
        AlertDialog dialog = new AlertDialog.Builder(this)
                .setTitle("Review AI Image - " + candidate.width + "x" + candidate.height)
                .setMessage("The result is temporary. Accept creates a new asset and never overwrites the reference.")
                .setView(content)
                .setPositiveButton("Accept as New Asset", new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        try {
                            if (reviewProjectRoot == null || activeProject == null
                                    || !reviewProjectRoot.equals(activeProject.root)) {
                                throw new IOException("active project changed before image acceptance");
                            }
                            WorkshopImageAssets.AssetInfo saved = WorkshopImageAssets.saveGeneratedPng(
                                    candidate.pngBytes, reviewProjectRoot, name.getText().toString());
                            refreshImageAssetList();
                            appendAiTrace("generated_image_review", new JSONObject()
                                    .put("action", "accepted").put("path", saved.relativePath)
                                    .put("width", saved.width).put("height", saved.height));
                            setStatusText("AI image accepted as new asset: " + saved.relativePath);
                        } catch (Exception error) {
                            setStatusText("AI image accept failed without project mutation: " + error.getMessage());
                        }
                    }
                })
                .setNegativeButton("Reject", new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        appendAiTraceFields("generated_image_review", "action", "rejected", "dimensions",
                                candidate.width + "x" + candidate.height, null, null);
                        setStatusText("AI image rejected; project assets unchanged");
                    }
                })
                .create();
        dialog.setOnDismissListener(new android.content.DialogInterface.OnDismissListener() {
            @Override public void onDismiss(android.content.DialogInterface ignored) {
                for (Bitmap bitmap : reviewBitmaps) if (!bitmap.isRecycled()) bitmap.recycle();
            }
        });
        dialog.show();
    }

    private void refreshImageAssetList() {
        if (imageAssetList == null) return;
        imageAssetList.removeAllViews();
        if (activeProject == null) return;
        try {
            List<WorkshopImageAssets.AssetInfo> assets = WorkshopImageAssets.list(activeProject.root);
            String activeId = activeProject.id;
            if (!activeId.equals(selectedImageAssetProjectId)) {
                selectedImageAssets.clear();
                selectedDesignSketchAssets.clear();
                selectedDesignSketchAssets.addAll(getSharedPreferences(AI_PREFS, MODE_PRIVATE)
                        .getStringSet(AI_PREF_DESIGN_SKETCHES_PREFIX + activeId,
                                Collections.<String>emptySet()));
                selectedImageAssetProjectId = activeId;
            }
            HashSet<String> available = new HashSet<>();
            for (WorkshopImageAssets.AssetInfo asset : assets) available.add(asset.relativePath);
            selectedImageAssets.retainAll(available);
            if (selectedDesignSketchAssets.retainAll(available)) persistDesignSketchAssets();
            refreshAiAttachmentStatus();
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
                preview.setText((selectedImageAssets.contains(asset.relativePath) ? "[Selected] " : "")
                        + asset.relativePath + "\n" + asset.width + "x" + asset.height
                        + " - " + asset.bytes + " bytes");
                preview.setContentDescription((selectedImageAssets.contains(asset.relativePath)
                        ? "Selected image asset " : "Image asset ") + asset.relativePath + ", "
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
        final boolean selected = selectedImageAssets.contains(asset.relativePath);
        String[] actions = new String[] {"Preview", selected ? "Unselect" : "Select",
                "Paint as Copy", "Rename", "Delete"};
        new AlertDialog.Builder(this)
                .setTitle(asset.relativePath)
                .setItems(actions, new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        if (which == 0) showImagePreview(asset);
                        else if (which == 1) toggleImageSelection(asset);
                        else if (which == 2) openPaintEditor(asset);
                        else if (which == 3) requestImageRename(asset);
                        else if (which == 4) requestImageDelete(asset);
                    }
                })
                .setNegativeButton("Cancel", null)
                .show();
    }

    private void requestNewPaintedImage() {
        if (!canModifyImageAssets()) return;
        final EditText width = new EditText(this);
        width.setHint("Width");
        width.setInputType(InputType.TYPE_CLASS_NUMBER);
        width.setText("256");
        final EditText height = new EditText(this);
        height.setHint("Height");
        height.setInputType(InputType.TYPE_CLASS_NUMBER);
        height.setText("256");
        LinearLayout dimensions = new LinearLayout(this);
        dimensions.setOrientation(LinearLayout.HORIZONTAL);
        dimensions.addView(width, weightedWidth());
        dimensions.addView(height, weightedWidth());
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
        showPaintEditor(width, height, initial, defaultName, false);
    }

    private void showPaintEditor(int width, int height, Bitmap initial, String defaultName,
                                 boolean suggestAiAttachment) {
        final WorkshopPaintView paint = new WorkshopPaintView(this, width, height, initial);
        LinearLayout content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        content.setPadding(dp(8), dp(8), dp(8), dp(8));
        content.addView(paint, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT, dp(360)));

        LinearLayout tools = new LinearLayout(this);
        tools.setOrientation(LinearLayout.HORIZONTAL);
        Button brush = compactButton("Brush");
        Button eraser = compactButton("Eraser");
        Button undo = compactButton("Undo");
        Button redo = compactButton("Redo");
        tools.addView(brush, weightedWidth());
        tools.addView(eraser, weightedWidth());
        tools.addView(undo, weightedWidth());
        tools.addView(redo, weightedWidth());
        content.addView(tools, fullWidth());
        brush.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { paint.setEraser(false); setStatusText("Paint tool: brush"); }
        });
        eraser.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { paint.setEraser(true); setStatusText("Paint tool: eraser"); }
        });
        undo.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { paint.undo(); }
        });
        redo.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { paint.redo(); }
        });

        LinearLayout sizes = new LinearLayout(this);
        sizes.setOrientation(LinearLayout.HORIZONTAL);
        for (final int size : new int[] {2, 8, 24, 64}) {
            Button choice = compactButton(Integer.toString(size) + "px");
            choice.setOnClickListener(new View.OnClickListener() {
                @Override public void onClick(View view) { paint.setBrushSize(size); }
            });
            sizes.addView(choice, weightedWidth());
        }
        content.addView(sizes, fullWidth());

        LinearLayout palette = new LinearLayout(this);
        palette.setOrientation(LinearLayout.HORIZONTAL);
        final int[] colors = new int[] {Color.BLACK, Color.WHITE, Color.RED, Color.GREEN, Color.BLUE};
        final String[] colorNames = new String[] {"Black", "White", "Red", "Green", "Blue"};
        for (int index = 0; index < colors.length; index++) {
            final int color = colors[index];
            Button choice = compactButton(colorNames[index]);
            choice.setOnClickListener(new View.OnClickListener() {
                @Override public void onClick(View view) { paint.setBrushColor(color); }
            });
            palette.addView(choice, weightedWidth());
        }
        content.addView(palette, fullWidth());

        LinearLayout customColor = new LinearLayout(this);
        customColor.setOrientation(LinearLayout.HORIZONTAL);
        final EditText hex = new EditText(this);
        hex.setHint("#RRGGBB or #AARRGGBB");
        hex.setSingleLine(true);
        Button applyColor = compactButton("Set Color");
        customColor.addView(hex, weightedWidth());
        customColor.addView(applyColor, weightedWidth());
        content.addView(customColor, fullWidth());
        applyColor.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) {
                try {
                    paint.setBrushColor(Color.parseColor(hex.getText().toString().trim()));
                    setStatusText("Paint color applied");
                } catch (Exception error) {
                    setStatusText("Paint color needs #RRGGBB or #AARRGGBB");
                }
            }
        });

        LinearLayout canvasActions = new LinearLayout(this);
        canvasActions.setOrientation(LinearLayout.HORIZONTAL);
        Button resize = compactButton("Resize / Crop");
        Button clear = compactButton("Clear");
        canvasActions.addView(resize, weightedWidth());
        canvasActions.addView(clear, weightedWidth());
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
        LinearLayout finish = new LinearLayout(this);
        finish.setOrientation(LinearLayout.HORIZONTAL);
        Button save = compactButton("Save as PNG");
        Button saveAndAttach = compactButton("Save + Attach to AI");
        Button cancel = compactButton("Cancel");
        finish.addView(save, weightedWidth());
        finish.addView(saveAndAttach, weightedWidth());
        finish.addView(cancel, weightedWidth());
        content.addView(finish, fullWidth());

        ScrollView editorScroll = new ScrollView(this);
        editorScroll.addView(content, fullWidth());
        final AlertDialog dialog = new AlertDialog.Builder(this)
                .setTitle("Mini Paint - " + width + "x" + height)
                .setView(editorScroll)
                .create();
        save.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) {
                savePaintedImage(paint, name.getText().toString(), dialog, false);
            }
        });
        saveAndAttach.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) {
                savePaintedImage(paint, name.getText().toString(), dialog, true);
            }
        });
        if (suggestAiAttachment) saveAndAttach.requestFocus();
        cancel.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) {
                setStatusText("Paint cancelled; project assets unchanged");
                dialog.dismiss();
            }
        });
        dialog.setOnDismissListener(new android.content.DialogInterface.OnDismissListener() {
            @Override public void onDismiss(android.content.DialogInterface ignored) { paint.dispose(); }
        });
        dialog.show();
    }

    private void savePaintedImage(WorkshopPaintView paint, String name, AlertDialog dialog,
                                  boolean attachToAi) {
        Bitmap snapshot = paint.snapshot();
        try {
            WorkshopImageAssets.AssetInfo saved = WorkshopImageAssets.savePainted(
                    snapshot, activeProject.root, name);
            if (attachToAi) {
                selectedImageAssetProjectId = activeProject.id;
                selectedImageAssets.add(saved.relativePath);
                selectedDesignSketchAssets.add(saved.relativePath);
                persistDesignSketchAssets();
            }
            refreshImageAssetList();
            refreshAiAttachmentStatus();
            setStatusText(attachToAi
                    ? "Design sketch saved and attached to the next AI command: " + saved.relativePath
                    : "Painted image saved as copy: " + saved.relativePath);
            dialog.dismiss();
        } catch (Exception error) {
            setStatusText("Paint save failed: " + error.getMessage());
        } finally {
            snapshot.recycle();
        }
    }

    private void persistDesignSketchAssets() {
        if (activeProject == null) return;
        getSharedPreferences(AI_PREFS, MODE_PRIVATE).edit()
                .putStringSet(AI_PREF_DESIGN_SKETCHES_PREFIX + activeProject.id,
                        new HashSet<String>(selectedDesignSketchAssets))
                .apply();
    }

    private void requestPaintResize(final WorkshopPaintView paint) {
        final EditText width = new EditText(this);
        width.setInputType(InputType.TYPE_CLASS_NUMBER);
        width.setText(Integer.toString(paint.canvasWidth()));
        final EditText height = new EditText(this);
        height.setInputType(InputType.TYPE_CLASS_NUMBER);
        height.setText(Integer.toString(paint.canvasHeight()));
        LinearLayout dimensions = new LinearLayout(this);
        dimensions.setOrientation(LinearLayout.HORIZONTAL);
        dimensions.addView(width, weightedWidth());
        dimensions.addView(height, weightedWidth());
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
        button.setTextSize(11.0f);
        button.setPadding(dp(2), 0, dp(2), 0);
        return button;
    }

    private static LinearLayout.LayoutParams weightedWidth() {
        return new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1.0f);
    }

    private void toggleImageSelection(WorkshopImageAssets.AssetInfo asset) {
        selectedImageAssetProjectId = activeProject == null ? "" : activeProject.id;
        if (!selectedImageAssets.remove(asset.relativePath)) selectedImageAssets.add(asset.relativePath);
        refreshImageAssetList();
        refreshAiAttachmentStatus();
        setStatusText((selectedImageAssets.contains(asset.relativePath) ? "Selected " : "Unselected ")
                + asset.relativePath + " for an explicit future attachment");
    }

    private void refreshAiAttachmentStatus() {
        if (aiAttachmentStatus == null) return;
        if (selectedImageAssets.isEmpty()) {
            aiAttachmentStatus.setText("AI images: none selected; no device media will be sent");
            return;
        }
        try {
            List<WorkshopImageAssets.AssetInfo> selected = selectedAiImageInfos();
            long patches = estimatedImagePatchTokens(selected);
            int sketches = 0;
            for (WorkshopImageAssets.AssetInfo image : selected) {
                if (selectedDesignSketchAssets.contains(image.relativePath)) sketches += 1;
            }
            String sketchText = sketches == 0 ? "" : ", " + sketches + " design sketch"
                    + (sketches == 1 ? "" : "es");
            String costText = AI_PROVIDER_CODEX.equals(selectedAiProvider()) ? ""
                    : " / " + formatAiCostUsd(selectedAiInputCostUsd(patches))
                    + " " + selectedAiModelForPricing() + " input";
            aiAttachmentStatus.setText("AI images: " + selected.size() + " selected" + sketchText
                    + ", about " + patches + " original-detail image tokens" + costText
                    + " (review before Run)");
        } catch (Exception error) {
            aiAttachmentStatus.setText("AI images: selection needs review - " + error.getMessage());
        }
    }

    private void reviewAiImageAttachments() {
        final ArrayList<Bitmap> previews = new ArrayList<>();
        LinearLayout content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        try {
            List<WorkshopImageAssets.AssetInfo> selected = selectedAiImageInfos();
            if (selected.isEmpty()) {
                TextView empty = new TextView(this);
                empty.setText("No project images are selected. Select them from Projects > Image Assets.");
                empty.setPadding(dp(12), dp(12), dp(12), dp(12));
                content.addView(empty, fullWidth());
            }
            for (final WorkshopImageAssets.AssetInfo asset : selected) {
                LinearLayout row = new LinearLayout(this);
                row.setOrientation(LinearLayout.HORIZONTAL);
                row.setGravity(Gravity.CENTER_VERTICAL);
                Bitmap bitmap = WorkshopImageAssets.decodePreview(asset);
                previews.add(bitmap);
                ImageView thumbnail = new ImageView(this);
                thumbnail.setImageBitmap(bitmap);
                thumbnail.setScaleType(ImageView.ScaleType.CENTER_CROP);
                thumbnail.setContentDescription("Preview of selected AI attachment " + asset.relativePath);
                row.addView(thumbnail, new LinearLayout.LayoutParams(dp(72), dp(72)));
                TextView label = new TextView(this);
                boolean designSketch = selectedDesignSketchAssets.contains(asset.relativePath);
                label.setText(asset.relativePath + "\n" + asset.width + "x" + asset.height
                        + " - original detail - " + WorkshopAiImageContext.reviewLabel(designSketch));
                label.setPadding(dp(8), 0, dp(8), 0);
                row.addView(label, weightedWidth());
                Button remove = compactButton("Remove");
                remove.setOnClickListener(new View.OnClickListener() {
                    @Override public void onClick(View view) {
                        selectedImageAssets.remove(asset.relativePath);
                        refreshImageAssetList();
                        refreshAiAttachmentStatus();
                        setStatusText("Removed AI attachment: " + asset.relativePath);
                        row.setVisibility(View.GONE);
                    }
                });
                row.addView(remove, new LinearLayout.LayoutParams(dp(88), LinearLayout.LayoutParams.WRAP_CONTENT));
                content.addView(row, fullWidth());
            }
        } catch (Exception error) {
            TextView failure = new TextView(this);
            failure.setText("Attachment review failed: " + error.getMessage());
            content.addView(failure, fullWidth());
        }
        ScrollView scroll = new ScrollView(this);
        scroll.addView(content, fullWidth());
        AlertDialog dialog = new AlertDialog.Builder(this)
                .setTitle("AI Image Attachments")
                .setMessage("Only these app-private project images will be included in the next AI request.")
                .setView(scroll)
                .setPositiveButton("Done", null)
                .create();
        dialog.setOnDismissListener(new android.content.DialogInterface.OnDismissListener() {
            @Override public void onDismiss(android.content.DialogInterface ignored) {
                for (Bitmap bitmap : previews) if (!bitmap.isRecycled()) bitmap.recycle();
            }
        });
        dialog.show();
    }

    private void capturePreviewForAi() {
        if (aiRunActive) {
            setStatusText("Preview capture blocked while an AI run is active");
            return;
        }
        if (gamePreview == null || gamePreview.getWidth() <= 0 || gamePreview.getHeight() <= 0) {
            setStatusText("Preview capture needs a visible rendered game frame");
            return;
        }
        screenshotAttachmentStatus.setText("AI preview: capturing rendered pixels");
        gamePreview.captureFrame(new GamePreviewView.CaptureCallback() {
            @Override public void onCaptured(final Bitmap bitmap, final String error, final int[] capturedFrame) {
                runOnUiThread(new Runnable() {
                    @Override public void run() {
                        if (bitmap == null) {
                            setStatusText("Preview capture failed: " + error);
                            refreshScreenshotAttachmentStatus();
                            return;
                        }
                        if (pendingPreviewScreenshot != null && !pendingPreviewScreenshot.isRecycled()) {
                            pendingPreviewScreenshot.recycle();
                        }
                        pendingPreviewScreenshot = bitmap;
                        try {
                            pendingPreviewLogicalSnapshot = logicalRenderSnapshot(capturedFrame);
                        } catch (Exception snapshotError) {
                            pendingPreviewLogicalSnapshot = null;
                            setStatusText("Pixels captured; logical snapshot failed: " + snapshotError.getMessage());
                        }
                        attachPreviewPixels = false;
                        attachPreviewLogicalSnapshot = false;
                        refreshScreenshotAttachmentStatus();
                        reviewPreviewCaptureForAi();
                    }
                });
            }
        });
    }

    private void clearPendingPreviewCapture() {
        if (pendingPreviewScreenshot != null && !pendingPreviewScreenshot.isRecycled()) {
            pendingPreviewScreenshot.recycle();
        }
        pendingPreviewScreenshot = null;
        pendingPreviewLogicalSnapshot = null;
        attachPreviewPixels = false;
        attachPreviewLogicalSnapshot = false;
        refreshScreenshotAttachmentStatus();
    }

    private void refreshScreenshotAttachmentStatus() {
        if (screenshotAttachmentStatus == null) return;
        if (pendingPreviewScreenshot == null || pendingPreviewScreenshot.isRecycled()) {
            screenshotAttachmentStatus.setText("AI preview: no pixel capture or logical snapshot selected");
            return;
        }
        String selections;
        if (attachPreviewPixels && attachPreviewLogicalSnapshot) selections = "pixels + logical snapshot";
        else if (attachPreviewPixels) selections = "pixels";
        else if (attachPreviewLogicalSnapshot) selections = "logical snapshot";
        else selections = "captured, nothing approved to send";
        long patches = ((pendingPreviewScreenshot.getWidth() + 31L) / 32L)
                * ((pendingPreviewScreenshot.getHeight() + 31L) / 32L);
        screenshotAttachmentStatus.setText("AI preview: " + selections + " - "
                + pendingPreviewScreenshot.getWidth() + "x" + pendingPreviewScreenshot.getHeight()
                + (attachPreviewPixels ? ", about " + patches + " image tokens / "
                        + formatAiCostUsd(selectedAiInputCostUsd(patches))
                        + " " + selectedAiModelForPricing() + " input" : "") + " (tap to review)");
    }

    private void reviewPreviewCaptureForAi() {
        if (pendingPreviewScreenshot == null || pendingPreviewScreenshot.isRecycled()) {
            setStatusText("Capture the preview before reviewing it");
            return;
        }
        LinearLayout content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        content.setPadding(dp(8), dp(8), dp(8), dp(8));
        ImageView preview = new ImageView(this);
        preview.setImageBitmap(pendingPreviewScreenshot);
        preview.setAdjustViewBounds(true);
        preview.setScaleType(ImageView.ScaleType.FIT_CENTER);
        preview.setContentDescription("Captured rendered game preview awaiting AI attachment consent");
        content.addView(preview, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT, dp(320)));
        final CheckBox pixels = new CheckBox(this);
        pixels.setText("Attach these rendered pixels to the next AI request");
        pixels.setChecked(attachPreviewPixels);
        content.addView(pixels, fullWidth());
        final CheckBox logical = new CheckBox(this);
        logical.setText("Attach logical render/runtime/input snapshot as text context");
        logical.setChecked(attachPreviewLogicalSnapshot);
        logical.setEnabled(pendingPreviewLogicalSnapshot != null);
        content.addView(logical, fullWidth());
        new AlertDialog.Builder(this)
                .setTitle("Review Preview Capture")
                .setMessage("Nothing is sent until selected here and Queue AI Change is pressed.")
                .setView(content)
                .setPositiveButton("Apply Selection", new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        attachPreviewPixels = pixels.isChecked();
                        attachPreviewLogicalSnapshot = logical.isChecked();
                        refreshScreenshotAttachmentStatus();
                        setStatusText("AI preview attachment selection updated");
                    }
                })
                .setNeutralButton("Remove Capture", new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface dialog, int which) {
                        pendingPreviewScreenshot.recycle();
                        pendingPreviewScreenshot = null;
                        pendingPreviewLogicalSnapshot = null;
                        attachPreviewPixels = false;
                        attachPreviewLogicalSnapshot = false;
                        refreshScreenshotAttachmentStatus();
                        setStatusText("AI preview capture removed");
                    }
                })
                .setNegativeButton("Cancel", null)
                .show();
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
                            if (selectedImageAssets.remove(asset.relativePath)) {
                                selectedImageAssets.add(renamed.relativePath);
                            }
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
                            selectedImageAssets.remove(asset.relativePath);
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
        if (aiRunActive || githubOperationActive || projectIoActive || hasPendingSourceEdit()) {
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
                            diagnosticFile = selectedRecoveryEntry.path;
                            diagnosticSymbol = selectedRecoveryEntry.symbol;
                            diagnosticLine = 0;
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
        JSONArray runs = testRun.optJSONArray("stasis_test_files");
        if (runs == null) return;
        for (int runIndex = 0; runIndex < runs.length(); runIndex += 1) {
            JSONObject run = runs.optJSONObject(runIndex);
            JSONArray results = run == null ? null : run.optJSONArray("results");
            if (results == null) continue;
            for (int resultIndex = 0; resultIndex < results.length(); resultIndex += 1) {
                JSONObject result = results.optJSONObject(resultIndex);
                if (result == null || result.optBoolean("passed", false)) continue;
                diagnosticFile = result.optString("file", "");
                diagnosticSymbol = result.optString("name", "");
                diagnosticLine = result.optInt("line", 0);
                String error = result.optString("error", "");
                diagnosticStatus.setText("Test failure\nfile=" + diagnosticFile
                        + (diagnosticLine > 0 ? "\nline=" + diagnosticLine : "")
                        + (diagnosticSymbol.isEmpty() ? "" : "\ntest=" + diagnosticSymbol)
                        + (error.isEmpty() ? "" : "\n" + error));
                return;
            }
        }
    }

    private void goToDiagnosticSource() {
        if (diagnosticFile.isEmpty()) {
            setStatusText("No source diagnostic is available");
            return;
        }
        ProjectSnapshot project = loadBundledProject();
        for (SymbolSection section : project.sections) {
            for (SymbolGroup group : section.groups) {
                for (SymbolEntry symbol : group.symbols) {
                    if (symbol.file.equals(diagnosticFile)
                            && (diagnosticSymbol.isEmpty() || symbol.name.equals(diagnosticSymbol))) {
                        showSymbol(symbol);
                        if (diagnosticLine > 0) {
                            int absoluteOffset = sourceOffsetForLine(symbol.sourceFile.source, diagnosticLine);
                            int symbolOffset = Math.max(0,
                                    Math.min(symbol.source.length(), absoluteOffset - symbol.start));
                            sourceEditor.setSelection(symbolOffset);
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
        setStatusText("Diagnostic file is available but its symbol could not be parsed");
    }

    private static int sourceOffsetForLine(String source, int oneBasedLine) {
        if (oneBasedLine <= 1) return 0;
        int line = 1;
        for (int index = 0; index < source.length(); index += 1) {
            if (source.charAt(index) == '\n') {
                line += 1;
                if (line == oneBasedLine) return index + 1;
            }
        }
        return source.length();
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

    private void refreshChangeSummary(ProjectSnapshot currentProject) {
        if (changeSummary == null) {
            return;
        }
        try {
            ProjectSnapshot baseline = loadProjectBaselineSnapshot();
            changeSummary.setText(formatChangeSummary(baseline, currentProject));
        } catch (IOException error) {
            changeSummary.setText("Changed symbols:\n  Unable to read project baseline: " + error.getMessage());
        }
    }

    private void showRawDiffReview() {
        if (changeSummary == null) {
            return;
        }
        try {
            changeSummary.setText(formatRawFileDiffs(loadProjectBaselineSnapshot(), loadBundledProject()));
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
        String expectedReady = "format=3\ntemplate_id=" + templateId + "\n";
        if (readyFile.isFile() && expectedReady.equals(readTextFile(readyFile))) return;
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
        return ProjectSnapshot.from(files);
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
                file = testFileForAiPath("tests/manual_test_" + number + ".test.stasis");
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
            appendAiFunction(project, "src/root.stasis", source);
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

    private File projectRoot() {
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

    private File testFileForAiPath(String path) throws IOException {
        String normalized = path == null ? "" : path.replace('\\', '/').trim();
        if (!normalized.startsWith("tests/") || normalized.contains("..")) {
            throw new IOException("AI test files must live under tests/: " + normalized);
        }
        if (!normalized.endsWith(".test.stasis")) {
            throw new IOException("AI test files must end with .test.stasis: " + normalized);
        }
        File file = new File(projectRoot(), normalized.replace('/', File.separatorChar));
        relativeProjectPath(file);
        return file;
    }

    private List<File> listProjectTestFiles() throws IOException {
        List<File> files = new ArrayList<>();
        collectTestFiles(new File(projectRoot(), "tests"), files);
        Collections.sort(files, new Comparator<File>() {
            @Override
            public int compare(File left, File right) {
                return left.getAbsolutePath().compareTo(right.getAbsolutePath());
            }
        });
        return files;
    }

    private void collectTestFiles(File file, List<File> out) throws IOException {
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
                    collectTestFiles(child, out);
                }
            }
            return;
        }
        String path = relativeProjectPath(file);
        if (path.endsWith(".ai_test.json") || path.endsWith(".test.stasis")) {
            out.add(file);
        }
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
    private String projectRootPath() {
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
                for (String file : template.sourceFiles) {
                    try {
                        ensureProjectFile(assets, template.assetRoot + file, new File(projectRoot, file));
                    } catch (IOException ignored) {
                        // The recursive load below includes files that were seeded successfully.
                    }
                }
                for (String file : template.testFiles) {
                    try {
                        ensureProjectFile(assets, template.assetRoot + file, new File(projectRoot, file));
                    } catch (IOException ignored) {
                        // The recursive load below includes files that were seeded successfully.
                    }
                }
                for (String file : template.auxiliaryFiles) {
                    try {
                        ensureProjectFile(assets, template.assetRoot + file, new File(projectRoot, file));
                    } catch (IOException ignored) {
                        // Optional template support files do not prevent source discovery.
                    }
                }
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

        return ProjectSnapshot.from(files);
    }

    private AiApiResponse callCodexResponses(String requestJson) throws Exception {
        JSONObject payload = new JSONObject();
        payload.put("model", "");
        payload.put("instructions", "");
        payload.put("input", buildAiOpenAiInput(requestJson, true, false));
        payload.put("tools", new JSONArray());
        payload.put("tool_choice", "auto");
        payload.put("parallel_tool_calls", false);
        payload.put("reasoning", new JSONObject().put("effort",
                isFastPathRequest(requestJson) ? "minimal" : "medium").put("summary",
                isFastPathRequest(requestJson) ? "none" : "auto"));
        payload.put("store", false);
        payload.put("stream", true);
        payload.put("include", new JSONArray().put("reasoning.encrypted_content"));
        payload.put("prompt_cache_key", AI_PROMPT_CACHE_KEY);
        payload.put("text", buildAiResponseTextFormat());

        long generation = nativeCodexBeginResponse();
        if (generation == 0) throw new IOException("Phone-native Codex library is not packaged");
        throwIfAiCancelled();
        JSONObject result = new JSONObject(nativeCodexResponse(
                codexHomePath(), payload.toString(), generation));
        if (!"ok".equals(result.optString("status", ""))) {
            throw new IOException(result.optString("error", "Phone-native Codex request failed"));
        }
        JSONObject response = result.optJSONObject("response");
        if (response == null) throw new IOException("Phone-native Codex returned no response object");
        String model = result.optString("model", "codex-default");
        return new AiApiResponse(response.toString(), extractAiUsage(response.toString()), model);
    }

    private static boolean isFastPathRequest(String requestJson) {
        try {
            JSONObject request = new JSONObject(requestJson);
            JSONObject fastPath = request.optJSONObject("fast_path");
            if (fastPath != null) return fastPath.optBoolean("enabled", false);
            JSONObject original = request.optJSONObject("original_request");
            fastPath = original == null ? null : original.optJSONObject("fast_path");
            return fastPath != null && fastPath.optBoolean("enabled", false);
        } catch (Exception ignored) {
            return false;
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

    private void ensureProjectFile(AssetManager assets, String assetPath, File diskFile) throws IOException {
        if (diskFile.isFile()) {
            return;
        }
        File parent = diskFile.getParentFile();
        if (parent != null && !parent.isDirectory() && !parent.mkdirs()) {
            throw new IOException("failed to create " + parent.getAbsolutePath());
        }

        writeTextFile(diskFile, readAsset(assets, assetPath));
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

    private File aiTraceLogFile() {
        return new File(getFilesDir(), AI_TRACE_LOG);
    }

    private String aiTraceLogPath() {
        return aiTraceLogFile().getAbsolutePath();
    }

    private void appendAiTraceFields(String event, String key1, String value1, String key2, String value2, String key3, String value3) {
        try {
            JSONObject fields = new JSONObject();
            if (key1 != null) {
                fields.put(key1, value1 == null ? "" : value1);
            }
            if (key2 != null) {
                fields.put(key2, value2 == null ? "" : value2);
            }
            if (key3 != null) {
                fields.put(key3, value3 == null ? "" : value3);
            }
            appendAiTrace(event, fields);
        } catch (Exception ignored) {
            // Trace logging must not break editing or gameplay.
        }
    }
    private void appendAiTrace(String event, JSONObject fields) {
        try {
            File file = aiTraceLogFile();
            long now = System.currentTimeMillis();
            if (file.isFile() && now - file.lastModified() > AI_TRACE_RETENTION_MS) {
                writeTextFile(file, "");
            }
            JSONObject entry = new JSONObject();
            entry.put("timestamp_ms", now);
            entry.put("event", event);
            if (fields != null) {
                entry.put("data", fields);
            }
            FileOutputStream output = new FileOutputStream(file, true);
            try {
                output.write((entry.toString() + "\n").getBytes(StandardCharsets.UTF_8));
            } finally {
                output.close();
            }
        } catch (Exception ignored) {
            // Trace logging must not break editing or gameplay.
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

    private GradientDrawable createPanelBackground(int fill, int stroke) {
        GradientDrawable drawable = new GradientDrawable();
        drawable.setColor(fill);
        drawable.setStroke(dp(1), stroke);
        drawable.setCornerRadius(dp(6));
        return drawable;
    }

    private int dp(int value) {
        return Math.round(value * getResources().getDisplayMetrics().density);
    }

    private static final class AiApiResponse {
        final String body;
        final JSONObject usage;
        final String model;

        AiApiResponse(String body, JSONObject usage, String model) {
            this.body = body;
            this.usage = usage;
            this.model = model;
        }
    }

    private static final class AiAgentResult {
        final String aiJson;
        final JSONObject usageJson;
        final String usageSummary;
        final int finalStep;
        final int finalActionCount;
        final List<AiGeneratedImageCandidate> generatedImages;

        AiAgentResult(String aiJson, JSONObject usageJson, String usageSummary, int finalStep,
                int finalActionCount, List<AiGeneratedImageCandidate> generatedImages) {
            this.aiJson = aiJson;
            this.usageJson = usageJson;
            this.usageSummary = usageSummary;
            this.finalStep = finalStep;
            this.finalActionCount = finalActionCount;
            this.generatedImages = Collections.unmodifiableList(
                    new ArrayList<AiGeneratedImageCandidate>(generatedImages));
        }
    }

    private static final class AiGeneratedImageCandidate {
        final byte[] pngBytes;
        final int width;
        final int height;

        AiGeneratedImageCandidate(byte[] pngBytes, int width, int height) {
            this.pngBytes = pngBytes;
            this.width = width;
            this.height = height;
        }
    }

    private static final class AiImageAttachment {
        final String projectPath;
        final String mimeType;
        final byte[] bytes;
        final int width;
        final int height;
        final String contextKind;

        AiImageAttachment(String projectPath, String mimeType, byte[] bytes, int width, int height,
                          String contextKind) {
            this.projectPath = projectPath;
            this.mimeType = mimeType;
            this.bytes = bytes;
            this.width = width;
            this.height = height;
            this.contextKind = contextKind;
        }

        long estimatedPatchTokens() {
            return ((width + 31L) / 32L) * ((height + 31L) / 32L);
        }
    }

    private static final class AiUsageAccumulator {
        private final JSONArray calls = new JSONArray();
        private long inputTokens;
        private long cachedInputTokens;
        private long cacheWriteInputTokens;
        private long outputTokens;
        private double estimatedCostUsd;
        private double imageGenerationCostUsd;
        private int generatedImageCount;
        private boolean costAvailable = true;
        private double lastCallEstimatedCostUsd;
        private boolean lastCallCostAvailable;

        void add(String model, JSONObject usage) throws Exception {
            long callInputTokens = usageTokenCount(usage, "input_tokens", "prompt_tokens");
            long callCachedInputTokens = cachedInputTokenCount(usage);
            long callCacheWriteInputTokens = cacheWriteInputTokenCount(usage);
            long callOutputTokens = usageTokenCount(usage, "output_tokens", "completion_tokens");
            boolean callCostAvailable = hasKnownAiPricing(model);
            double callEstimatedCostUsd = estimateAiCostUsd(model, callInputTokens, callCachedInputTokens, callCacheWriteInputTokens, callOutputTokens);
            lastCallEstimatedCostUsd = callEstimatedCostUsd;
            lastCallCostAvailable = callCostAvailable;

            JSONObject call = new JSONObject();
            call.put("turn", calls.length() + 1);
            call.put("model", model);
            call.put("input_tokens", callInputTokens);
            call.put("cached_input_tokens", callCachedInputTokens);
            call.put("cache_write_input_tokens", callCacheWriteInputTokens);
            call.put("output_tokens", callOutputTokens);
            call.put("estimated_cost_usd", callEstimatedCostUsd);
            call.put("cost_available", callCostAvailable);
            calls.put(call);

            inputTokens += callInputTokens;
            cachedInputTokens += callCachedInputTokens;
            cacheWriteInputTokens += callCacheWriteInputTokens;
            outputTokens += callOutputTokens;
            estimatedCostUsd += callEstimatedCostUsd;
            costAvailable = costAvailable && callCostAvailable;
        }

        void addUnpriced(String model, JSONObject usage) throws Exception {
            long callInputTokens = usageTokenCount(usage, "input_tokens", "prompt_tokens");
            long callCachedInputTokens = cachedInputTokenCount(usage);
            long callCacheWriteInputTokens = cacheWriteInputTokenCount(usage);
            long callOutputTokens = usageTokenCount(usage, "output_tokens", "completion_tokens");
            calls.put(new JSONObject()
                    .put("turn", calls.length() + 1)
                    .put("model", model)
                    .put("input_tokens", callInputTokens)
                    .put("cached_input_tokens", callCachedInputTokens)
                    .put("cache_write_input_tokens", callCacheWriteInputTokens)
                    .put("output_tokens", callOutputTokens)
                    .put("billing", "codex_subscription"));
            inputTokens += callInputTokens;
            cachedInputTokens += callCachedInputTokens;
            cacheWriteInputTokens += callCacheWriteInputTokens;
            outputTokens += callOutputTokens;
            costAvailable = false;
            lastCallCostAvailable = false;
        }

        void addImageGenerationCost(double costUsd, int count) {
            imageGenerationCostUsd += costUsd;
            generatedImageCount += count;
            estimatedCostUsd += costUsd;
        }

        JSONObject toJson(String model) throws Exception {
            JSONObject json = new JSONObject();
            json.put("model", model);
            json.put("calls", calls);
            json.put("turns", calls.length());
            json.put("input_tokens", inputTokens);
            json.put("cached_input_tokens", cachedInputTokens);
            json.put("cache_write_input_tokens", cacheWriteInputTokens);
            json.put("output_tokens", outputTokens);
            json.put("estimated_cost_usd", estimatedCostUsd);
            json.put("image_generation_cost_usd", imageGenerationCostUsd);
            json.put("generated_image_count", generatedImageCount);
            json.put("cost_available", costAvailable);
            return json;
        }

        String summary() {
            StringBuilder builder = new StringBuilder();
            builder.append("estimated cost=");
            if (costAvailable) {
                builder.append(formatAiCostUsd(estimatedCostUsd));
            } else {
                builder.append("unavailable");
            }
            return builder.toString();
        }

        String subscriptionSummary() {
            return "Codex subscription, turns=" + calls.length();
        }
    }
    private final class AiAgentSession {
        int currentStep;
        int actionCount;
        int successfulWriteCount;
        int rolledBackWriteCount;
        String lastToolSummary = "none";
        String lastToolError = "";
        String workingNotes = "";
        TreeSet<String> lastPassingTestKeys = new TreeSet<>();
        JSONObject latestTestObservation = new JSONObject();
        final WorkshopAiObservationMemory observationMemory = new WorkshopAiObservationMemory();
        final WorkshopAiToolLoopPolicy toolLoopPolicy =
                new WorkshopAiToolLoopPolicy(MAX_AI_READ_ONLY_BATCHES);
        boolean deferBatchCompile;
        private ProjectSnapshot cachedProject;

        ProjectSnapshot project() {
            if (cachedProject == null) {
                cachedProject = loadBundledProject();
            }
            return cachedProject;
        }

        boolean latestRunnableTestsPassed() {
            return latestTestObservation != null && latestTestObservation.optBoolean("all_runnable_tests_passed", false);
        }

        void rememberToolObservations(JSONArray observations) throws Exception {
            for (int index = 0; index < observations.length(); index += 1) {
                JSONObject observation = observations.optJSONObject(index);
                if (observation == null) continue;
                String tool = observation.optString("tool", "observation");
                JSONObject args = observation.optJSONObject("args");
                String key = tool + "|" + (args == null ? "{}" : args.toString());
                observationMemory.remember(key, observation.toString());
            }
        }

        JSONArray retainedToolObservations() throws Exception {
            JSONArray retained = new JSONArray();
            for (String observation : observationMemory.snapshotNewestFirst()) {
                retained.put(new JSONObject(observation));
            }
            return retained;
        }

        void invalidateProject() {
            cachedProject = null;
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

    private static boolean sameSymbolIdentity(SymbolEntry left, SymbolEntry right) {
        return left.kind.equals(right.kind)
                && left.file.equals(right.file)
                && left.owner.equals(right.owner)
                && left.name.equals(right.name);
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
            void onCaptured(Bitmap bitmap, String error, int[] capturedFrame);
        }

        private final MainActivity activity;
        private final PreviewRenderer renderer;
        private int touchX;
        private int touchY;
        private boolean touchActive;

        GamePreviewView(MainActivity activity) {
            super(activity);
            this.activity = activity;
            setEGLContextClientVersion(2);
            renderer = new PreviewRenderer(activity);
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

        void setRenderFrameValues(int[] frameValues) {
            renderer.setFrameValues(frameValues);
            requestRender();
        }

        void captureFrame(CaptureCallback callback) {
            renderer.requestCapture(callback);
            requestRender();
        }

        @Override
        public boolean onTouchEvent(MotionEvent event) {
            touchX = Math.round(event.getX());
            touchY = Math.round(event.getY());
            int action = event.getActionMasked();
            if (BuildConfig.STASIS_PUBLISHED_BUILD && action == MotionEvent.ACTION_POINTER_DOWN && event.getPointerCount() >= 3) {
                activity.toggleBenchmarkHudFromPreview();
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
                "void main() {" +
                "  gl_FragColor = texture2D(uTexture, vTexCoord) * vColor;" +
                "}";

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
        private final int[] lastDrawnFrame = new int[RENDER_FRAME_I32_CAPACITY];
        private int program;
        private int positionHandle;
        private int resolutionHandle;
        private int colorHandle;
        private int textureProgram;
        private int texturePositionHandle;
        private int textureCoordHandle;
        private int textureColorHandle;
        private int textureResolutionHandle;
        private int textureSamplerHandle;
        private int ballTexture;
        private int surfaceWidth = 1;
        private int surfaceHeight = 1;
        private GamePreviewView.CaptureCallback pendingCapture;

        PreviewRenderer(MainActivity activity) {
            this.activity = activity;
        }

        synchronized void setFrameValues(int[] values) {
            System.arraycopy(values, 0, frameValues, 0, RENDER_FRAME_I32_CAPACITY);
        }

        synchronized void requestCapture(GamePreviewView.CaptureCallback callback) {
            if (pendingCapture != null) {
                pendingCapture.onCaptured(null, "a newer preview capture replaced this request", new int[0]);
            }
            pendingCapture = callback;
        }

        @Override
        public void onSurfaceCreated(javax.microedition.khronos.opengles.GL10 gl, javax.microedition.khronos.egl.EGLConfig config) {
            program = createProgram(VERTEX_SHADER, FRAGMENT_SHADER);
            positionHandle = GLES20.glGetAttribLocation(program, "aPosition");
            resolutionHandle = GLES20.glGetUniformLocation(program, "uResolution");
            colorHandle = GLES20.glGetAttribLocation(program, "aColor");
            textureProgram = createProgram(TEXTURE_VERTEX_SHADER, TEXTURE_FRAGMENT_SHADER);
            texturePositionHandle = GLES20.glGetAttribLocation(textureProgram, "aPosition");
            textureCoordHandle = GLES20.glGetAttribLocation(textureProgram, "aTexCoord");
            textureColorHandle = GLES20.glGetAttribLocation(textureProgram, "aColor");
            textureResolutionHandle = GLES20.glGetUniformLocation(textureProgram, "uResolution");
            textureSamplerHandle = GLES20.glGetUniformLocation(textureProgram, "uTexture");
            ballTexture = createCircleTexture();
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
            long renderStartNanos = System.nanoTime();
            GLES20.glClear(GLES20.GL_COLOR_BUFFER_BIT);
            GLES20.glUseProgram(program);
            GLES20.glUniform2f(resolutionHandle, (float)surfaceWidth, (float)surfaceHeight);

            vertexBuffer.clear();
            spriteVertexBuffer.clear();
            int vertexCount = 0;
            int spriteVertexCount = 0;
            synchronized (this) {
                System.arraycopy(frameValues, 0, lastDrawnFrame, 0, lastDrawnFrame.length);
                int commandCount = Math.max(0, Math.min(MAX_RENDER_COMMANDS, frameValues[5]));
                for (int index = 0; index < commandCount; index += 1) {
                    int base = RENDER_FRAME_HEADER_SIZE + index * RENDER_COMMAND_STRIDE;
                    int kind = frameValues[base];
                    if (kind == 1) {
                        appendRect(base);
                        vertexCount += RECT_VERTICES;
                    } else if (kind == 2 && frameValues[base + 6] != 0) {
                        appendSprite(base);
                        spriteVertexCount += RECT_VERTICES;
                    }
                }
            }
            vertexBuffer.flip();
            spriteVertexBuffer.flip();
            if (vertexCount > 0) {
                drawBatch(vertexCount);
            }
            if (spriteVertexCount > 0) {
                drawSpriteBatch(spriteVertexCount);
            }
            captureRenderedPixelsIfRequested();
            activity.recordRenderTimeNanos(System.nanoTime() - renderStartNanos);
        }

        private void captureRenderedPixelsIfRequested() {
            GamePreviewView.CaptureCallback callback;
            synchronized (this) {
                callback = pendingCapture;
                pendingCapture = null;
            }
            if (callback == null) return;
            int[] capturedFrame = new int[RENDER_FRAME_I32_CAPACITY];
            synchronized (this) {
                System.arraycopy(lastDrawnFrame, 0, capturedFrame, 0, capturedFrame.length);
            }
            try {
                long pixelCount = (long)surfaceWidth * (long)surfaceHeight;
                if (pixelCount > MAX_PREVIEW_CAPTURE_PIXELS) {
                    callback.onCaptured(null, "preview framebuffer exceeds the 8 megapixel capture limit", capturedFrame);
                    return;
                }
                IntBuffer pixels = ByteBuffer.allocateDirect(surfaceWidth * surfaceHeight * 4)
                        .order(ByteOrder.nativeOrder()).asIntBuffer();
                GLES20.glReadPixels(0, 0, surfaceWidth, surfaceHeight, GLES20.GL_RGBA,
                        GLES20.GL_UNSIGNED_BYTE, pixels);
                int[] flipped = new int[surfaceWidth * surfaceHeight];
                for (int y = 0; y < surfaceHeight; y++) {
                    int sourceRow = y * surfaceWidth;
                    int targetRow = (surfaceHeight - y - 1) * surfaceWidth;
                    for (int x = 0; x < surfaceWidth; x++) {
                        int rgba = pixels.get(sourceRow + x);
                        int redBlueSwapped = (rgba & 0xff00ff00)
                                | ((rgba << 16) & 0x00ff0000) | ((rgba >> 16) & 0x000000ff);
                        flipped[targetRow + x] = redBlueSwapped;
                    }
                }
                Bitmap full = Bitmap.createBitmap(flipped, surfaceWidth, surfaceHeight, Bitmap.Config.ARGB_8888);
                int largest = Math.max(surfaceWidth, surfaceHeight);
                if (largest <= 1024) {
                    callback.onCaptured(full, "", capturedFrame);
                    return;
                }
                float scale = 1024.0f / largest;
                Bitmap bounded = Bitmap.createScaledBitmap(full, Math.max(1, Math.round(surfaceWidth * scale)),
                        Math.max(1, Math.round(surfaceHeight * scale)), true);
                full.recycle();
                callback.onCaptured(bounded, "", capturedFrame);
            } catch (OutOfMemoryError error) {
                callback.onCaptured(null, "not enough memory for bounded pixel capture", capturedFrame);
            } catch (RuntimeException error) {
                callback.onCaptured(null, error.getMessage(), capturedFrame);
            }
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

        private void appendSprite(int base) {
            int color = frameValues[base + 5];
            float red = ((color >> 16) & 255) / 255.0f;
            float green = ((color >> 8) & 255) / 255.0f;
            float blue = (color & 255) / 255.0f;
            float left = frameValues[base + 1];
            float top = frameValues[base + 2];
            float right = frameValues[base + 1] + frameValues[base + 3];
            float bottom = frameValues[base + 2] + frameValues[base + 4];
            putSpriteVertex(left, top, 0.0f, 0.0f, red, green, blue);
            putSpriteVertex(right, top, 1.0f, 0.0f, red, green, blue);
            putSpriteVertex(left, bottom, 0.0f, 1.0f, red, green, blue);
            putSpriteVertex(right, top, 1.0f, 0.0f, red, green, blue);
            putSpriteVertex(right, bottom, 1.0f, 1.0f, red, green, blue);
            putSpriteVertex(left, bottom, 0.0f, 1.0f, red, green, blue);
        }

        private void putSpriteVertex(float x, float y, float u, float v, float red, float green, float blue) {
            spriteVertexBuffer.put(x).put(y).put(u).put(v).put(red).put(green).put(blue).put(1.0f);
        }
        private void drawBatch(int vertexCount) {
            GLES20.glEnableVertexAttribArray(positionHandle);
            GLES20.glEnableVertexAttribArray(colorHandle);
            vertexBuffer.position(0);
            GLES20.glVertexAttribPointer(positionHandle, 2, GLES20.GL_FLOAT, false, RENDER_VERTEX_BYTES, vertexBuffer);
            vertexBuffer.position(2);
            GLES20.glVertexAttribPointer(colorHandle, 4, GLES20.GL_FLOAT, false, RENDER_VERTEX_BYTES, vertexBuffer);
            GLES20.glDrawArrays(GLES20.GL_TRIANGLES, 0, vertexCount);
            vertexBuffer.position(0);
            GLES20.glDisableVertexAttribArray(colorHandle);
            GLES20.glDisableVertexAttribArray(positionHandle);
        }

        private void drawSpriteBatch(int vertexCount) {
            GLES20.glUseProgram(textureProgram);
            GLES20.glUniform2f(textureResolutionHandle, (float)surfaceWidth, (float)surfaceHeight);
            GLES20.glActiveTexture(GLES20.GL_TEXTURE0);
            GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, ballTexture);
            GLES20.glUniform1i(textureSamplerHandle, 0);
            GLES20.glEnableVertexAttribArray(texturePositionHandle);
            GLES20.glEnableVertexAttribArray(textureCoordHandle);
            GLES20.glEnableVertexAttribArray(textureColorHandle);
            spriteVertexBuffer.position(0);
            GLES20.glVertexAttribPointer(texturePositionHandle, 2, GLES20.GL_FLOAT, false, SPRITE_VERTEX_BYTES, spriteVertexBuffer);
            spriteVertexBuffer.position(2);
            GLES20.glVertexAttribPointer(textureCoordHandle, 2, GLES20.GL_FLOAT, false, SPRITE_VERTEX_BYTES, spriteVertexBuffer);
            spriteVertexBuffer.position(4);
            GLES20.glVertexAttribPointer(textureColorHandle, 4, GLES20.GL_FLOAT, false, SPRITE_VERTEX_BYTES, spriteVertexBuffer);
            GLES20.glDrawArrays(GLES20.GL_TRIANGLES, 0, vertexCount);
            spriteVertexBuffer.position(0);
            GLES20.glDisableVertexAttribArray(textureColorHandle);
            GLES20.glDisableVertexAttribArray(textureCoordHandle);
            GLES20.glDisableVertexAttribArray(texturePositionHandle);
            GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, 0);
        }

        private static int createCircleTexture() {
            final int size = 32;
            ByteBuffer pixels = ByteBuffer.allocateDirect(size * size * 4);
            float center = (size - 1) / 2.0f;
            float radius = center - 1.0f;
            for (int y = 0; y < size; y += 1) {
                for (int x = 0; x < size; x += 1) {
                    float dx = x - center;
                    float dy = y - center;
                    int alpha = dx * dx + dy * dy <= radius * radius ? 255 : 0;
                    pixels.put((byte)255).put((byte)255).put((byte)255).put((byte)alpha);
                }
            }
            pixels.flip();
            int[] textures = new int[1];
            GLES20.glGenTextures(1, textures, 0);
            GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, textures[0]);
            GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, GLES20.GL_TEXTURE_MIN_FILTER, GLES20.GL_LINEAR);
            GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, GLES20.GL_TEXTURE_MAG_FILTER, GLES20.GL_LINEAR);
            GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, GLES20.GL_TEXTURE_WRAP_S, GLES20.GL_CLAMP_TO_EDGE);
            GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, GLES20.GL_TEXTURE_WRAP_T, GLES20.GL_CLAMP_TO_EDGE);
            GLES20.glTexImage2D(GLES20.GL_TEXTURE_2D, 0, GLES20.GL_RGBA, size, size, 0, GLES20.GL_RGBA, GLES20.GL_UNSIGNED_BYTE, pixels);
            GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, 0);
            return textures[0];
        }

        private static int createProgram(String vertexSource, String fragmentSource) {
            int vertexShader = compileShader(GLES20.GL_VERTEX_SHADER, vertexSource);
            int fragmentShader = compileShader(GLES20.GL_FRAGMENT_SHADER, fragmentSource);
            int program = GLES20.glCreateProgram();
            GLES20.glAttachShader(program, vertexShader);
            GLES20.glAttachShader(program, fragmentShader);
            GLES20.glLinkProgram(program);
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

    private static final class AiCancelledException extends Exception {
        AiCancelledException() {
            super("AI run cancelled");
        }
    }

    private static final class RollingMetric {
        private static final long WINDOW_NANOS = 5_000_000_000L;
        private static final int CAPACITY = 600;
        private final long[] timestamps = new long[CAPACITY];
        private final long[] durations = new long[CAPACITY];
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
        }

        String displayName() {
            if ("struct".equals(kind)) {
                return signature;
            }
            return signature;
        }

        String identityKey() {
            return kind + "|" + file + "|" + owner + "|" + name;
        }
    }
}
