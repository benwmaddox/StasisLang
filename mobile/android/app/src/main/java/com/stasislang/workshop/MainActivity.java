package com.stasislang.workshop;

import android.app.Activity;
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
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.FloatBuffer;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.TreeSet;

public final class MainActivity extends Activity {
    private static final String ASSET_ROOT = "workshop_sample/";
    private static final String PROJECT_DIR = "workshop_project";
    private static final long DEFAULT_TICK_INTERVAL_MS = 16L;
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
    private EditText sourceEditor;
    private TextView reloadStatus;
    private TextView gameStatus;
    private GamePreviewView gamePreview;
    private ScrollView editorPanel;
    private Button editorToggle;
    private final Handler gameLoopHandler = new Handler(Looper.getMainLooper());
    private Runnable gameLoop;
    private final RollingMetric tickMetric = new RollingMetric();
    private final RollingMetric renderMetric = new RollingMetric();
    private boolean compileReady;
    private boolean compileAttempted;
    private SymbolEntry selectedSymbol;

    static {
        System.loadLibrary("stasis_mobile_smoke");
    }

    private static native String nativeStatus();
    private static native String nativeCompileProject(String projectRoot);
    private static native String nativeRunTick(String projectRoot, int touchX, int touchY, int touchActive, int screenWidth, int screenHeight);
    private static native int[] nativeRunFrame(String projectRoot, int touchX, int touchY, int touchActive, int screenWidth, int screenHeight);

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

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

        gameStatus = new TextView(this);
        gameStatus.setText("tick=0  tick=-- ms  render=-- ms");
        gameStatus.setTextColor(Color.WHITE);
        gameStatus.setTextSize(12.0f);
        gameStatus.setSingleLine(true);
        gameStatus.setPadding(dp(10), dp(6), dp(10), dp(6));
        gameStatus.setBackgroundColor(Color.argb(150, 20, 28, 38));
        FrameLayout.LayoutParams statusParams = new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.WRAP_CONTENT,
                FrameLayout.LayoutParams.WRAP_CONTENT,
                Gravity.TOP | Gravity.START);
        statusParams.setMargins(dp(8), dp(8), dp(68), 0);
        root.addView(gameStatus, statusParams);

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

        for (SymbolSection section : project.sections) {
            addSection(content, section);
        }

        sourceTitle = new TextView(this);
        sourceTitle.setTextColor(Color.rgb(22, 27, 34));
        sourceTitle.setTextSize(15.0f);
        sourceTitle.setTypeface(Typeface.DEFAULT_BOLD);
        sourceTitle.setPadding(0, dp(14), 0, dp(6));
        content.addView(sourceTitle, fullWidth());

        sourceEditor = new EditText(this);
        sourceEditor.setTextColor(Color.rgb(28, 37, 49));
        sourceEditor.setTextSize(12.0f);
        sourceEditor.setTypeface(Typeface.MONOSPACE);
        sourceEditor.setMinLines(8);
        sourceEditor.setGravity(Gravity.TOP | Gravity.START);
        sourceEditor.setPadding(dp(12), dp(10), dp(12), dp(10));
        sourceEditor.setSingleLine(false);
        sourceEditor.setBackground(createPanelBackground(Color.WHITE, Color.rgb(207, 214, 224)));
        content.addView(sourceEditor, fullWidth());

        content.addView(createEditControls(), fullWidth());

        reloadStatus = new TextView(this);
        reloadStatus.setTextColor(Color.rgb(73, 84, 100));
        reloadStatus.setTextSize(13.0f);
        reloadStatus.setPadding(0, dp(8), 0, dp(6));
        content.addView(reloadStatus, fullWidth());

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
                    String compileResult = nativeCompileProject(projectRoot().getAbsolutePath());
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

    private void updateGameDebugText(RenderFrame frame) {
        if (gameStatus == null) {
            return;
        }
        gameStatus.setText(String.format(
                Locale.US,
                "tick=%d  tick=%.2f ms  render=%.2f ms",
                frame.tickCount,
                tickMetric.averageMillis(),
                renderMetric.averageMillis()));
    }

    private void recordRenderTimeNanos(long durationNanos) {
        renderMetric.add(System.nanoTime(), durationNanos);
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
        setStatusText("No pending edit");
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

        LinearLayout runtimeRow = new LinearLayout(this);
        runtimeRow.setOrientation(LinearLayout.HORIZONTAL);

        Button compile = new Button(this);
        compile.setText("Compile");
        compile.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                runNativeCompile();
            }
        });
        runtimeRow.addView(compile, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1.0f));

        Button runTick = new Button(this);
        runTick.setText("Run Tick");
        runTick.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                runNativeTick();
            }
        });
        runtimeRow.addView(runTick, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1.0f));
        controls.addView(runtimeRow, fullWidth());

        return controls;
    }
    private void runNativeCompile() {
        String compileResult = nativeCompileProject(projectRoot().getAbsolutePath());
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
        int[] frameValues = nativeRunFrame(
                projectRoot().getAbsolutePath(),
                touchX,
                touchY,
                touchActive,
                screenWidth,
                screenHeight);
        long tickEndNanos = System.nanoTime();
        tickMetric.add(tickEndNanos, tickEndNanos - tickStartNanos);
        if (frameValues == null || frameValues.length == 0 || frameValues[0] != 0) {
            compileReady = false;
            compileAttempted = true;
            setStatusText("RunError: native frame tick failed");
            return;
        }
        RenderFrame frame = RenderFrame.fromNativeFrame(frameValues);
        if (gamePreview != null) {
            gamePreview.setRenderFrame(frame);
        }
        updateGameDebugText(frame);
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

    private void applySelectedEdit() {
        if (selectedSymbol == null) {
            return;
        }

        String editedSource = sourceEditor.getText().toString().trim();
        String reload = classifySelectedReload(selectedSymbol, editedSource);
        try {
            persistSelectedEdit(selectedSymbol, editedSource);
            String compileResult = nativeCompileProject(projectRoot().getAbsolutePath());
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
        return new File(getFilesDir(), PROJECT_DIR);
    }
    private ProjectSnapshot loadBundledProject() {
        List<SourceFile> files = new ArrayList<>();
        AssetManager assets = getAssets();
        File projectRoot = projectRoot();
        deleteProjectDirectory(projectRoot);

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
            int next = minPositive(nextStruct, nextFunction);
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
        private final PreviewRenderer renderer;
        private int touchX;
        private int touchY;
        private boolean touchActive;

        GamePreviewView(MainActivity activity) {
            super(activity);
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

        void setRenderFrame(RenderFrame value) {
            renderer.setFrame(value);
            requestRender();
        }

        @Override
        public boolean onTouchEvent(MotionEvent event) {
            touchX = Math.round(event.getX());
            touchY = Math.round(event.getY());
            int action = event.getActionMasked();
            touchActive = action != MotionEvent.ACTION_UP && action != MotionEvent.ACTION_CANCEL;
            return true;
        }
    }

    private static final class PreviewRenderer implements GLSurfaceView.Renderer {
        private static final String VERTEX_SHADER =
                "attribute vec2 aPosition;" +
                "uniform vec2 uResolution;" +
                "void main() {" +
                "  vec2 zeroToOne = aPosition / uResolution;" +
                "  vec2 clip = zeroToOne * 2.0 - 1.0;" +
                "  gl_Position = vec4(clip.x, -clip.y, 0.0, 1.0);" +
                "}";
        private static final String FRAGMENT_SHADER =
                "precision mediump float;" +
                "uniform vec4 uColor;" +
                "void main() {" +
                "  gl_FragColor = uColor;" +
                "}";

        private final MainActivity activity;
        private final FloatBuffer vertexBuffer = ByteBuffer
                .allocateDirect(8 * 4)
                .order(ByteOrder.nativeOrder())
                .asFloatBuffer();
        private RenderFrame frame = RenderFrame.empty();
        private int program;
        private int positionHandle;
        private int resolutionHandle;
        private int colorHandle;
        private int surfaceWidth = 1;
        private int surfaceHeight = 1;

        PreviewRenderer(MainActivity activity) {
            this.activity = activity;
        }

        synchronized void setFrame(RenderFrame value) {
            frame = value;
        }

        @Override
        public void onSurfaceCreated(javax.microedition.khronos.opengles.GL10 gl, javax.microedition.khronos.egl.EGLConfig config) {
            program = createProgram(VERTEX_SHADER, FRAGMENT_SHADER);
            positionHandle = GLES20.glGetAttribLocation(program, "aPosition");
            resolutionHandle = GLES20.glGetUniformLocation(program, "uResolution");
            colorHandle = GLES20.glGetUniformLocation(program, "uColor");
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

            RenderFrame current;
            synchronized (this) {
                current = frame;
            }
            for (int index = 0; index < current.commandCount; index += 1) {
                RenderCommand command = current.commands[index];
                if (command.kind == 1) {
                    drawRect(command);
                }
            }
            activity.recordRenderTimeNanos(System.nanoTime() - renderStartNanos);
        }

        private void drawRect(RenderCommand command) {
            float left = command.x;
            float top = command.y;
            float right = command.x + command.w;
            float bottom = command.y + command.h;
            vertexBuffer.clear();
            vertexBuffer.put(left).put(top);
            vertexBuffer.put(right).put(top);
            vertexBuffer.put(left).put(bottom);
            vertexBuffer.put(right).put(bottom);
            vertexBuffer.flip();

            GLES20.glEnableVertexAttribArray(positionHandle);
            GLES20.glVertexAttribPointer(positionHandle, 2, GLES20.GL_FLOAT, false, 0, vertexBuffer);
            GLES20.glUniform4f(
                    colorHandle,
                    ((command.color >> 16) & 255) / 255.0f,
                    ((command.color >> 8) & 255) / 255.0f,
                    (command.color & 255) / 255.0f,
                    1.0f);
            GLES20.glDrawArrays(GLES20.GL_TRIANGLE_STRIP, 0, 4);
            GLES20.glDisableVertexAttribArray(positionHandle);
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

    private static final class RenderFrame {
        private static final int MAX_COMMANDS = 8;
        final int tickCount;
        final int commandCount;
        final RenderCommand[] commands;

        RenderFrame(int tickCount, int commandCount, RenderCommand[] commands) {
            this.tickCount = tickCount;
            this.commandCount = commandCount;
            this.commands = commands;
        }

        static RenderFrame empty() {
            return new RenderFrame(0, 0, emptyCommands());
        }

        static RenderFrame fromNativeFrame(int[] frameValues) {
            RenderCommand[] commands = emptyCommands();
            int count = Math.max(0, Math.min(MAX_COMMANDS, frameValues[5]));
            for (int index = 0; index < count; index += 1) {
                int base = 6 + index * 6;
                commands[index] = new RenderCommand(
                        frameValues[base],
                        frameValues[base + 1],
                        frameValues[base + 2],
                        frameValues[base + 3],
                        frameValues[base + 4],
                        frameValues[base + 5]);
            }
            return new RenderFrame(frameValues[1], count, commands);
        }

        static RenderFrame fromRunResult(String runResult) {
            RenderCommand[] commands = emptyCommands();
            int count = Math.max(0, Math.min(MAX_COMMANDS, extractIntField(runResult, "render_command_count", 0)));
            for (int index = 0; index < count; index += 1) {
                String prefix = "render" + index + "_";
                commands[index] = new RenderCommand(
                        extractIntField(runResult, prefix + "kind", 0),
                        extractIntField(runResult, prefix + "x", 0),
                        extractIntField(runResult, prefix + "y", 0),
                        extractIntField(runResult, prefix + "w", 0),
                        extractIntField(runResult, prefix + "h", 0),
                        extractIntField(runResult, prefix + "color", Color.WHITE));
            }
            return new RenderFrame(extractIntField(runResult, "tick_count", 0), count, commands);
        }

        private static RenderCommand[] emptyCommands() {
            RenderCommand[] commands = new RenderCommand[MAX_COMMANDS];
            for (int index = 0; index < commands.length; index += 1) {
                commands[index] = RenderCommand.empty();
            }
            return commands;
        }
    }

    private static final class RenderCommand {
        final int kind;
        final int x;
        final int y;
        final int w;
        final int h;
        final int color;

        RenderCommand(int kind, int x, int y, int w, int h, int color) {
            this.kind = kind;
            this.x = x;
            this.y = y;
            this.w = w;
            this.h = h;
            this.color = color;
        }

        static RenderCommand empty() {
            return new RenderCommand(0, 0, 0, 0, 0, Color.TRANSPARENT);
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
        final int start;
        int end;

        SymbolEntry(String kind, String name, String owner, String signature, SourceFile sourceFile, String file, String source, int start, int end) {
            this.kind = kind;
            this.name = name;
            this.owner = owner;
            this.signature = signature;
            this.sourceFile = sourceFile;
            this.file = file;
            this.source = source;
            this.start = start;
            this.end = end;
        }

        String displayName() {
            if ("struct".equals(kind)) {
                return signature;
            }
            return signature;
        }
    }
}

