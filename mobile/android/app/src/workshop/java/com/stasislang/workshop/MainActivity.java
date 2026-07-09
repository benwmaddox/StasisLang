package com.stasislang.workshop;

import android.app.Activity;
import android.content.SharedPreferences;
import android.content.res.AssetManager;
import android.graphics.Color;
import android.opengl.GLES20;
import android.opengl.GLSurfaceView;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
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
import android.widget.Button;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.ScrollView;
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
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.TreeSet;

import org.json.JSONArray;
import org.json.JSONObject;

public final class MainActivity extends Activity {
    private static final String ASSET_ROOT = "workshop_sample/";
    private static final String PROJECT_DIR = "workshop_project";
    private static final String AI_PREFS = "ai_settings";
    private static final String AI_PREF_API_KEY = "openai_api_key";
    private static final String AI_PREF_MODEL = "openai_model";
    private static final String AI_PREF_LAST_USAGE = "last_ai_usage";
    private static final String AI_TRACE_LOG = "ai_trace.jsonl";
    private static final long AI_TRACE_RETENTION_MS = 24L * 60L * 60L * 1000L;
    private static final long DEFAULT_TICK_INTERVAL_MS = 16L;
    private static final long DEBUG_UPDATE_INTERVAL_NANOS = 250_000_000L;
    private static final double FRAME_BUDGET_MILLIS = 1000.0 / 60.0;
    private static final int MAX_RENDER_COMMANDS = 8;
    private static final int MAX_AI_AGENT_TURNS = 5;
    private static final double GPT_5_4_MINI_INPUT_USD_PER_MILLION = 0.75;
    private static final double GPT_5_4_MINI_CACHED_INPUT_USD_PER_MILLION = 0.075;
    private static final double GPT_5_4_MINI_OUTPUT_USD_PER_MILLION = 4.50;
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

    private TextView sourceTitle;
    private LinearLayout selectedSourcePanel;
    private LinearLayout manualEditBody;
    private EditText sourceEditor;
    private EditText aiPromptEditor;
    private EditText aiApiKeyEditor;
    private EditText aiModelEditor;
    private TextView aiStepPill;
    private TextView aiActionPill;
    private TextView aiPhasePill;
    private TextView aiElapsedPill;
    private TextView reloadStatus;
    private TextView changeSummary;
    private TextView gameStatus;
    private GamePreviewView gamePreview;
    private LinearLayout symbolList;
    private File projectRootFile;
    private String projectRootPath;
    private ScrollView editorPanel;
    private Button editorToggle;
    private final Handler gameLoopHandler = new Handler(Looper.getMainLooper());
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

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

        projectRootFile = new File(getFilesDir(), PROJECT_DIR);
        projectRootPath = projectRootFile.getAbsolutePath();

        Window window = getWindow();
        window.setStatusBarColor(Color.BLACK);
        window.setNavigationBarColor(Color.BLACK);

        ProjectSnapshot project = loadBundledProject();
        setContentView(createWorkshopView(project));
    }

    @Override
    protected void onDestroy() {
        if (gameLoop != null) {
            gameLoopHandler.removeCallbacks(gameLoop);
        }
        super.onDestroy();
    }

    private View createWorkshopView(ProjectSnapshot project) {
        FrameLayout root = new FrameLayout(this);
        root.setBackgroundColor(Color.rgb(15, 20, 28));
        installSystemInsetGuard(root);

        gamePreview = new GamePreviewView(this);
        root.addView(gamePreview, new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT,
                FrameLayout.LayoutParams.MATCH_PARENT));

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
        aiTitle.setText("AI Edit");
        aiTitle.setTextColor(Color.rgb(35, 45, 60));
        aiTitle.setTextSize(14.0f);
        aiTitle.setTypeface(Typeface.DEFAULT_BOLD);
        controls.addView(aiTitle, fullWidth());

        SharedPreferences aiPrefs = getSharedPreferences(AI_PREFS, MODE_PRIVATE);

        aiPromptEditor = new EditText(this);
        aiPromptEditor.setHint("Describe a game change. AI can inspect and edit any symbol.");
        aiPromptEditor.setSingleLine(false);
        aiPromptEditor.setMinLines(2);
        aiPromptEditor.setTextSize(12.0f);
        controls.addView(aiPromptEditor, fullWidth());

        aiApiKeyEditor = new EditText(this);
        aiApiKeyEditor.setHint("OpenAI API key");
        aiApiKeyEditor.setSingleLine(true);
        aiApiKeyEditor.setText(aiPrefs.getString(AI_PREF_API_KEY, ""));
        aiApiKeyEditor.setTextSize(12.0f);
        controls.addView(aiApiKeyEditor, fullWidth());

        aiModelEditor = new EditText(this);
        aiModelEditor.setSingleLine(true);
        aiModelEditor.setText(aiPrefs.getString(AI_PREF_MODEL, "gpt-5.4-mini"));
        aiModelEditor.setTextSize(12.0f);
        controls.addView(aiModelEditor, fullWidth());

        Button aiPatch = new Button(this);
        aiPatch.setText("AI Edit Workspace");
        aiPatch.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                runAiPatch();
            }
        });
        controls.addView(aiPatch, fullWidth());

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
        return controls;
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

        Button refreshChanges = new Button(this);
        refreshChanges.setText("Changes");
        refreshChanges.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                refreshChangeSummary(loadBundledProject());
            }
        });
        controls.addView(refreshChanges, fullWidth());

        Button resetProject = new Button(this);
        resetProject.setText("Reset Project");
        resetProject.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                resetProjectFiles();
            }
        });
        controls.addView(resetProject, fullWidth());

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
        String apiKey = aiApiKeyEditor == null ? "" : aiApiKeyEditor.getText().toString().trim();
        String prompt = aiPromptEditor == null ? "" : aiPromptEditor.getText().toString().trim();
        String model = aiModelEditor == null ? "" : aiModelEditor.getText().toString().trim();
        if (apiKey.isEmpty() || prompt.isEmpty()) {
            setStatusText("AI edit requires an API key and prompt");
            updateAiProgress(0, 0, "needs input");
            return;
        }
        if (model.isEmpty()) {
            model = "gpt-5.4-mini";
        }
        saveAiSettings(apiKey, model);
        final SymbolEntry symbol = selectedSymbol;
        final String selectedSource = symbol == null || sourceEditor == null ? "" : sourceEditor.getText().toString().trim();
        final String requestJson = buildAiCodeRequestJson(prompt, symbol, selectedSource);
        final String requestModel = model;
        final String requestApiKey = apiKey;
        aiStartedAtNanos = System.nanoTime();
        appendAiTraceFields("request", "model", requestModel, "request_json", requestJson, null, null);
        setStatusText("AI edit: sending workspace context");
        updateAiProgress(0, 0, "queued");
        new Thread(new Runnable() {
            @Override
            public void run() {
                try {
                    final AiAgentResult aiResult = runAiAgentLoop(requestApiKey, requestModel, requestJson);
                    runOnUiThread(new Runnable() {
                        @Override
                        public void run() {
                            applyAiCodeResponse(aiResult, symbol);
                        }
                    });
                } catch (final Exception error) {
                    runOnUiThread(new Runnable() {
                        @Override
                        public void run() {
                            String elapsed = currentAiElapsedText();
                            updateAiProgress(aiProgressStep, aiProgressActions, "failed");
                            appendAiTraceFields("fatal_error", "error", error.getMessage(), "elapsed", elapsed, "trace_path", aiTraceLogPath());
                            setStatusText("AI edit failed: elapsed=" + elapsed + " - " + error.getMessage() + " - trace=" + aiTraceLogPath());
                        }
                    });
                }
            }
        }).start();
    }

    private void saveAiSettings(String apiKey, String model) {
        getSharedPreferences(AI_PREFS, MODE_PRIVATE)
                .edit()
                .putString(AI_PREF_API_KEY, apiKey)
                .putString(AI_PREF_MODEL, model)
                .apply();
    }

    private static String buildAiCodeRequestJson(String prompt, SymbolEntry symbol, String selectedSource) {
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
                    .put("Use tick() as the deterministic simulation step and keep render() as a projection of current state.")
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
            request.put("user_prompt", prompt);
            request.put("scope", "entire_workspace");
            request.put("selected_symbols", selectedSymbols);
            request.put("selected_symbols_are_context_only", true);
            request.put("available_tools", new JSONArray()
                    .put("list_symbols")
                    .put("read_symbol")
                    .put("read_file")
                    .put("read_imports")
                    .put("write_imports")
                    .put("write_symbol")
                    .put("compile_project")
                    .put("get_diagnostics")
                    .put("set_input_state")
                    .put("set_runtime_i32")
                    .put("get_runtime_i32")
                    .put("run_frame")
                    .put("run_for_ticks")
                    .put("inspect_runtime_state")
                    .put("take_screenshot"));
            request.put("stasis_style_rules", rules);
            request.put("game_design_rules", gameRules);
            request.put("architecture_recommendations", architectureRecommendations);
            return request.toString();
        } catch (Exception error) {
            return "{}";
        }
    }

    private AiAgentResult runAiAgentLoop(String apiKey, String model, String initialRequestJson) throws Exception {
        String currentRequestJson = initialRequestJson;
        AiAgentSession session = new AiAgentSession();
        AiUsageAccumulator usage = new AiUsageAccumulator();
        String previousToolCallBatch = "";
        for (int turn = 0; turn < MAX_AI_AGENT_TURNS; turn += 1) {
            session.currentStep = turn + 1;
            postAiProgress(session.currentStep, session.actionCount, "calling AI");
            appendAiTrace("llm_request", new JSONObject().put("turn", session.currentStep).put("model", model).put("request", new JSONObject(currentRequestJson)));
            AiApiResponse apiResponse = callOpenAiResponsesApi(apiKey, model, currentRequestJson);
            usage.add(model, apiResponse.usage);
            appendAiTrace("llm_response", new JSONObject().put("turn", session.currentStep).put("body", apiResponse.body));
            String aiJson = extractAiJsonResponse(apiResponse.body);
            appendAiTrace("llm_json", new JSONObject().put("turn", session.currentStep).put("response", new JSONObject(aiJson)));
            JSONObject response = new JSONObject(aiJson);
            JSONArray toolCalls = response.optJSONArray("tool_calls");
            String mode = response.optString("mode", "edits");
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
                if (session.successfulWriteCount > 0 && compileReady) {
                    return new AiAgentResult(repeated.toString(), usage.toJson(model), usage.summary(), session.currentStep, session.actionCount);
                }
                throw new IOException("AI repeated identical tool calls; actions=" + session.actionCount + " successful_writes=" + session.successfulWriteCount + " rolled_back_writes=" + session.rolledBackWriteCount + " last_tool=" + session.lastToolSummary + " last_error=" + session.lastToolError);
            }
            previousToolCallBatch = currentToolCallBatch;
            postAiProgress(session.currentStep, session.actionCount, "tools " + toolCalls.length());
            appendAiTrace("tool_calls", new JSONObject().put("turn", session.currentStep).put("tool_calls", toolCalls));
            JSONArray observations = executeAiToolCalls(toolCalls, session);
            appendAiTrace("tool_observations", new JSONObject().put("turn", session.currentStep).put("observations", observations));
            JSONObject followup = new JSONObject();
            followup.put("original_request", new JSONObject(initialRequestJson));
            followup.put("tool_observations", observations);
            followup.put("instruction", "Use the tool observations to either request more tools or return final edits. Inspect current symbols before writing unless the exact current source is already available. Apply code changes with write_symbol or write_imports before final edits so compile failures return observations you can correct. Tool errors and validation_error observations are not final; use accepted_shape, required_args, and the error observation to choose another tool call or corrected write. Return mode=edits only after the intended code has been written and compiled successfully. If no further action is needed, return mode=done with empty tool_calls and empty edits.");
            currentRequestJson = followup.toString();
        }
        postAiProgress(MAX_AI_AGENT_TURNS, session.actionCount, "limit hit");
        if (session.successfulWriteCount > 0 && compileReady) {
            String summary = "Applied " + session.successfulWriteCount + " tool write(s) before response limit";
            JSONObject synthetic = new JSONObject()
                    .put("mode", "done")
                    .put("summary", summary)
                    .put("tool_calls", new JSONArray())
                    .put("edits", new JSONArray())
                    .put("expected_reload", reloadKind(lastCompileResult))
                    .put("reason", "The model reached the tool-call limit after successful write_symbol calls; accepted compiled tool writes.")
                    .put("warning", "tool_call_limit_after_successful_writes")
                    .put("successful_writes", session.successfulWriteCount)
                    .put("rolled_back_writes", session.rolledBackWriteCount)
                    .put("last_tool", session.lastToolSummary)
                    .put("last_error", session.lastToolError);
            appendAiTrace("limit_after_successful_writes", synthetic);
            return new AiAgentResult(synthetic.toString(), usage.toJson(model), usage.summary(), MAX_AI_AGENT_TURNS, session.actionCount);
        }
        throw new IOException("AI agent reached tool-call limit before returning edits; actions=" + session.actionCount + " successful_writes=" + session.successfulWriteCount + " rolled_back_writes=" + session.rolledBackWriteCount + " last_tool=" + session.lastToolSummary + " last_error=" + session.lastToolError);
    }

    private JSONArray executeAiToolCalls(JSONArray toolCalls, AiAgentSession session) throws Exception {
        JSONArray observations = new JSONArray();
        for (int index = 0; index < toolCalls.length(); index += 1) {
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
            try {
                JSONObject result = executeAiToolCall(tool, args, session);
                observation.put("result", result);
                recordAiToolResult(session, tool, result);
            } catch (Exception error) {
                session.lastToolError = error.getMessage();
                observation.put("error", error.getMessage());
            }
            observations.put(observation);
        }
        return observations;
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
                if (!hasTextArg(args, "new_source") && !hasTextArg(args, "source")) {
                    return aiToolValidationError(tool, args, "Tool " + tool + " requires new_source, or source as a compatibility alias", required);
                }
            } else if ("imports".equals(name)) {
                if (!args.has("imports") && !hasTextArg(args, "source") && !hasTextArg(args, "import_source")) {
                    return aiToolValidationError(tool, args, "Tool " + tool + " requires imports array, or source/import_source as a compatibility alias", required);
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
                || "compile_project".equals(tool)
                || "get_diagnostics".equals(tool)
                || "run_frame".equals(tool)
                || "inspect_runtime_state".equals(tool)
                || "take_screenshot".equals(tool)
                || "set_input_state".equals(tool)
                || "run_for_ticks".equals(tool)) {
            return new JSONArray();
        }
        if ("read_symbol".equals(tool)) {
            return new JSONArray().put("name");
        }
        if ("read_file".equals(tool) || "read_imports".equals(tool)) {
            return new JSONArray().put("file");
        }
        if ("write_imports".equals(tool)) {
            return new JSONArray().put("file").put("imports");
        }
        if ("write_symbol".equals(tool)) {
            return new JSONArray().put("file").put("name").put("new_source");
        }
        if ("set_runtime_i32".equals(tool)) {
            return new JSONArray().put("path");
        }
        if ("get_runtime_i32".equals(tool)) {
            return new JSONArray().put("path");
        }
        return null;
    }

    private static JSONArray supportedAiTools() {
        return new JSONArray()
                .put("list_symbols")
                .put("read_symbol")
                .put("read_file")
                .put("read_imports")
                .put("write_imports")
                .put("write_symbol")
                .put("compile_project")
                .put("get_diagnostics")
                .put("set_input_state")
                .put("set_runtime_i32")
                .put("get_runtime_i32")
                .put("run_frame")
                .put("run_for_ticks")
                .put("inspect_runtime_state")
                .put("take_screenshot");
    }

    private static JSONObject aiToolValidationError(String tool, JSONObject args, String error, JSONArray requiredArgs) throws Exception {
        JSONObject acceptedArgs = new JSONObject();
        String normalizedTool = tool == null ? "" : tool;
        if ("read_symbol".equals(normalizedTool)) {
            acceptedArgs.put("name", "symbol_name").put("kind", "function_struct_or_global_optional").put("file", "src/main.stasis_optional").put("owner", "owner_optional");
        } else if ("read_file".equals(normalizedTool)) {
            acceptedArgs.put("file", "src/main.stasis");
        } else if ("read_imports".equals(normalizedTool)) {
            acceptedArgs.put("file", "src/main.stasis");
        } else if ("write_imports".equals(normalizedTool)) {
            acceptedArgs.put("file", "src/main.stasis").put("imports", new JSONArray().put("game_state.stasis").put("systems/collision.stasis"));
        } else if ("write_symbol".equals(normalizedTool)) {
            acceptedArgs.put("file", "src/main.stasis").put("name", "function_name").put("kind", "replace_function").put("owner", "Root").put("new_source", "function function_name(): void {\n    // ...\n}");
        } else if ("set_runtime_i32".equals(normalizedTool)) {
            acceptedArgs.put("path", "GameState.field").put("value", 0);
        } else if ("get_runtime_i32".equals(normalizedTool)) {
            acceptedArgs.put("path", "GameState.field");
        } else if ("set_input_state".equals(normalizedTool)) {
            acceptedArgs.put("x", 180).put("y", 320).put("active", 1).put("screen_w", 360).put("screen_h", 640);
        } else if ("run_for_ticks".equals(normalizedTool)) {
            acceptedArgs.put("ticks", 1);
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
        if ("write_symbol".equals(tool) || "write_imports".equals(tool)) {
            if ("written".equals(status) || "created".equals(status)) {
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
        if ("read_symbol".equals(tool)) {
            return aiToolReadSymbol(session, args);
        }
        if ("read_file".equals(tool)) {
            return aiToolReadFile(session, args);
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
        if ("compile_project".equals(tool)) {
            return aiToolCompileProject();
        }
        if ("get_diagnostics".equals(tool)) {
            return aiToolGetDiagnostics();
        }
        if ("set_input_state".equals(tool)) {
            return aiToolSetInputState(args);
        }
        if ("set_runtime_i32".equals(tool)) {
            return aiToolSetRuntimeI32(args);
        }
        if ("get_runtime_i32".equals(tool)) {
            return aiToolGetRuntimeI32(args);
        }
        if ("run_frame".equals(tool)) {
            return aiToolRunFrame();
        }
        if ("run_for_ticks".equals(tool)) {
            return aiToolRunForTicks(args);
        }
        if ("inspect_runtime_state".equals(tool)) {
            return aiToolInspectRuntimeState();
        }
        if ("take_screenshot".equals(tool)) {
            return aiToolTakeScreenshot();
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
        String source = call.optString("source", call.optString("import_source", ""));
        String[] lines = source.split("\\r?\\n");
        for (String line : lines) {
            String path = normalizeImportPath(line);
            if (!path.isEmpty()) {
                out.put(path);
            }
        }
        return out;
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
    private JSONObject aiToolWriteSymbol(AiAgentSession session, JSONObject call) throws Exception {
        ProjectSnapshot project = session.project();
        Map<String, String> originalSources = snapshotProjectSources(project);
        String kind = call.optString("kind", "replace_function");
        String expectedKind = "replace_struct".equals(kind) || "struct".equals(kind) ? "struct" : "function";
        String editKind = "struct".equals(expectedKind) ? "replace_struct" : "replace_function";
        String newSource = call.optString("new_source", call.optString("source", "")).trim();
        if (newSource.isEmpty()) {
            throw new IOException("No value for new_source");
        }
        boolean existed = findSymbolForAiEditOrNull(project, expectedKind, call, selectedSymbol) != null;
        try {
            SymbolEntry target = resolveAiEditTarget(project, editKind, expectedKind, call, selectedSymbol, newSource);
            validateAiReplacementSource(editKind, target.name, newSource);
            persistSelectedEdit(target, newSource);
            session.invalidateProject();

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

    private JSONObject aiToolRunForTicks(JSONObject call) throws Exception {
        int requested = Math.max(1, Math.min(600, call.optInt("ticks", 1)));
        JSONObject lastFrame = null;
        int ran = 0;
        for (int index = 0; index < requested; index += 1) {
            lastFrame = aiToolRunFrame();
            ran += 1;
            if (lastFrame.optInt("status", -1) != 0) {
                break;
            }
        }
        return new JSONObject()
                .put("ticks_requested", requested)
                .put("ticks_run", ran)
                .put("input", currentInputStateJson())
                .put("last_frame", lastFrame == null ? new JSONObject() : lastFrame)
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

    private static String aiLookupExpectedKind(String kind) {
        if ("replace_struct".equals(kind) || "struct".equals(kind)) {
            return "struct";
        }
        if ("global".equals(kind)) {
            return "global";
        }
        return "function";
    }
    private static AiApiResponse callOpenAiResponsesApi(String apiKey, String model, String requestJson) throws Exception {
        JSONObject payload = new JSONObject();
        payload.put("model", model);
        payload.put("text", buildAiResponseTextFormat());
        payload.put("input", "Return only one JSON object. You may inspect and edit any Stasis symbol in the workspace; selected_symbols are optional context only. You may use mode=tool_calls with tool_calls to inspect or write the Stasis workspace using only these tools: list_symbols, read_symbol, read_file, read_imports, write_imports, write_symbol, compile_project, get_diagnostics, set_input_state, set_runtime_i32, get_runtime_i32, run_frame, run_for_ticks, inspect_runtime_state, take_screenshot. take_screenshot returns a compact logical render snapshot with decoded commands, runtime state, and input. set_input_state controls simulated test input; set_runtime_i32 and get_runtime_i32 mutate or inspect i32 Stasis global paths; run_for_ticks advances the game and returns runtime/render state. Before writing, inspect the current target with list_symbols and read_symbol/read_file unless the exact current source was already provided in selected_symbols or tool observations. For behavior that depends on time since an entity, encounter, projectile, effect, resource, objective, mode, or event was created/entered, prefer local lifecycle state that is reset on creation/entry and incremented by tick over using overall game tick count; inspect creation and update paths together. Follow architecture_recommendations for Stasis code structure when changing or adding features. write_symbol creates or replaces a symbol, compiles immediately, and returns status=rolled_back with diagnostics if the edit breaks compilation. read_imports returns the imports for one file; write_imports replaces that file import block using an imports JSON array, compiles immediately, and rolls back on failure. Apply code changes with write_symbol before final edits so failed writes return observations you can correct. Each tool call must use {\"tool\":\"name\",\"args\":{...}}; include only args relevant to that tool. Return mode=edits with replace_function/replace_struct edits only after write_symbol/write_imports has successfully written and compiled the intended changes. If the requested work is already complete or no code changes are needed, return mode=done with empty tool_calls and empty edits. A replace_function edit for a missing function in an existing file is treated as an added helper. Do not use markdown. Request: " + requestJson);
        byte[] body = payload.toString().getBytes(StandardCharsets.UTF_8);

        HttpURLConnection connection = (HttpURLConnection)new URL("https://api.openai.com/v1/responses").openConnection();
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
        connection.disconnect();
        if (status < 200 || status >= 300) {
            throw new IOException("OpenAI HTTP " + status + ": " + response);
        }
        return new AiApiResponse(response, extractAiUsage(response));
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
                .put("read_symbol")
                .put("read_file")
                .put("read_imports")
                .put("write_imports")
                .put("write_symbol")
                .put("compile_project")
                .put("get_diagnostics")
                .put("set_input_state")
                .put("set_runtime_i32")
                .put("get_runtime_i32")
                .put("run_frame")
                .put("run_for_ticks")
                .put("inspect_runtime_state")
                .put("take_screenshot")));
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
        responseProperties.put("expected_reload", new JSONObject().put("type", "string").put("enum", new JSONArray()
                .put("FastReload")
                .put("ResetRequired")));
        responseProperties.put("reason", new JSONObject().put("type", "string"));

        JSONObject schema = new JSONObject();
        schema.put("type", "object");
        schema.put("additionalProperties", false);
        schema.put("required", new JSONArray()
                .put("mode")
                .put("summary")
                .put("tool_calls")
                .put("edits")
                .put("expected_reload")
                .put("reason"));
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

    private static boolean hasKnownAiPricing(String model) {
        return "gpt-5.4-mini".equals(model);
    }

    private static double estimateAiCostUsd(String model, long inputTokens, long cachedInputTokens, long outputTokens) {
        if (!hasKnownAiPricing(model)) {
            return 0.0;
        }
        long uncachedInputTokens = Math.max(0L, inputTokens - cachedInputTokens);
        double inputCost = uncachedInputTokens * GPT_5_4_MINI_INPUT_USD_PER_MILLION;
        double cachedInputCost = cachedInputTokens * GPT_5_4_MINI_CACHED_INPUT_USD_PER_MILLION;
        double outputCost = outputTokens * GPT_5_4_MINI_OUTPUT_USD_PER_MILLION;
        return (inputCost + cachedInputCost + outputCost) / 1000000.0;
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
                appendAiTrace("apply_done", new JSONObject().put("summary", response.optString("summary", "no actions")).put("compile", compileResult).put("elapsed", elapsed));
                setStatusText("AI edit complete: " + response.optString("summary", "no actions") + " - no actions - " + aiReloadSummary(compileResult) + " - elapsed=" + elapsed + " - " + compileResult + " - " + aiResult.usageSummary + " - trace=" + aiTraceLogPath());
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
            appendAiTrace("apply_edits", new JSONObject().put("summary", response.optString("summary", "updated workspace")).put("compile", compileResult).put("elapsed", elapsed));
            setStatusText("AI edit applied: " + response.optString("summary", "updated workspace") + " - " + aiReloadSummary(compileResult) + " - elapsed=" + elapsed + " - " + compileResult + " - " + aiResult.usageSummary + " - trace=" + aiTraceLogPath());
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
                    setStatusText("AI edit apply failed and rollback failed: elapsed=" + elapsed + " - " + error.getMessage() + " / " + restoreError.getMessage() + " - trace=" + aiTraceLogPath());
                    return;
                }
            }
            String elapsed = currentAiElapsedText();
            updateAiProgress(aiResult.finalStep, aiResult.finalActionCount, "rolled back");
            appendAiTraceFields("apply_failed_rolled_back", "error", error.getMessage(), "elapsed", elapsed, null, null);
            setStatusText("AI edit apply failed and rolled back: elapsed=" + elapsed + " - " + error.getMessage() + " - trace=" + aiTraceLogPath());
        }
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
            if (!newSource.trim().startsWith("struct ") || !expectedName.equals(extractDeclarationName(newSource, "struct"))) {
                throw new IOException("AI replace_struct source does not define expected struct: " + expectedName);
            }
            return;
        }
        if (!newSource.trim().startsWith("function ") || !expectedName.equals(extractDeclarationName(newSource, "function"))) {
            throw new IOException("AI replace_function source does not define expected function: " + expectedName);
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

    private ProjectSnapshot loadBundledAssetSnapshot() throws IOException {
        List<SourceFile> files = new ArrayList<>();
        AssetManager assets = getAssets();
        File projectRoot = projectRoot();
        for (String file : SAMPLE_FILES) {
            files.add(new SourceFile(file, new File(projectRoot, file), readAsset(assets, ASSET_ROOT + file)));
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

    private void resetSelectedEdit() {
        if (selectedSymbol == null) {
            return;
        }

        sourceEditor.setText(selectedSymbol.source.trim());
        setStatusText("Reset editor to selected symbol");
    }

    private String classifySelectedReload(SymbolEntry symbol, String editedSource) {
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
        if (file.isDirectory()) {
            File[] children = file.listFiles();
            if (children != null) {
                for (File child : children) {
                    deleteProjectDirectory(child);
                }
            }
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
        private long outputTokens;
        private double estimatedCostUsd;
        private boolean costAvailable = true;

        void add(String model, JSONObject usage) throws Exception {
            long callInputTokens = usageTokenCount(usage, "input_tokens", "prompt_tokens");
            long callCachedInputTokens = cachedInputTokenCount(usage);
            long callOutputTokens = usageTokenCount(usage, "output_tokens", "completion_tokens");
            boolean callCostAvailable = hasKnownAiPricing(model);
            double callEstimatedCostUsd = estimateAiCostUsd(model, callInputTokens, callCachedInputTokens, callOutputTokens);

            JSONObject call = new JSONObject();
            call.put("turn", calls.length() + 1);
            call.put("model", model);
            call.put("input_tokens", callInputTokens);
            call.put("cached_input_tokens", callCachedInputTokens);
            call.put("output_tokens", callOutputTokens);
            call.put("estimated_cost_usd", callEstimatedCostUsd);
            call.put("cost_available", callCostAvailable);
            calls.put(call);

            inputTokens += callInputTokens;
            cachedInputTokens += callCachedInputTokens;
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
        private ProjectSnapshot cachedProject;

        ProjectSnapshot project() {
            if (cachedProject == null) {
                cachedProject = loadBundledProject();
            }
            return cachedProject;
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
            int next = minPositive(minPositive(nextStruct, nextFunction), nextGlobal);
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
