package com.stasislang.workshop;

import android.app.Activity;
import android.Manifest;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.res.AssetManager;
import android.content.pm.PackageManager;
import android.graphics.Color;
import android.opengl.GLES20;
import android.opengl.GLSurfaceView;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
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
import android.widget.ArrayAdapter;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.Spinner;
import android.widget.TextView;

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
import java.net.HttpURLConnection;
import java.net.URL;
import java.net.URLEncoder;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.ArrayList;
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
    private static final String ASSET_ROOT = "workshop_sample/";
    private static final String PROJECT_DIR = WorkshopProjectRegistry.LEGACY_PROJECT_DIR;
    private static final String AI_PREFS = "ai_settings";
    private static final String AI_PREF_API_KEY = "openai_api_key";
    private static final String AI_PREF_MODEL = "openai_model";
    private static final String AI_PREF_LAST_USAGE = "last_ai_usage";
    private static final String AI_PREF_COMMAND_HISTORY_PREFIX = "command_history_";
    private static final String AI_PREF_OUTCOME_HISTORY_PREFIX = "outcome_history_";
    private static final String AI_PREF_MAX_RUN_USD = "max_run_usd";
    private static final String AI_PREF_MONTHLY_LIMIT_USD = "monthly_limit_usd";
    private static final String AI_PREF_MONTH_KEY = "monthly_spend_month";
    private static final String AI_PREF_MONTH_SPEND_USD = "monthly_spend_usd";
    private static final String GITHUB_PREFS = "github_sync_settings";
    private static final String GITHUB_PREF_TOKEN = "github_token";
    private static final String GITHUB_PREF_REPOSITORY = "github_repository";
    private static final String GITHUB_PREF_BRANCH = "github_branch";
    private static final String GITHUB_PREF_OPERATION = "github_pending_operation";
    private static final String GITHUB_PREF_OPERATION_STATE = "github_operation_state";
    private static final String GITHUB_PREF_OPERATION_DETAIL = "github_operation_detail";
    private static final String GITHUB_PREF_REVIEW_FINGERPRINT = "github_review_fingerprint";
    private static final String AI_TRACE_LOG = "ai_trace.jsonl";
    private static final String DEFAULT_AI_MODEL = "gpt-5.6-terra";
    private static final String AI_PROMPT_CACHE_KEY = "stasis-android-workshop-v2";
    private static final long AI_TRACE_RETENTION_MS = 24L * 60L * 60L * 1000L;
    private static final long DEFAULT_TICK_INTERVAL_MS = 16L;
    private static final long DEBUG_UPDATE_INTERVAL_NANOS = 250_000_000L;
    private static final double FRAME_BUDGET_MILLIS = 1000.0 / 60.0;
    private static final int MAX_RENDER_COMMANDS = 8;
    private static final int MAX_AI_AGENT_TURNS = 15;
    private static final int MAX_AI_OUTPUT_TOKENS = 8192;
    private static final int MAX_COMMAND_HISTORY = 20;
    private static final int GITHUB_NETWORK_TIMEOUT_MS = 15_000;
    private static final int AI_CONNECT_TIMEOUT_MS = 15_000;
    private static final int AI_READ_TIMEOUT_MS = 120_000;
    private static final int VOICE_RECORD_PERMISSION_REQUEST = 41;
    private static final int EXPORT_PROJECT_REQUEST = 71;
    private static final int IMPORT_PROJECT_REQUEST = 72;
    private static final double GPT_5_6_TERRA_INPUT_USD_PER_MILLION = 2.50;
    private static final double GPT_5_6_TERRA_CACHED_INPUT_USD_PER_MILLION = 0.25;
    private static final double GPT_5_6_TERRA_CACHE_WRITE_USD_PER_MILLION = 3.125;
    private static final double GPT_5_6_TERRA_OUTPUT_USD_PER_MILLION = 15.00;
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
    private static final String[] SAMPLE_FILES = new String[] {
            "src/main.stasis",
            "src/root.stasis",
            "src/game_state.stasis",
            "src/player.stasis",
            "src/enemy.stasis",
            "src/input.stasis",
            "src/assets.stasis",
            "src/systems/collision.stasis"
    };
    private static final String[] SAMPLE_TEST_FILES = new String[] {
            "tests/enemy_paddle_speed_schedule.test.stasis"
    };

    private TextView sourceTitle;
    private LinearLayout selectedSourcePanel;
    private LinearLayout manualEditBody;
    private EditText sourceEditor;
    private EditText aiPromptEditor;
    private EditText aiApiKeyEditor;
    private EditText aiModelEditor;
    private EditText aiMaxRunUsdEditor;
    private EditText aiMonthlyLimitUsdEditor;
    private TextView aiBudgetStatus;
    private TextView aiStepPill;
    private TextView aiActionPill;
    private TextView aiPhasePill;
    private TextView aiElapsedPill;
    private LinearLayout aiSettingsBody;
    private LinearLayout commandHistoryBody;
    private TextView commandHistoryText;
    private LinearLayout githubSettingsBody;
    private EditText githubTokenEditor;
    private EditText githubRepositoryEditor;
    private EditText githubBranchEditor;
    private TextView githubSyncStatus;
    private LinearLayout projectSettingsBody;
    private EditText newProjectNameEditor;
    private Spinner projectSelector;
    private TextView projectStatus;
    private final ArrayList<WorkshopProjectRegistry.ProjectInfo> availableProjects = new ArrayList<>();
    private WorkshopProjectRegistry.ProjectInfo activeProject;
    private WorkshopProjectRegistry.ProjectInfo pendingExportProject;
    private String pendingImportProjectName = "";
    private String projectRegistryError = "";
    private String reviewedGitHubChangeFingerprint = "";
    private String credentialStorageError = "";
    private volatile boolean githubOperationActive;
    private volatile boolean projectIoActive;
    private volatile boolean aiRunActive;
    private volatile boolean aiCancelRequested;
    private volatile HttpURLConnection activeAiConnection;
    private String activeAiPrompt = "";
    private TextView reloadStatus;
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

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

        try {
            activeProject = WorkshopProjectRegistry.initialize(this);
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
        setContentView(createWorkshopView(project));
    }

    @Override
    protected void onDestroy() {
        stopVoiceRecognition();
        aiCancelRequested = true;
        HttpURLConnection aiConnection = activeAiConnection;
        if (aiConnection != null) aiConnection.disconnect();
        githubSyncExecutor.shutdownNow();
        projectIoExecutor.shutdownNow();
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
        }
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
        projectIoActive = true;
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
                    projectIoActive = false;
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
        projectIoActive = true;
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
                    projectIoActive = false;
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
                    projectIoActive = false;
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
        content.addView(reloadStatus, fullWidth());

        changeSummary = new TextView(this);
        changeSummary.setTextColor(Color.rgb(73, 84, 100));
        changeSummary.setTextSize(12.0f);
        changeSummary.setTypeface(Typeface.MONOSPACE);
        changeSummary.setPadding(0, dp(6), 0, dp(6));
        content.addView(changeSummary, fullWidth());
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
        editorToggle.setBackground(createPanelBackground(Color.rgb(35, 45, 60), Color.rgb(83, 96, 115)));
        editorToggle.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                toggleEditorPanel();
            }
        });
        FrameLayout.LayoutParams toggleParams = new FrameLayout.LayoutParams(dp(52), dp(48), Gravity.TOP | Gravity.END);
        toggleParams.setMargins(0, dp(8), dp(10), 0);
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
        voiceToggle.setTextColor(Color.WHITE);
        voiceToggle.setBackground(createPanelBackground(Color.rgb(35, 45, 60), Color.rgb(83, 96, 115)));
        voiceToggle.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                startVoiceChange();
            }
        });
        FrameLayout.LayoutParams voiceParams = new FrameLayout.LayoutParams(dp(74), dp(48), Gravity.TOP | Gravity.END);
        voiceParams.setMargins(0, dp(8), dp(68), 0);
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
        actionParams.setMargins(dp(8), dp(58), dp(8), 0);
        root.addView(voiceActionRow, actionParams);
    }

    private void startVoiceChange() {
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
        setStatusText("Voice change confirmed: starting AI run");
        runAiPatch();
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
            editorToggle.bringToFront();
        }
        if (voiceActionRow != null && voiceActionRow.getVisibility() == View.VISIBLE) {
            voiceActionRow.bringToFront();
        }
        if (voiceToggle != null) {
            voiceToggle.bringToFront();
        }
    }

    private void startGameLoop() {
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
            reloadStatus.setText(status);
        }
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
        gameStatus.setTextColor(debugColorForBudget(budgetPercent));
        gameStatus.setText(debugTextBuilder.toString());
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

        TextView aiTitle = new TextView(this);
        aiTitle.setText("Chat and Commands");
        aiTitle.setTextColor(Color.rgb(35, 45, 60));
        aiTitle.setTextSize(14.0f);
        aiTitle.setTypeface(Typeface.DEFAULT_BOLD);
        controls.addView(aiTitle, fullWidth());

        SharedPreferences aiPrefs = getSharedPreferences(AI_PREFS, MODE_PRIVATE);

        aiPromptEditor = new EditText(this);
        aiPromptEditor.setHint("Describe a game change or command. The workspace will inspect, edit, compile, and test it.");
        aiPromptEditor.setSingleLine(false);
        aiPromptEditor.setMinLines(2);
        aiPromptEditor.setTextSize(12.0f);
        controls.addView(aiPromptEditor, fullWidth());

        LinearLayout aiActionRow = new LinearLayout(this);
        aiActionRow.setOrientation(LinearLayout.HORIZONTAL);
        Button aiPatch = new Button(this);
        aiPatch.setText("Run AI Change");
        aiPatch.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                runAiPatch();
            }
        });
        aiActionRow.addView(aiPatch, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1.0f));
        Button cancelAi = new Button(this);
        cancelAi.setText("Cancel AI");
        cancelAi.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { cancelAiRun(); }
        });
        aiActionRow.addView(cancelAi, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1.0f));
        controls.addView(aiActionRow, fullWidth());

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
        controls.addView(progressRow, fullWidth());

        aiBudgetStatus = new TextView(this);
        aiBudgetStatus.setTextSize(12.0f);
        aiBudgetStatus.setTextColor(Color.rgb(73, 84, 100));
        aiBudgetStatus.setPadding(0, dp(4), 0, dp(2));
        controls.addView(aiBudgetStatus, fullWidth());
        refreshAiBudgetStatus();

        Button historyToggle = new Button(this);
        historyToggle.setText("Recent Commands");
        historyToggle.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { toggleCommandHistory(); }
        });
        controls.addView(historyToggle, fullWidth());
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
        controls.addView(commandHistoryBody, fullWidth());
        refreshCommandHistory();

        Button projectSettingsToggle = new Button(this);
        projectSettingsToggle.setText("Projects");
        projectSettingsToggle.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View view) { toggleProjectSettings(); }
        });
        controls.addView(projectSettingsToggle, fullWidth());
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
        Button newSampleProject = new Button(this);
        newSampleProject.setText("New Project From Sample");
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
        controls.addView(projectSettingsBody, fullWidth());
        refreshProjectControls();

        githubSyncStatus = new TextView(this);
        githubSyncStatus.setTextSize(12.0f);
        githubSyncStatus.setTextColor(Color.rgb(73, 84, 100));
        githubSyncStatus.setPadding(0, dp(4), 0, dp(2));
        controls.addView(githubSyncStatus, fullWidth());
        refreshGitHubSyncStatus();

        Button settingsToggle = new Button(this);
        settingsToggle.setText("AI Settings");
        settingsToggle.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                toggleAiSettings();
            }
        });
        controls.addView(settingsToggle, fullWidth());

        aiSettingsBody = new LinearLayout(this);
        aiSettingsBody.setOrientation(LinearLayout.VERTICAL);
        aiSettingsBody.setVisibility(View.GONE);
        aiApiKeyEditor = new EditText(this);
        aiApiKeyEditor.setHint("OpenAI API key");
        aiApiKeyEditor.setSingleLine(true);
        aiApiKeyEditor.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_PASSWORD);
        aiApiKeyEditor.setText(readSecretPreference(aiPrefs, AI_PREF_API_KEY));
        aiApiKeyEditor.setTextSize(12.0f);
        aiSettingsBody.addView(aiApiKeyEditor, fullWidth());

        aiModelEditor = new EditText(this);
        aiModelEditor.setHint("Model");
        aiModelEditor.setSingleLine(true);
        aiModelEditor.setText(aiPrefs.getString(AI_PREF_MODEL, DEFAULT_AI_MODEL));
        aiModelEditor.setTextSize(12.0f);
        aiSettingsBody.addView(aiModelEditor, fullWidth());

        aiMaxRunUsdEditor = new EditText(this);
        aiMaxRunUsdEditor.setHint("Maximum USD per AI run");
        aiMaxRunUsdEditor.setSingleLine(true);
        aiMaxRunUsdEditor.setText(aiPrefs.getString(AI_PREF_MAX_RUN_USD, "0.25"));
        aiSettingsBody.addView(aiMaxRunUsdEditor, fullWidth());

        aiMonthlyLimitUsdEditor = new EditText(this);
        aiMonthlyLimitUsdEditor.setHint("Monthly AI limit USD");
        aiMonthlyLimitUsdEditor.setSingleLine(true);
        aiMonthlyLimitUsdEditor.setText(aiPrefs.getString(AI_PREF_MONTHLY_LIMIT_USD, "5.00"));
        aiSettingsBody.addView(aiMonthlyLimitUsdEditor, fullWidth());

        Button saveSettings = new Button(this);
        saveSettings.setText("Save AI Settings");
        saveSettings.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                saveAiSettingsFromEditors();
            }
        });
        aiSettingsBody.addView(saveSettings, fullWidth());
        controls.addView(aiSettingsBody, fullWidth());

        Button githubSettingsToggle = new Button(this);
        githubSettingsToggle.setText("GitHub Sync Settings");
        githubSettingsToggle.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                toggleGitHubSettings();
            }
        });
        controls.addView(githubSettingsToggle, fullWidth());

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
        controls.addView(githubSettingsBody, fullWidth());
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
        } catch (Exception ignored) {
            // Outcome history must not interfere with AI execution or source recovery.
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
                    : "Active project: " + activeProject.name + " (format " + WorkshopProjectRegistry.FORMAT_VERSION + ")");
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
        if (aiRunActive || githubOperationActive || projectIoActive
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
            WorkshopProjectRegistry.ProjectInfo project = WorkshopProjectRegistry.createFromSample(this, name);
            activateProject(project);
            newProjectNameEditor.setText("");
        } catch (Exception error) {
            setStatusText("Project creation failed: " + error.getMessage());
        }
    }

    private boolean activateProject(WorkshopProjectRegistry.ProjectInfo project) {
        if (aiRunActive || githubOperationActive || projectIoActive
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
            selectedSymbol = null;
            compileAttempted = false;
            compileReady = false;
            lastCompileResult = "CompileNotRun";
            reviewedGitHubChangeFingerprint = "";
            ProjectSnapshot snapshot = loadBundledProject();
            rebuildSymbolList(snapshot);
            if (snapshot.firstSymbol != null) showSymbol(snapshot.firstSymbol);
            refreshChangeSummary(snapshot);
            refreshCommandHistory();
            refreshGitHubSettingsEditors();
            refreshGitHubSyncStatus();
            refreshProjectControls();
            String compileResult = nativeCompileProject(projectRootPath());
            lastCompileResult = compileResult;
            compileReady = isRunnableCompile(compileResult);
            compileAttempted = true;
            setStatusText("Switched to " + project.name + " - " + compileResult);
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
        if (aiRunActive || githubOperationActive || projectIoActive
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
        if (aiRunActive || githubOperationActive || projectIoActive
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
        final SharedPreferences prefs = getSharedPreferences(GITHUB_PREFS, MODE_PRIVATE);
        final String token = readSecretPreference(prefs, GITHUB_PREF_TOKEN).trim();
        final String repository = readGitHubProjectPreference(prefs, GITHUB_PREF_REPOSITORY, "").trim();
        final String branch = readGitHubProjectPreference(prefs, GITHUB_PREF_BRANCH, "main").trim();
        if (token.isEmpty() || repository.indexOf('/') <= 0) {
            setStatusText("GitHub sync needs configured settings");
            return;
        }
        final Map<String, String> files;
        try {
            files = changedProjectSources(loadBundledAssetSnapshot(), loadBundledProject());
        } catch (IOException error) {
            githubSyncStatus.setText("GitHub sync error: unable to read local baseline");
            return;
        }
        if (files.isEmpty()) {
            githubSyncStatus.setText("GitHub sync: no local changes");
            return;
        }
        if (!beginGitHubOperation("sync", "GitHub sync: queued (" + files.size() + " files)")) return;
        githubSyncExecutor.submit(new Runnable() {
            @Override public void run() {
                try {
                    int completed = 0;
                    for (Map.Entry<String, String> entry : files.entrySet()) {
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
            ProjectSnapshot baseline = loadBundledAssetSnapshot();
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
            changes = changedProjectSources(loadBundledAssetSnapshot(), loadBundledProject());
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

    private synchronized boolean beginGitHubOperation(String operation, String status) {
        if (githubOperationActive) {
            githubSyncStatus.setText("GitHub sync: another operation is already queued or running");
            return false;
        }
        githubOperationActive = true;
        postGitHubOperationState(operation, "queued", status);
        return true;
    }

    private void postGitHubOperationState(final String operation, final String state, final String status) {
        persistGitHubOperationState(operation, state, status);
        if ("complete".equals(state) || "error".equals(state)) githubOperationActive = false;
        runOnUiThread(new Runnable() {
            @Override public void run() { if (githubSyncStatus != null) githubSyncStatus.setText(status); }
        });
    }

    private static void uploadGitHubFile(String token, String repository, String branch, String path, String source) throws Exception {
        String base = githubApiUrl(repository, "/contents/" + encodeGitHubPath(path));
        HttpURLConnection get = (HttpURLConnection)new URL(base + "?ref=" + encodeGitHubQuery(branch)).openConnection();
        configureGitHubConnection(get, token);
        String sha = "";
        int getCode = get.getResponseCode();
        if (getCode == 200) sha = new JSONObject(readStreamStatic(get.getInputStream())).optString("sha", "");
        else if (getCode != 404) throw new IOException("read " + path + " HTTP " + getCode);
        JSONObject body = new JSONObject().put("message", "stasis workshop sync: " + path)
                .put("content", Base64.encodeToString(source.getBytes(StandardCharsets.UTF_8), Base64.NO_WRAP))
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

    private void saveAiSettingsFromEditors() {
        String apiKey = aiApiKeyEditor == null ? "" : aiApiKeyEditor.getText().toString().trim();
        String model = aiModelEditor == null ? "" : aiModelEditor.getText().toString().trim();
        String maxRunText = aiMaxRunUsdEditor == null ? "0.25" : aiMaxRunUsdEditor.getText().toString().trim();
        String monthlyLimitText = aiMonthlyLimitUsdEditor == null ? "5.00" : aiMonthlyLimitUsdEditor.getText().toString().trim();
        if (apiKey.isEmpty()) {
            setStatusText("AI settings need an API key before a run can start");
            return;
        }
        if (parseNonNegativeUsd(maxRunText) < 0.0 || parseNonNegativeUsd(monthlyLimitText) < 0.0) {
            setStatusText("AI budget limits must be non-negative USD values");
            return;
        }
        if (!saveAiSettings(apiKey, model.isEmpty() ? DEFAULT_AI_MODEL : model)) return;
        getSharedPreferences(AI_PREFS, MODE_PRIVATE).edit()
                .putString(AI_PREF_MAX_RUN_USD, maxRunText)
                .putString(AI_PREF_MONTHLY_LIMIT_USD, monthlyLimitText)
                .apply();
        refreshAiBudgetStatus();
        setStatusText("AI settings saved");
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
        double monthlyLimit = configuredAiLimit(AI_PREF_MONTHLY_LIMIT_USD, "5.00");
        double spent = monthlyAiSpendUsd();
        aiBudgetStatus.setText("AI budget: " + formatAiCostUsd(spent) + " / " + formatAiCostUsd(monthlyLimit) + " this month");
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
        if (aiRunActive) {
            setStatusText("AI run already active; cancel it before starting another");
            return;
        }
        String apiKey = aiApiKeyEditor == null ? "" : aiApiKeyEditor.getText().toString().trim();
        String prompt = aiPromptEditor == null ? "" : aiPromptEditor.getText().toString().trim();
        String model = aiModelEditor == null ? "" : aiModelEditor.getText().toString().trim();
        if (apiKey.isEmpty() || prompt.isEmpty()) {
            setStatusText("AI run needs both a request and an API key; open AI Settings if the key is not saved");
            updateAiProgress(0, 0, "needs input");
            return;
        }
        if (model.isEmpty()) {
            model = DEFAULT_AI_MODEL;
        }
        double maxRunUsd = configuredAiLimit(AI_PREF_MAX_RUN_USD, "0.25");
        double monthlyLimitUsd = configuredAiLimit(AI_PREF_MONTHLY_LIMIT_USD, "5.00");
        if (!hasKnownAiPricing(model)) {
            setStatusText("AI run blocked: pricing is unavailable for " + model);
            updateAiProgress(0, 0, "budget blocked");
            return;
        }
        if (maxRunUsd <= 0.0 || monthlyLimitUsd <= 0.0 || monthlyAiSpendUsd() >= monthlyLimitUsd) {
            setStatusText("AI run blocked by configured spending limit; open AI Settings");
            updateAiProgress(0, 0, "budget blocked");
            return;
        }
        recordCommandHistory(prompt);
        if (!saveAiSettings(apiKey, model)) return;
        final SymbolEntry symbol = selectedSymbol;
        final String selectedSource = symbol == null || sourceEditor == null ? "" : sourceEditor.getText().toString().trim();
        final ProjectSnapshot aiProject = loadBundledProject();
        final String requestJson = buildAiCodeRequestJson(prompt, symbol, selectedSource, aiProject);
        final String requestModel = model;
        final String requestApiKey = apiKey;
        activeAiPrompt = prompt;
        aiCancelRequested = false;
        aiRunActive = true;
        recordAiOutcome(activeAiPrompt, "started", "AI run started", "");
        aiStartedAtNanos = System.nanoTime();
        appendAiTraceFields("request", "model", requestModel, "request_json", requestJson, null, null);
        setStatusText("AI run started: preparing workspace and command context");
        updateAiProgress(0, 0, "preparing");
        new Thread(new Runnable() {
            @Override
            public void run() {
                try {
                    final AiAgentResult aiResult = runAiAgentLoop(requestApiKey, requestModel, requestJson);
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
                    aiRunActive = false;
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
        preferences.edit().putString(AI_PREF_MODEL, model).apply();
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

    private void runNativeTests() {
        try {
            JSONObject result = aiToolRunTests(new AiAgentSession());
            setStatusText(testSummaryText(result));
        } catch (Exception error) {
            setStatusText("Tests failed: " + error.getMessage());
        }
    }

    private static String buildAiCodeRequestJson(String prompt, SymbolEntry symbol, String selectedSource, ProjectSnapshot project) {
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
                    .put("Before writing, inspect the full feature path: state definition, creation/reset, tick update, render output, and test/input path.")
                    .put("Keep mobile input abstracted through Stasis Input globals and helper functions so logic can move across platforms.")
                    .put("Add or use testable invariants by setting input/state, running ticks, and checking state or render output.")
                    .put("Avoid broad rewrites; make the smallest structural change that gives the feature a clear owner.")
                    .put("Prefer data-oriented clarity over deep abstractions: arrays, IDs, counters, and explicit update loops.")
                    .put("Avoid per-tick allocation/object churn and keep new systems within the visible 60 fps budget.");

            JSONObject request = new JSONObject();
            request.put("cache_layout", "Stable request context is first. Volatile tool observations are sent after the prompt cache breakpoint.");
            request.put("scope", "entire_workspace");
            request.put("response_contract", aiResponseContract());
            request.put("available_tools", supportedAiTools());
            request.put("tool_specs", aiToolSpecs());
            request.put("stasis_style_rules", rules);
            request.put("game_design_rules", gameRules);
            request.put("architecture_recommendations", architectureRecommendations);
            request.put("project_globals", aiProjectGlobals(project));
            request.put("user_prompt", prompt);
            request.put("selected_symbols", selectedSymbols);
            request.put("selected_symbols_are_context_only", true);
            return request.toString();
        } catch (Exception error) {
            return "{}";
        }
    }

    private static JSONObject aiResponseContract() throws Exception {
        JSONArray acceptedShapes = new JSONArray()
                .put(new JSONObject()
                        .put("mode", "tool_calls")
                        .put("summary", "short optional status")
                        .put("tool_calls", new JSONArray().put(new JSONObject()
                                .put("tool", "read_symbol")
                                .put("args", new JSONObject().put("name", "tick")))))
                .put(new JSONObject()
                        .put("mode", "done")
                        .put("summary", "what was verified"))
                .put(new JSONObject()
                        .put("mode", "edits")
                        .put("summary", "short change summary")
                        .put("edits", new JSONArray().put(new JSONObject()
                                .put("kind", "replace_function")
                                .put("owner", "Player")
                                .put("name", "jump")
                                .put("file", "src/player.stasis")
                                .put("new_source", "function jump(self: Player): void {\n}"))));
        return new JSONObject()
                .put("required", "Return exactly one JSON object. The top-level object must match one of the accepted_response_shapes.")
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
                ? unsupportedJsonKeys(response, "mode", "summary", "tool_calls")
                : ("done".equals(mode)
                        ? unsupportedJsonKeys(response, "mode", "summary")
                        : unsupportedJsonKeys(response, "mode", "summary", "edits"));
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
    private AiAgentResult runAiAgentLoop(String apiKey, String model, String initialRequestJson) throws Exception {
        String currentRequestJson = initialRequestJson;
        AiAgentSession session = new AiAgentSession();
        AiUsageAccumulator usage = new AiUsageAccumulator();
        String previousToolCallBatch = "";
        for (int turn = 0; turn < MAX_AI_AGENT_TURNS; turn += 1) {
            throwIfAiCancelled();
            double maxRunUsd = configuredAiLimit(AI_PREF_MAX_RUN_USD, "0.25");
            double monthlyLimitUsd = configuredAiLimit(AI_PREF_MONTHLY_LIMIT_USD, "5.00");
            if (usage.estimatedCostUsd >= maxRunUsd || monthlyAiSpendUsd() >= monthlyLimitUsd) {
                throw new IOException("AI spending limit reached before agent turn " + (turn + 1));
            }
            session.currentStep = turn + 1;
            postAiProgress(session.currentStep, session.actionCount, "calling AI");
            appendAiTrace("llm_request", new JSONObject()
                    .put("turn", session.currentStep)
                    .put("model", model)
                    .put("summary", summarizeAiRequestForTrace(currentRequestJson)));
            double remainingUsd = Math.min(maxRunUsd - usage.estimatedCostUsd, monthlyLimitUsd - monthlyAiSpendUsd());
            int maxOutputTokens = maxOutputTokensForBudget(currentRequestJson, remainingUsd);
            AiApiResponse apiResponse = callOpenAiResponsesApi(apiKey, model, currentRequestJson, maxOutputTokens);
            usage.add(model, apiResponse.usage);
            if (usage.lastCallCostAvailable) recordMonthlyAiSpend(usage.lastCallEstimatedCostUsd);
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
                followup.put("instruction", "Your previous JSON response shape was invalid. Return exactly one JSON object matching the stable request response_contract. For tool use, use mode=tool_calls and a top-level tool_calls array. Each call must be {\"tool\":\"name\",\"args\":{...}} with no aliases such as calls, name, function, arguments, type, or source.");
                currentRequestJson = followup.toString();
                continue;
            }
            String mode = response.getString("mode");
            JSONArray toolCalls = response.optJSONArray("tool_calls");
            if (!"tool_calls".equals(mode) || toolCalls == null || toolCalls.length() == 0) {
                postAiProgress(session.currentStep, session.actionCount, "finalizing");
                return new AiAgentResult(aiJson, usage.toJson(model), usage.summary(), session.currentStep, session.actionCount);
            }
            String currentToolCallBatch = toolCalls.toString();
            if (currentToolCallBatch.equals(previousToolCallBatch)) {
                postAiProgress(session.currentStep, session.actionCount, "repeated tools");
                JSONObject repeated = new JSONObject()
                        .put("mode", "done")
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
                    return new AiAgentResult(repeated.toString(), usage.toJson(model), usage.summary(), session.currentStep, session.actionCount);
                }
                throw new IOException("AI repeated identical tool calls; actions=" + session.actionCount + " successful_writes=" + session.successfulWriteCount + " rolled_back_writes=" + session.rolledBackWriteCount + " last_tool=" + session.lastToolSummary + " last_error=" + session.lastToolError);
            }
            previousToolCallBatch = currentToolCallBatch;
            postAiProgress(session.currentStep, session.actionCount, "tools " + toolCalls.length());
            appendAiTrace("tool_calls", new JSONObject().put("turn", session.currentStep).put("tool_calls", toolCalls));
            JSONArray observations = executeAiToolCalls(toolCalls, session);
            throwIfAiCancelled();
            appendAiTrace("tool_observations", new JSONObject().put("turn", session.currentStep).put("observations", observations));
            JSONObject testObservation = runAiTestsAfterBatch(session);
            session.latestTestObservation = testObservation;
            appendAiTrace("test_observation", new JSONObject().put("turn", session.currentStep).put("result", testObservation));
            JSONObject followup = new JSONObject();
            followup.put("original_request", new JSONObject(initialRequestJson));
            followup.put("tool_observations", observations);
            followup.put("test_observation", testObservation);
            followup.put("tool_specs", aiToolSpecs());
            followup.put("instruction", "Use the tool observations to either request more tools or return final edits. Inspect current symbols/imports/tests before writing unless the exact current source is already available. Do not use read_file; use read_symbol/read_imports/read_test_file. Apply code changes with write_symbol, delete_symbol, write_imports, write_test_file, or delete_test_file before final edits so compile failures and test_observation results return observations you can correct. Tool errors, validation_error observations, and test_observation failures are not final; use accepted_shape, required_args, response_contract, and the error observation to choose another tool call or corrected write. Return mode=edits only after the intended code has been written, compiled, and the latest test_observation has passed runnable tests. If no further action is needed, return mode=done.");
            currentRequestJson = followup.toString();
        }
        postAiProgress(MAX_AI_AGENT_TURNS, session.actionCount, "limit hit");
        if (session.successfulWriteCount > 0 && compileReady && session.latestRunnableTestsPassed()) {
            String summary = "Applied " + session.successfulWriteCount + " tool write(s) before response limit";
            JSONObject synthetic = new JSONObject()
                    .put("mode", "done")
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
            return new AiAgentResult(synthetic.toString(), usage.toJson(model), usage.summary(), MAX_AI_AGENT_TURNS, session.actionCount);
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
                        entry.put("preferred_call", preferredFunctionCall(symbol));
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
        int width = gamePreview == null ? 0 : gamePreview.getWidth();
        int height = gamePreview == null ? 0 : gamePreview.getHeight();
        JSONArray frame = new JSONArray();
        for (int index = 0; index < nativeFrameValues.length; index += 1) {
            frame.put(nativeFrameValues[index]);
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
                .put("frame", frameValuesToJson(nativeFrameValues))
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

    private static String preferredFunctionCall(SymbolEntry symbol) {
        if (!"function".equals(symbol.kind)) {
            return symbol.name;
        }
        String first = firstParameter(symbol.signature);
        if (first != null) {
            String[] parts = first.split(":", 2);
            if (parts.length == 2 && "self".equals(parts[0].trim()) && parts[1].trim().equals(symbol.owner)) {
                return lowerFirst(symbol.owner) + "." + symbol.name + "(" + callArgumentList(symbol.signature, true) + ")";
            }
        }
        return symbol.name + "(" + callArgumentList(symbol.signature, false) + ")";
    }

    private static String callArgumentList(String signature, boolean skipFirst) {
        int open = signature.indexOf('(');
        int close = signature.indexOf(')', open + 1);
        if (open < 0 || close < 0 || close <= open + 1) {
            return "";
        }
        String parameters = signature.substring(open + 1, close).trim();
        if (parameters.isEmpty()) {
            return "";
        }
        String[] parts = parameters.split(",");
        StringBuilder builder = new StringBuilder();
        for (int index = skipFirst ? 1 : 0; index < parts.length; index += 1) {
            String parameter = parts[index].trim();
            int colon = parameter.indexOf(':');
            String name = colon > 0 ? parameter.substring(0, colon).trim() : parameter;
            if (name.isEmpty()) {
                continue;
            }
            if (builder.length() > 0) {
                builder.append(", ");
            }
            builder.append(name);
        }
        return builder.toString();
    }

    private static String lowerFirst(String text) {
        if (text == null || text.isEmpty()) {
            return "value";
        }
        return Character.toLowerCase(text.charAt(0)) + text.substring(1);
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

    private static JSONArray buildAiOpenAiInput(String requestJson) throws Exception {
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
        String stableInstruction = "Return only one JSON object. You may inspect and edit any Stasis symbol in the workspace; selected_symbols are optional context only. You may use mode=tool_calls with tool_calls to inspect or write the Stasis workspace using only these tools: list_symbols, list_owner_symbols, read_symbol, read_imports, write_imports, write_symbol, delete_symbol, list_tests, read_test_file, write_test_file, delete_test_file, run_tests, get_diagnostics, set_input_state, run_frame, inspect_runtime_state, take_screenshot. take_screenshot returns a compact logical render snapshot with decoded commands, runtime state, and input. set_input_state controls simulated test input; run_frame advances one frame and returns runtime/render state. Before writing, inspect the current target with list_symbols, list_owner_symbols, read_symbol, read_imports, list_tests, and read_test_file unless the exact current source was already provided in selected_symbols or tool observations. Do not use read_file; the workshop edits symbols, imports, and tests rather than whole source files. For behavior-changing requests, add or update a tests/*.test.stasis test before returning done. A valid test uses test `name`(): bool and returns true or false; do not create .ai_test.json files or use assert_runtime helpers, which are not Stasis syntax. run_tests executes the native bridge tests on the Android device. Apply code changes with write_symbol, delete_symbol, write_imports, write_test_file, or delete_test_file before final edits so failed writes and automatic compile/test_observation results return observations you can correct. The app compiles once after each tool-call batch that contains writes and runs tests after each tool-call batch; use write_test_file/run_tests or take_screenshot for validation instead of direct runtime pokes. Use on_code_swap() for post-hot-swap migration, reinitialization, or compatibility work when a running game needs state adjusted after code changes. Use tool_specs in the request for required_args, optional_args, and examples. Each tool call must use {\"tool\":\"name\",\"args\":{...}}; include only args relevant to that tool. Return mode=edits with replace_function/replace_struct edits only after write_symbol/delete_symbol/write_imports has successfully written, compiled, and the latest test_observation has passed runnable tests, including any new or updated behavior test for the request. If the requested work is already complete or no code changes are needed, return mode=done with a summary only. A replace_function edit for a missing function in an existing file is treated as an added helper. Do not use markdown.";
        stableInstruction += " write_symbol creates or replaces a symbol. Before writing, inspect the current target. Follow game_design_rules, prefer_lifecycle_local_state, avoid_global_tick_for_per_entity_progression, and architecture_recommendations. Follow architecture_recommendations. Use command/event-style functions for durable gameplay concepts. Tool errors, validation_error observations, and test_observation failures are not final; correct them before returning mode=done. A failed write batch rolls back the whole batch and returns diagnostics.";
        return new JSONArray()
                .put(aiInputMessage("system", stableInstruction, false))
                .put(aiInputMessage("user", "Stable request context: " + stableRequest.toString(), true))
                .put(aiInputMessage("user", "Volatile turn context: " + volatileRequest.toString(), false));
    }

    private static JSONObject aiInputMessage(String role, String text, boolean cacheBreakpoint) throws Exception {
        JSONObject content = new JSONObject().put("type", "input_text").put("text", text);
        if (cacheBreakpoint) {
            content.put("prompt_cache_breakpoint", new JSONObject().put("mode", "explicit"));
        }
        return new JSONObject().put("role", role).put("content", new JSONArray().put(content));
    }

    private static int maxOutputTokensForBudget(String requestJson, double remainingUsd) throws Exception {
        byte[] inputBytes = buildAiOpenAiInput(requestJson).toString().getBytes(StandardCharsets.UTF_8);
        double inputRate = Math.max(GPT_5_6_TERRA_INPUT_USD_PER_MILLION, GPT_5_6_TERRA_CACHE_WRITE_USD_PER_MILLION);
        double conservativeInputCost = inputBytes.length * inputRate / 1000000.0;
        double outputBudget = remainingUsd - conservativeInputCost;
        int outputTokens = (int)Math.floor(outputBudget * 1000000.0 / GPT_5_6_TERRA_OUTPUT_USD_PER_MILLION);
        if (outputTokens < 64) {
            throw new IOException("AI spending limit leaves insufficient budget for another response");
        }
        return Math.min(MAX_AI_OUTPUT_TOKENS, outputTokens);
    }

    private AiApiResponse callOpenAiResponsesApi(String apiKey, String model, String requestJson, int maxOutputTokens) throws Exception {
        JSONObject payload = new JSONObject();
        payload.put("model", model);
        payload.put("max_output_tokens", maxOutputTokens);
        payload.put("prompt_cache_key", AI_PROMPT_CACHE_KEY);
        payload.put("prompt_cache_options", new JSONObject().put("mode", "explicit").put("ttl", "30m"));
        payload.put("text", buildAiResponseTextFormat());
        payload.put("input", buildAiOpenAiInput(requestJson));
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
            return new AiApiResponse(response, extractAiUsage(response));
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
        responseProperties.put("summary", new JSONObject().put("type", "string"));
        responseProperties.put("tool_calls", new JSONObject().put("type", "array").put("items", toolSchema));
        responseProperties.put("edits", new JSONObject().put("type", "array").put("items", editSchema));

        JSONObject schema = new JSONObject();
        schema.put("type", "object");
        schema.put("additionalProperties", false);
        schema.put("required", new JSONArray()
                .put("mode"));
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
            throw new IOException("AI response did not include JSON edits");
        }
        return text.substring(start, end + 1);
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
        JSONArray selected = stable.optJSONArray("selected_symbols");
        JSONArray tools = stable.optJSONArray("available_tools");
        return new JSONObject()
                .put("followup", followup)
                .put("cache_key", AI_PROMPT_CACHE_KEY)
                .put("cache_breakpoint_after", "stable_request_context")
                .put("stable_keys", sortedJsonKeys(stable))
                .put("volatile_keys", sortedJsonKeys(volatileContext))
                .put("project_global_count", globals == null ? 0 : globals.length())
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
        return DEFAULT_AI_MODEL.equals(model);
    }

    private static double estimateAiCostUsd(String model, long inputTokens, long cachedInputTokens, long cacheWriteInputTokens, long outputTokens) {
        if (!hasKnownAiPricing(model)) {
            return 0.0;
        }
        double inputCost = Math.max(0L, inputTokens - cachedInputTokens - cacheWriteInputTokens) * GPT_5_6_TERRA_INPUT_USD_PER_MILLION;
        double cachedInputCost = cachedInputTokens * GPT_5_6_TERRA_CACHED_INPUT_USD_PER_MILLION;
        double cacheWriteCost = cacheWriteInputTokens * GPT_5_6_TERRA_CACHE_WRITE_USD_PER_MILLION;
        double outputCost = outputTokens * GPT_5_6_TERRA_OUTPUT_USD_PER_MILLION;
        return (inputCost + cachedInputCost + cacheWriteCost + outputCost) / 1000000.0;
    }

    private static String formatAiCostUsd(double costUsd) {
        long millionths = Math.round(costUsd * 1000000.0);
        String fraction = Long.toString(millionths % 1000000L);
        StringBuilder builder = new StringBuilder();
        builder.append('$').append(millionths / 1000000L).append('.');
        for (int index = fraction.length(); index < 6; index += 1) {
            builder.append('0');
        }
        builder.append(fraction);
        return builder.toString();
    }
    private void applyAiCodeResponse(AiAgentResult aiResult, SymbolEntry fallbackSymbol) {
        Map<String, String> originalSources = null;
        try {
            saveLastAiUsage(aiResult.usageJson);
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
            updateAiProgress(aiResult.finalStep, aiResult.finalActionCount, aiReloadPhase(compileResult));
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
        String reload = classifySelectedReload(editedSymbol, editedSource);
        try {
            persistSelectedEdit(editedSymbol, editedSource);
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
            setStatusText("Saved to .stasis file - " + reload + " - " + compileResult);
        } catch (IOException error) {
            setStatusText("Save failed: " + error.getMessage());
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
            ProjectSnapshot baseline = loadBundledAssetSnapshot();
            changeSummary.setText(formatChangeSummary(baseline, currentProject));
        } catch (IOException error) {
            changeSummary.setText("Changed symbols:\n  Unable to read bundled baseline: " + error.getMessage());
        }
    }

    private void showRawDiffReview() {
        if (changeSummary == null) {
            return;
        }
        try {
            changeSummary.setText(formatRawFileDiffs(loadBundledAssetSnapshot(), loadBundledProject()));
        } catch (IOException error) {
            changeSummary.setText("Raw file diffs:\n  Unable to read bundled baseline: " + error.getMessage());
        }
    }

    private ProjectSnapshot loadBundledAssetSnapshot() throws IOException {
        List<SourceFile> files = new ArrayList<>();
        AssetManager assets = getAssets();
        File projectRoot = projectRoot();
        for (String file : SAMPLE_FILES) {
            File diskFile = new File(projectRoot, file);
            try {
                files.add(new SourceFile(file, diskFile, readAsset(assets, ASSET_ROOT + file)));
            } catch (IOException error) {
                files.add(new SourceFile(file, diskFile, "// Unable to load " + file + ": " + error.getMessage()));
            }
        }
        for (String file : SAMPLE_TEST_FILES) {
            File diskFile = new File(projectRoot, file);
            try {
                files.add(new SourceFile(file, diskFile, readAsset(assets, ASSET_ROOT + file)));
            } catch (IOException error) {
                files.add(new SourceFile(file, diskFile, "// Unable to load " + file + ": " + error.getMessage()));
            }
        }

        return ProjectSnapshot.from(files);
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
        ProjectSnapshot project = loadBundledProject(true);
        rebuildSymbolList(project);
        if (project.firstSymbol != null) {
            showSymbol(project.firstSymbol);
        }
        refreshChangeSummary(project);
        compileReady = false;
        compileAttempted = false;
        setStatusText("Reset project from bundled sample");
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
            if (findMatchingSymbol(loadBundledAssetSnapshot(), selectedSymbol) != null) {
                setStatusText("Delete Test unavailable: bundled tests can be reverted, not deleted");
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
            if (findMatchingSymbol(loadBundledAssetSnapshot(), selectedSymbol) != null) {
                setStatusText("Delete Helper unavailable: bundled helpers can be reverted, not deleted");
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
        setStatusText("Reset editor to selected symbol");
    }

    private void revertSelectedToBundled() {
        if (selectedSymbol == null) {
            setStatusText("Revert unavailable: select a bundled symbol first");
            return;
        }
        try {
            SymbolEntry baseline = findMatchingSymbol(loadBundledAssetSnapshot(), selectedSymbol);
            if (baseline == null) {
                setStatusText("Revert unavailable: selected symbol is not bundled");
                return;
            }
            persistSelectedEdit(selectedSymbol, baseline.source);
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
            setStatusText("Reverted saved symbol to bundled baseline - " + compileResult);
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
            File[] children = file.listFiles();
            if (children != null) {
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
            File[] children = file.listFiles();
            if (children != null) {
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

        for (String file : SAMPLE_FILES) {
            File diskFile = new File(projectRoot, file);
            try {
                ensureProjectFile(assets, ASSET_ROOT + file, diskFile);
                files.add(new SourceFile(file, diskFile, readTextFile(diskFile)));
            } catch (IOException error) {
                files.add(new SourceFile(file, diskFile, "// Unable to load " + file + ": " + error.getMessage()));
            }
        }
        for (String file : SAMPLE_TEST_FILES) {
            File diskFile = new File(projectRoot, file);
            try {
                ensureProjectFile(assets, ASSET_ROOT + file, diskFile);
                files.add(new SourceFile(file, diskFile, readTextFile(diskFile)));
            } catch (IOException error) {
                files.add(new SourceFile(file, diskFile, "// Unable to load " + file + ": " + error.getMessage()));
            }
        }
        try {
            TreeSet<String> seen = new TreeSet<>();
            for (SourceFile file : files) {
                seen.add(file.path);
            }
            collectProjectStasisFiles(new File(projectRoot, "tests"), files, seen);
        } catch (IOException ignored) {
            // Extra user-authored test files should not prevent the source tree from loading.
        }

        return ProjectSnapshot.from(files);
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

        AiApiResponse(String body, JSONObject usage) {
            this.body = body;
            this.usage = usage;
        }
    }

    private static final class AiAgentResult {
        final String aiJson;
        final JSONObject usageJson;
        final String usageSummary;
        final int finalStep;
        final int finalActionCount;

        AiAgentResult(String aiJson, JSONObject usageJson, String usageSummary, int finalStep, int finalActionCount) {
            this.aiJson = aiJson;
            this.usageJson = usageJson;
            this.usageSummary = usageSummary;
            this.finalStep = finalStep;
            this.finalActionCount = finalActionCount;
        }
    }

    private static final class AiUsageAccumulator {
        private final JSONArray calls = new JSONArray();
        private long inputTokens;
        private long cachedInputTokens;
        private long cacheWriteInputTokens;
        private long outputTokens;
        private double estimatedCostUsd;
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
    }
    private final class AiAgentSession {
        int currentStep;
        int actionCount;
        int successfulWriteCount;
        int rolledBackWriteCount;
        String lastToolSummary = "none";
        String lastToolError = "";
        TreeSet<String> lastPassingTestKeys = new TreeSet<>();
        JSONObject latestTestObservation = new JSONObject();
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
            activity.recordRenderTimeNanos(System.nanoTime() - renderStartNanos);
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
