package com.stasislang.workshop;

import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.ImageDecoder;
import android.graphics.Paint;
import android.graphics.Rect;
import android.graphics.Typeface;
import android.opengl.GLES20;
import android.os.Build;
import android.util.SparseArray;
import android.util.Log;

import org.json.JSONObject;

import java.io.File;
import java.io.IOException;
import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.HashMap;

final class WorkshopTextureProvider implements StasisPreviewRenderer.TextureProvider {
    private static final String LOG_TAG = "StasisRenderer";
    private static final long MANIFEST_CHECK_INTERVAL_NANOS = 500_000_000L;
    private static final long RESTORE_BUDGET_NANOS = 8_000_000L;
    private static final long MAX_ATLAS_CAPACITY_BYTES = 64L * 1024L * 1024L;
    private static final long MAX_TEXT_CACHE_BYTES = 32L * 1024L * 1024L;
    private static final long MAX_TEXT_RASTER_BYTES = 16L * 1024L * 1024L;
    private static final int MAX_TEXT_TEXTURES = 4096;
    private final MainActivity activity;
    private final SparseArray<SpriteTexture> textures = new SparseArray<>();
    private final HashMap<String, SpriteTexture> spriteTexturesByHash = new HashMap<>();
    private final SparseArray<TextTexture> textTextures = new SparseArray<>();
    private final SparseArray<FontInfo> fonts = new SparseArray<>();
    private final ArrayList<DynamicTextTexture> dynamicTextTextures = new ArrayList<>();
    private final ArrayList<AtlasPage> atlasPages = new ArrayList<>();
    private final ArrayList<AtlasPage> dedicatedAtlasPages = new ArrayList<>();
    private final int[] deletedTexture = new int[1];
    private File manifest;
    private String projectRootPath;
    private WorkshopSpriteAtlas atlasLayout;
    private SpriteTexture placeholderRegion;
    private int maximumTextureSize;
    private long manifestStamp = Long.MIN_VALUE;
    private long nextManifestCheckNanos;
    private float rasterScale = 1.0f;
    private float textRasterScale = 1.0f;
    private int densityGeneration = -1;
    private long textUseClock;
    private int surfaceGeneration;
    private int rendererGeneration;
    private String lastFailure;
    private String transitionReason = "none";
    private boolean reportRestoreTiming;
    private long restoreStartedNanos;
    private long spriteResolveNanos;
    private long spriteDecodeNanos;
    private long spriteUploadNanos;
    private long textRasterNanos;
    private int restoredSprites;
    private int restoredTextRuns;
    private long restoreDeadlineNanos;
    private boolean restoreDeferred;
    private int deferredResources;
    private long atlasUploadBytes;
    private int atlasPageCreates;
    private int atlasLiveRegions;
    private final HashSet<String> acceptanceUploads = new HashSet<>();
    private final HashSet<String> acceptanceIdentities = new HashSet<>();
    private int acceptanceProjectSwitches;
    private int acceptanceStaleGenerationRejects;
    private int acceptanceRestoreUploads;
    private int acceptanceDuplicateUploads;
    private int acceptanceMaximumAtlasPages;
    private int acceptanceMaximumLiveRegions;
    private int acceptanceMaximumTextTextures;
    private int acceptanceMaximumFontEntries;
    private long acceptanceSourceBytes;
    private long acceptanceDecodeBytes;
    private long acceptanceUploadBytes;
    private long acceptanceTextureBytes;
    private long acceptanceMaximumCacheBytes;
    private int acceptanceUnderprovisionedSprites;

    WorkshopTextureProvider(MainActivity activity) {
        this.activity = activity;
    }

    @Override
    public void onResourceGenerationChanged(int nextSurfaceGeneration,
            int nextRendererGeneration, boolean discardGpuHandles,
            String nextTransitionReason) {
        if (BuildConfig.STASIS_RENDER_ACCEPTANCE
                && (surfaceGeneration != nextSurfaceGeneration
                || rendererGeneration != nextRendererGeneration)) {
            acceptanceStaleGenerationRejects += liveResourceCount();
            acceptanceIdentities.clear();
        }
        clearTextures(!discardGpuHandles);
        surfaceGeneration = nextSurfaceGeneration;
        rendererGeneration = nextRendererGeneration;
        transitionReason = nextTransitionReason;
        reportRestoreTiming = true;
        restoreStartedNanos = 0L;
        setProjectRoot(activity.projectRootPath());
        manifestStamp = Long.MIN_VALUE;
        nextManifestCheckNanos = 0L;
    }

    @Override
    public void beginRestoreAttempt() {
        lastFailure = null;
        if (reportRestoreTiming) {
            long now = System.nanoTime();
            restoreDeadlineNanos = now + RESTORE_BUDGET_NANOS;
            restoreDeferred = false;
            if (restoreStartedNanos == 0L) {
                restoreStartedNanos = now;
                spriteResolveNanos = 0L;
                spriteDecodeNanos = 0L;
                spriteUploadNanos = 0L;
                textRasterNanos = 0L;
                restoredSprites = 0;
                restoredTextRuns = 0;
                deferredResources = 0;
            }
        }
    }

    @Override
    public String consumeFailure() {
        String failure = lastFailure;
        lastFailure = null;
        if (reportRestoreTiming && !restoreDeferred) {
            long total = Math.max(0L, System.nanoTime() - restoreStartedNanos);
            Log.i(LOG_TAG, "resource_restore_timing reason=" + transitionReason
                    + " total_ms=" + millis(total)
                    + " sprite_resolve_ms=" + millis(spriteResolveNanos)
                    + " sprite_decode_ms=" + millis(spriteDecodeNanos)
                    + " sprite_upload_ms=" + millis(spriteUploadNanos)
                    + " text_raster_ms=" + millis(textRasterNanos)
                    + " sprites=" + restoredSprites + " text_runs=" + restoredTextRuns
                    + " deferred=" + deferredResources);
            reportRestoreTiming = failure != null;
            if (!reportRestoreTiming) restoreStartedNanos = 0L;
        }
        return failure;
    }

    @Override
    public boolean isRestoreComplete() {
        return !reportRestoreTiming;
    }

    @Override
    public void onFrameStart() {
        ensureCurrentProject();
        long now = System.nanoTime();
        if (now < nextManifestCheckNanos) return;
        nextManifestCheckNanos = now + MANIFEST_CHECK_INTERVAL_NANOS;
        long currentStamp = manifest.isFile()
                ? manifest.lastModified() ^ (manifest.length() << 7) : 0L;
        if (currentStamp != manifestStamp) manifestStamp = currentStamp;
    }

    @Override
    public void onDisplayMetricsChanged(float nextRasterScale, float nextTextRasterScale,
            int nextDensityGeneration) {
        boolean spriteScaleChanged = densityGeneration != nextDensityGeneration
                || Float.floatToIntBits(rasterScale) != Float.floatToIntBits(nextRasterScale);
        boolean textScaleChanged = Float.floatToIntBits(textRasterScale)
                != Float.floatToIntBits(nextTextRasterScale);
        if (!spriteScaleChanged && !textScaleChanged) return;
        if (spriteScaleChanged) clearSpriteTextures(true);
        if (textScaleChanged) clearTextTextures(true);
        rasterScale = nextRasterScale;
        textRasterScale = nextTextRasterScale;
        densityGeneration = nextDensityGeneration;
    }

    @Override
    public int textureFor(int handle) {
        return textureFor(handle, null);
    }

    @Override
    public int textureFor(int handle, AndroidRasterPlan.Requirement requirement) {
        if (usesFallbackSprite(handle)) return fallbackTexture();
        SpriteTexture cached = textures.get(handle);
        AndroidRasterPlan.Result cachedPlan = null;
        if (cached != null) {
            cachedPlan = AndroidRasterPlan.exact(cached.logicalWidth, cached.logicalHeight,
                    requirement, rasterScale,
                    WorkshopSpriteAtlas.maximumRasterWidth(maximumTextureSize),
                    WorkshopSpriteAtlas.maximumRasterHeight(maximumTextureSize));
        }
        if (cached != null && cached.matches(surfaceGeneration, rendererGeneration)
                && cached.checkedManifestStamp == manifestStamp
                && cachedPlan.supported
                && cached.rasterWidth == cachedPlan.width && cached.rasterHeight == cachedPlan.height) {
            return cached.texture;
        }
        if (cached != null && !cached.matches(surfaceGeneration, rendererGeneration)) {
            textures.remove(handle);
            releaseSpriteIfUnreferenced(cached);
            cached = null;
        }
        if (deferRestoreResource()) return fallbackTexture();
        JSONObject resolved = null;
        try {
            long resolveStarted = System.nanoTime();
            resolved = new JSONObject(MainActivity.nativeResolveSpriteAsset(
                    projectRootPath, handle));
            if (reportRestoreTiming) spriteResolveNanos += System.nanoTime() - resolveStarted;
            if (!"ok".equals(resolved.optString("status"))) {
                throw new IOException(resolved.optString("error", "sprite resolution failed"));
            }
            String hash = resolved.getString("content_sha256");
            ensureAtlas();
            AndroidRasterPlan.Result plan = AndroidRasterPlan.exact(
                    Math.max(1, resolved.optInt("width")),
                    Math.max(1, resolved.optInt("height")), requirement, rasterScale,
                    WorkshopSpriteAtlas.maximumRasterWidth(maximumTextureSize),
                    WorkshopSpriteAtlas.maximumRasterHeight(maximumTextureSize));
            if (!plan.supported) throw new IOException(
                    "required physical raster exceeds Android texture limits");
            String canonicalSource = new File(resolved.getString("path")).getCanonicalPath();
            String exactIdentity = canonicalSource + ":" + hash + ":" + plan.identity(
                    rasterScale, surfaceGeneration, rendererGeneration);
            if (cached != null && exactIdentity.equals(cached.exactIdentity)) {
                cached.checkedManifestStamp = manifestStamp;
                return cached.texture;
            }
            SpriteTexture shared = spriteTexturesByHash.get(exactIdentity);
            if (shared != null && shared.matches(surfaceGeneration, rendererGeneration)) {
                shared.checkedManifestStamp = manifestStamp;
                textures.put(handle, shared);
                releaseSpriteIfUnreferenced(cached);
                return shared.texture;
            }
            long decodeStarted = System.nanoTime();
            DecodedSprite decoded = decode(resolved, plan);
            if (reportRestoreTiming) spriteDecodeNanos += System.nanoTime() - decodeStarted;
            SpriteTexture replacement;
            try {
                long uploadStarted = System.nanoTime();
                replacement = uploadSprite(decoded, exactIdentity, manifestStamp,
                        Math.max(1, resolved.optInt("width")),
                        Math.max(1, resolved.optInt("height")));
                if (reportRestoreTiming) spriteUploadNanos += System.nanoTime() - uploadStarted;
            } finally {
                decoded.bitmap.recycle();
            }
            if (decoded.sourceUnderprovisioned) {
                Log.w(LOG_TAG, formatSourceResolutionDiagnostic(
                        canonicalSource, hash, decoded.sourceWidth, decoded.sourceHeight,
                        plan.width, plan.height, plan.width,
                        plan.height, decoded.decodedBytes,
                        decoded.preparedBytes));
            }
            if (reportRestoreTiming) restoredSprites += 1;
            textures.put(handle, replacement);
            spriteTexturesByHash.put(exactIdentity, replacement);
            if (BuildConfig.STASIS_RENDER_ACCEPTANCE) {
                acceptanceSourceBytes += new File(resolved.getString("path")).length();
                acceptanceDecodeBytes += replacement.decodedBytes;
                acceptanceUploadBytes += WorkshopSpriteAtlas.uploadBytes(
                        replacement.rasterWidth, replacement.rasterHeight);
                acceptanceTextureBytes += replacement.preparedBytes;
                if (replacement.sourceUnderprovisioned) {
                    acceptanceUnderprovisionedSprites += 1;
                }
            }
            recordAcceptanceUpload("sprite", handle, exactIdentity);
            releaseSpriteIfUnreferenced(cached);
            return replacement.texture;
        } catch (Exception error) {
            recordFailure("sprite", handle,
                    resolved == null ? "<unresolved>" : resolved.optString("path", "<unresolved>"),
                    resolved == null ? 0 : resolved.optInt("width"),
                    resolved == null ? 0 : resolved.optInt("height"), error);
            if (cached != null) {
                cached.checkedManifestStamp = manifestStamp;
                return cached.texture;
            }
            return fallbackTexture();
        }
    }

    @Override
    public int logicalWidthFor(int handle) {
        SpriteTexture cached = textures.get(handle);
        return cached == null ? 1 : cached.logicalWidth;
    }

    @Override
    public int logicalHeightFor(int handle) {
        SpriteTexture cached = textures.get(handle);
        return cached == null ? 1 : cached.logicalHeight;
    }

    @Override public float atlasU0For(int handle) { return spriteFor(handle).u0; }
    @Override public float atlasV0For(int handle) { return spriteFor(handle).v0; }
    @Override public float atlasU1For(int handle) { return spriteFor(handle).u1; }
    @Override public float atlasV1For(int handle) { return spriteFor(handle).v1; }

    private SpriteTexture spriteFor(int handle) {
        SpriteTexture sprite = textures.get(handle);
        return sprite == null ? ensurePlaceholder() : sprite;
    }

    @Override
    public void releaseSprite(int handle) {
        SpriteTexture cached = textures.get(handle);
        if (cached == null) return;
        textures.remove(handle);
        releaseSpriteIfUnreferenced(cached);
    }

    static boolean usesFallbackSprite(int handle) {
        return handle == 0;
    }

    @Override
    public int fallbackTexture() {
        return ensurePlaceholder().texture;
    }

    @Override
    public int solidTextureFor(int preferredTexture) {
        if (preferredTexture != 0 && pageForTexture(preferredTexture) != null) {
            return preferredTexture;
        }
        return ensurePlaceholder().texture;
    }

    @Override
    public float solidUFor(int texture) {
        AtlasPage page = pageForTexture(texture);
        return page == null ? 0.5f : 1.5f / page.width;
    }

    @Override
    public float solidVFor(int texture) {
        AtlasPage page = pageForTexture(texture);
        return page == null ? 0.5f : 1.5f / page.height;
    }

    @Override
    public String atlasMetrics() {
        return "atlas_pages=" + (atlasPages.size() + dedicatedAtlasPages.size())
                + " atlas_page_creates=" + atlasPageCreates
                + " atlas_live_regions=" + atlasLiveRegions
                + " atlas_upload_bytes=" + atlasUploadBytes;
    }

    @Override
    public long cachedTextTextureFor(int runHandle) {
        ensureCurrentProject();
        TextTexture cached = textTextures.get(runHandle);
        JSONObject resolved;
        try {
            resolved = new JSONObject(MainActivity.nativeResolveCachedText(
                    activity.projectRootPath(), runHandle));
            if (!"ok".equals(resolved.optString("status"))) {
                throw new IOException(resolved.optString("error", "cached text resolution failed"));
            }
        } catch (Exception error) {
            if (cached != null) {
                textTextures.remove(runHandle);
                deleteTexture(cached.texture);
            }
            recordFailure("cached_text", runHandle, "<resolved-cached-text>", 0, 0, error);
            return 0L;
        }
        File fontFile = new File(resolved.optString("font_path", ""));
        String text = resolved.optString("text", "");
        String fontIdentity;
        try {
            fontIdentity = fontFile.getCanonicalPath() + ":" + fontFile.length() + ":"
                    + fontFile.lastModified() + ":" + resolved.getInt("font_size");
        } catch (Exception error) {
            recordFailure("cached_text", runHandle, "<resolved-cached-text>", 0, 0, error);
            return 0L;
        }
        String exactIdentity = textIdentity(fontIdentity, text, textRasterScale);
        if (cached != null && cached.matches(surfaceGeneration, rendererGeneration)
                && cached.exactIdentity.equals(exactIdentity)) {
            touchText(cached);
            return StasisPreviewRenderer.packTexture(
                cached.texture, cached.width, cached.height);
        }
        if (cached != null) {
            textTextures.remove(runHandle);
            deleteTexture(cached.texture);
        }
        if (deferRestoreResource()) return 0L;
        try {
            long rasterStarted = System.nanoTime();
            FontInfo resolvedFont = new FontInfo(Typeface.createFromFile(fontFile),
                    resolved.getInt("font_size"), fontFile.length(), fontIdentity);
            cached = rasterText(resolvedFont, text, textRasterScale, exactIdentity);
            textTextures.put(runHandle, cached);
            if (BuildConfig.STASIS_RENDER_ACCEPTANCE) {
                long bytes = cached.byteLength();
                acceptanceSourceBytes += fontFile.length();
                acceptanceDecodeBytes += bytes;
                acceptanceUploadBytes += bytes;
                acceptanceTextureBytes += bytes;
                recordAcceptanceUpload("cached_text", runHandle, exactIdentity);
            }
            if (reportRestoreTiming) {
                textRasterNanos += System.nanoTime() - rasterStarted;
                restoredTextRuns += 1;
            }
            return StasisPreviewRenderer.packTexture(
                    cached.texture, cached.width, cached.height);
        } catch (Exception error) {
            recordFailure("cached_text", runHandle, "<resolved-cached-text>", 0, 0, error);
            return 0L;
        }
    }

    @Override
    public long textTextureFor(int font, ByteBuffer utf8, int offset, int length) {
        ensureCurrentProject();
        final FontInfo resolvedFont;
        try {
            resolvedFont = fontInfo(font);
        } catch (Exception error) {
            recordFailure("text", font, "<resolved-font>", 0, 0, error);
            return 0L;
        }
        for (int index = 0; index < dynamicTextTextures.size(); index += 1) {
            DynamicTextTexture cached = dynamicTextTextures.get(index);
            if (cached.texture.matches(surfaceGeneration, rendererGeneration)
                    && cached.matches(font, utf8, offset, length)) {
                touchText(cached.texture);
                return StasisPreviewRenderer.packTexture(
                        cached.texture.texture, cached.texture.width, cached.texture.height);
            }
        }
        if (deferRestoreResource()) return 0L;
        try {
            long rasterStarted = System.nanoTime();
            byte[] bytes = new byte[length];
            for (int index = 0; index < length; index += 1) bytes[index] = utf8.get(offset + index);
            String text = new String(bytes, StandardCharsets.UTF_8);
            String exactIdentity = textIdentity(resolvedFont.identity, text, textRasterScale);
            TextTexture texture = rasterText(
                    resolvedFont, text, textRasterScale, exactIdentity);
            long rasterBytes = texture.byteLength();
            dynamicTextTextures.add(new DynamicTextTexture(font, bytes, texture));
            if (BuildConfig.STASIS_RENDER_ACCEPTANCE) {
                acceptanceSourceBytes += resolvedFont.sourceBytes;
                acceptanceDecodeBytes += rasterBytes;
                acceptanceUploadBytes += rasterBytes;
                acceptanceTextureBytes += rasterBytes;
                recordAcceptanceUpload("text", font, exactIdentity);
            }
            if (reportRestoreTiming) {
                textRasterNanos += System.nanoTime() - rasterStarted;
                restoredTextRuns += 1;
            }
            return StasisPreviewRenderer.packTexture(texture.texture, texture.width, texture.height);
        } catch (Exception error) {
            recordFailure("text", font, "<resolved-font>", 0, 0, error);
            return 0L;
        }
    }

    private String ensureCurrentProject() {
        String currentProjectRoot = activity.projectRootPath();
        if (projectChanged(projectRootPath, currentProjectRoot)) {
            if (BuildConfig.STASIS_RENDER_ACCEPTANCE && projectRootPath != null) {
                acceptanceProjectSwitches += 1;
                acceptanceIdentities.clear();
            }
            clearTextures();
            setProjectRoot(currentProjectRoot);
        }
        return currentProjectRoot;
    }

    private FontInfo fontInfo(int handle) throws Exception {
        JSONObject resolved = new JSONObject(MainActivity.nativeResolveFont(projectRootPath, handle));
        if (!"ok".equals(resolved.optString("status"))) {
            invalidateFontCaches(handle);
            throw new IOException(resolved.optString("error", "font resolution failed"));
        }
        File fontFile = new File(resolved.getString("font_path"));
        String identity = fontFile.getCanonicalPath() + ":"
                + fontFile.length() + ":" + fontFile.lastModified() + ":"
                + resolved.optString("content_sha256", "") + ":"
                + resolved.getInt("font_size");
        FontInfo cached = fonts.get(handle);
        if (cached != null && cached.identity.equals(identity)) return cached;
        if (cached != null) invalidateFontCaches(handle);
        cached = new FontInfo(Typeface.createFromFile(fontFile),
                resolved.getInt("font_size"), fontFile.length(), identity);
        fonts.put(handle, cached);
        if (BuildConfig.STASIS_RENDER_ACCEPTANCE) {
            acceptanceIdentities.add("font:" + handle + ":" + canonicalProjectRoot()
                    + ":" + resolved.optString("content_sha256", "") + ":"
                    + resolved.getInt("font_size"));
            updateAcceptanceMaximums();
        }
        return cached;
    }

    private void invalidateFontCaches(int handle) {
        fonts.remove(handle);
        for (int index = dynamicTextTextures.size() - 1; index >= 0; index -= 1) {
            DynamicTextTexture cached = dynamicTextTextures.get(index);
            if (cached.font != handle) continue;
            dynamicTextTextures.remove(index);
            deleteTexture(cached.texture.texture);
        }
    }

    private TextTexture rasterText(FontInfo font, String text, float rasterScale,
            String exactIdentity) throws IOException {
        Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG | Paint.SUBPIXEL_TEXT_FLAG);
        paint.setColor(0xffffffff);
        paint.setTextSize(font.size);
        paint.setTypeface(font.typeface);
        Paint.FontMetrics metrics = paint.getFontMetrics();
        TextRasterPlan plan = textRasterPlan(
                paint.measureText(text), metrics.ascent, metrics.descent,
                rasterScale, maximumTextTextureSize());
        if (!plan.supported) throw new IOException(
                "text raster exceeds Android memory or texture limits");
        if (!ensureTextCacheCapacity(plan.byteLength())) throw new IOException(
                "text cache exceeds Android memory limit");
        Bitmap bitmap = Bitmap.createBitmap(
                plan.rasterWidth, plan.rasterHeight, Bitmap.Config.ARGB_8888);
        Canvas canvas = new Canvas(bitmap);
        canvas.scale(plan.scaleX(), plan.scaleY());
        canvas.drawText(text, 0.0f, -metrics.ascent, paint);
        int texture;
        try {
            texture = uploadTextTexture(bitmap);
        } finally {
            bitmap.recycle();
        }
        return new TextTexture(texture, exactIdentity,
                plan.logicalWidth, plan.logicalHeight,
                plan.rasterWidth, plan.rasterHeight,
                surfaceGeneration, rendererGeneration, ++textUseClock);
    }

    static boolean textRasterSupported(int width, int height) {
        return width > 0 && height > 0
                && (long)width * height * 4L <= MAX_TEXT_RASTER_BYTES;
    }

    static TextRasterPlan textRasterPlan(float measuredWidth, float ascent, float descent,
            float rasterScale, int maximumTextureSize) {
        int logicalWidth = Math.max(1, (int)Math.ceil(Math.max(0.0f, measuredWidth)));
        int logicalHeight = Math.max(1, (int)Math.ceil(Math.max(0.0f, descent - ascent)));
        double scale = Float.isFinite(rasterScale) && rasterScale >= 1.0f
                ? rasterScale : 1.0;
        double rasterWidth = Math.ceil(logicalWidth * scale);
        double rasterHeight = Math.ceil(logicalHeight * scale);
        if (rasterWidth > Integer.MAX_VALUE || rasterHeight > Integer.MAX_VALUE) {
            return new TextRasterPlan(logicalWidth, logicalHeight, 0, 0, false);
        }
        int width = Math.max(1, (int)rasterWidth);
        int height = Math.max(1, (int)rasterHeight);
        boolean supported = maximumTextureSize > 0
                && width <= maximumTextureSize && height <= maximumTextureSize
                && textRasterSupported(width, height);
        return new TextRasterPlan(logicalWidth, logicalHeight, width, height, supported);
    }

    static String textIdentity(String fontIdentity, String text, float rasterScale) {
        return fontIdentity + ":" + Float.toHexString(rasterScale) + ":" + sha256(text);
    }

    private long textCacheBytes() {
        long bytes = 0L;
        for (int index = 0; index < textTextures.size(); index += 1) {
            TextTexture texture = textTextures.valueAt(index);
            bytes += (long)texture.rasterWidth * texture.rasterHeight * 4L;
        }
        for (DynamicTextTexture dynamic : dynamicTextTextures) {
            bytes += (long)dynamic.texture.rasterWidth * dynamic.texture.rasterHeight * 4L;
        }
        return bytes;
    }

    private boolean ensureTextCacheCapacity(long requiredBytes) {
        if (requiredBytes < 0L || requiredBytes > MAX_TEXT_CACHE_BYTES) return false;
        while (textCacheBytes() > MAX_TEXT_CACHE_BYTES - requiredBytes
                || textTextures.size() + dynamicTextTextures.size() >= MAX_TEXT_TEXTURES) {
            if (!evictOldestTextTexture()) return false;
        }
        return true;
    }

    private boolean evictOldestTextTexture() {
        int staticIndex = -1;
        int dynamicIndex = -1;
        long oldest = Long.MAX_VALUE;
        for (int index = 0; index < textTextures.size(); index += 1) {
            long lastUse = textTextures.valueAt(index).lastUse;
            if (lastUse < oldest) {
                oldest = lastUse;
                staticIndex = index;
                dynamicIndex = -1;
            }
        }
        for (int index = 0; index < dynamicTextTextures.size(); index += 1) {
            long lastUse = dynamicTextTextures.get(index).texture.lastUse;
            if (lastUse < oldest) {
                oldest = lastUse;
                staticIndex = -1;
                dynamicIndex = index;
            }
        }
        if (staticIndex >= 0) {
            TextTexture evicted = textTextures.valueAt(staticIndex);
            textTextures.removeAt(staticIndex);
            deleteTexture(evicted.texture);
            return true;
        }
        if (dynamicIndex >= 0) {
            TextTexture evicted = dynamicTextTextures.remove(dynamicIndex).texture;
            deleteTexture(evicted.texture);
            return true;
        }
        return false;
    }

    private void touchText(TextTexture texture) {
        texture.lastUse = ++textUseClock;
    }

    static boolean projectChanged(String boundRoot, String currentRoot) {
        return boundRoot == null || !boundRoot.equals(currentRoot);
    }

    static boolean generationMatches(int entrySurface, int entryRenderer,
            int surface, int renderer) {
        return entrySurface == surface && entryRenderer == renderer;
    }

    static String acceptanceIdentity(String kind, int handle, String projectRoot,
            String exactIdentity) {
        return kind + ":" + handle + ":" + projectRoot + ":" + exactIdentity;
    }

    static String formatSourceResolutionDiagnostic(String path, String contentHash,
            int sourceWidth, int sourceHeight, int requiredWidth, int requiredHeight,
            int preparedWidth, int preparedHeight, long decodedBytes, long preparedBytes) {
        return "sprite_source_resolution status=source-underprovisioned path=" + path
                + " content_sha256=" + contentHash
                + " source=" + sourceWidth + "x" + sourceHeight
                + " required=" + requiredWidth + "x" + requiredHeight
                + " prepared=" + preparedWidth + "x" + preparedHeight
                + " decoded_bytes=" + decodedBytes
                + " prepared_bytes=" + preparedBytes;
    }

    synchronized void resetAcceptanceMetrics() {
        if (!BuildConfig.STASIS_RENDER_ACCEPTANCE) return;
        acceptanceUploads.clear();
        acceptanceIdentities.clear();
        acceptanceProjectSwitches = 0;
        acceptanceStaleGenerationRejects = 0;
        acceptanceRestoreUploads = 0;
        acceptanceDuplicateUploads = 0;
        acceptanceMaximumAtlasPages = 0;
        acceptanceMaximumLiveRegions = 0;
        acceptanceMaximumTextTextures = 0;
        acceptanceMaximumFontEntries = 0;
        acceptanceSourceBytes = 0L;
        acceptanceDecodeBytes = 0L;
        acceptanceUploadBytes = 0L;
        acceptanceTextureBytes = 0L;
        acceptanceMaximumCacheBytes = 0L;
        acceptanceUnderprovisionedSprites = 0;
    }

    synchronized JSONObject acceptanceSnapshot() throws Exception {
        updateAcceptanceMaximums();
        org.json.JSONArray handles = new org.json.JSONArray();
        for (int index = 0; index < textures.size(); index += 1) {
            handles.put(textures.keyAt(index));
        }
        org.json.JSONArray identities = new org.json.JSONArray();
        ArrayList<String> ordered = new ArrayList<>(acceptanceIdentities);
        java.util.Collections.sort(ordered);
        for (String identity : ordered) identities.put(identity);
        return new JSONObject()
                .put("project_root", canonicalProjectRoot())
                .put("surface_generation", surfaceGeneration)
                .put("renderer_generation", rendererGeneration)
                .put("sprite_handles", handles)
                .put("identities", identities)
                .put("project_switches", acceptanceProjectSwitches)
                .put("stale_generation_rejections", acceptanceStaleGenerationRejects)
                .put("restore_uploads", acceptanceRestoreUploads)
                .put("duplicate_restore_uploads", acceptanceDuplicateUploads)
                .put("atlas_pages", atlasPages.size() + dedicatedAtlasPages.size())
                .put("atlas_live_regions", atlasLiveRegions)
                .put("text_textures", textTextures.size() + dynamicTextTextures.size())
                .put("font_entries", fonts.size())
                .put("maximum_atlas_pages", acceptanceMaximumAtlasPages)
                .put("maximum_live_regions", acceptanceMaximumLiveRegions)
                .put("maximum_text_textures", acceptanceMaximumTextTextures)
                .put("maximum_font_entries", acceptanceMaximumFontEntries)
                .put("source_bytes", acceptanceSourceBytes)
                .put("decode_bytes", acceptanceDecodeBytes)
                .put("upload_bytes", acceptanceUploadBytes)
                .put("texture_bytes", acceptanceTextureBytes)
                .put("underprovisioned_sprites", acceptanceUnderprovisionedSprites)
                .put("maximum_cache_bytes", acceptanceMaximumCacheBytes)
                .put("atlas_capacity_bytes", atlasCapacityBytes());
    }

    private void recordAcceptanceUpload(String kind, int handle, String identity) {
        if (!BuildConfig.STASIS_RENDER_ACCEPTANCE) return;
        String exact = acceptanceIdentity(kind, handle, canonicalProjectRoot(), identity);
        acceptanceIdentities.add(exact);
        String upload = exact + ":" + surfaceGeneration + ":" + rendererGeneration;
        if (!acceptanceUploads.add(upload)) acceptanceDuplicateUploads += 1;
        acceptanceRestoreUploads += 1;
        updateAcceptanceMaximums();
    }

    private void updateAcceptanceMaximums() {
        acceptanceMaximumAtlasPages = Math.max(acceptanceMaximumAtlasPages,
                atlasPages.size() + dedicatedAtlasPages.size());
        acceptanceMaximumLiveRegions = Math.max(acceptanceMaximumLiveRegions, atlasLiveRegions);
        acceptanceMaximumTextTextures = Math.max(acceptanceMaximumTextTextures,
                textTextures.size() + dynamicTextTextures.size());
        acceptanceMaximumFontEntries = Math.max(acceptanceMaximumFontEntries, fonts.size());
        long cacheBytes = 0L;
        for (SpriteTexture texture : spriteTexturesByHash.values()) {
            cacheBytes += (long)texture.rasterWidth * texture.rasterHeight * 4L;
        }
        for (int index = 0; index < textTextures.size(); index += 1) {
            TextTexture texture = textTextures.valueAt(index);
            cacheBytes += (long)texture.rasterWidth * texture.rasterHeight * 4L;
        }
        for (DynamicTextTexture dynamic : dynamicTextTextures) {
            cacheBytes += (long)dynamic.texture.rasterWidth * dynamic.texture.rasterHeight * 4L;
        }
        acceptanceMaximumCacheBytes = Math.max(acceptanceMaximumCacheBytes, cacheBytes);
    }

    private int liveResourceCount() {
        return textures.size() + textTextures.size() + dynamicTextTextures.size() + fonts.size();
    }

    private String canonicalProjectRoot() {
        if (projectRootPath == null) return "";
        try {
            return new File(projectRootPath).getCanonicalPath();
        } catch (IOException ignored) {
            return new File(projectRootPath).getAbsolutePath();
        }
    }

    private static String sha256(String text) {
        return sha256(text.getBytes(StandardCharsets.UTF_8));
    }

    private static String sha256(byte[] bytes) {
        try {
            MessageDigest digest = MessageDigest.getInstance("SHA-256");
            StringBuilder output = new StringBuilder();
            for (byte value : digest.digest(bytes)) {
                output.append(String.format(java.util.Locale.US, "%02x", value & 0xff));
            }
            return output.toString();
        } catch (Exception impossible) {
            throw new IllegalStateException(impossible);
        }
    }

    private void setProjectRoot(String root) {
        projectRootPath = root;
        manifest = new File(root, WorkshopAssetManifest.RELATIVE_PATH);
        manifestStamp = Long.MIN_VALUE;
        nextManifestCheckNanos = 0L;
    }

    private void clearTextures() {
        clearTextures(true);
    }

    private void clearTextures(boolean deleteGpuHandles) {
        clearSpriteTextures(deleteGpuHandles);
        clearTextTextures(deleteGpuHandles);
        fonts.clear();
    }

    private void clearSpriteTextures(boolean deleteGpuHandles) {
        if (deleteGpuHandles) {
            for (AtlasPage page : atlasPages) deleteTexture(page.texture);
            for (AtlasPage page : dedicatedAtlasPages) deleteTexture(page.texture);
        }
        atlasPages.clear();
        dedicatedAtlasPages.clear();
        atlasLayout = null;
        placeholderRegion = null;
        maximumTextureSize = 0;
        atlasUploadBytes = 0L;
        atlasPageCreates = 0;
        atlasLiveRegions = 0;
        textures.clear();
        spriteTexturesByHash.clear();
    }

    private void clearTextTextures(boolean deleteGpuHandles) {
        for (int index = 0; index < textTextures.size(); index++) {
            if (deleteGpuHandles) deleteTexture(textTextures.valueAt(index).texture);
        }
        textTextures.clear();
        for (DynamicTextTexture texture : dynamicTextTextures) {
            if (deleteGpuHandles) deleteTexture(texture.texture.texture);
        }
        dynamicTextTextures.clear();
        textUseClock = 0L;
    }

    private void recordFailure(String stage, int handle, String path,
            int logicalWidth, int logicalHeight, Exception error) {
        lastFailure = StasisPreviewRenderer.formatResourceFailure(stage, handle, path,
                logicalWidth, logicalHeight,
                Math.max(0, Math.round(logicalWidth * rasterScale)),
                Math.max(0, Math.round(logicalHeight * rasterScale)),
                surfaceGeneration, rendererGeneration, transitionReason, error.getMessage());
        activity.reportPreviewResourceError(lastFailure);
    }

    private void deleteTexture(int texture) {
        deletedTexture[0] = texture;
        GLES20.glDeleteTextures(1, deletedTexture, 0);
    }

    private void releaseSpriteIfUnreferenced(SpriteTexture candidate) {
        if (candidate == null) return;
        for (int index = 0; index < textures.size(); index += 1) {
            if (textures.valueAt(index) == candidate) return;
        }
        if (spriteTexturesByHash.remove(candidate.exactIdentity, candidate)) {
            // Atlas storage is generation-owned. Regions are intentionally not reclaimed
            // mid-generation, so replay never observes a partially reused allocation.
        }
    }

    private static String millis(long nanos) {
        return String.format(java.util.Locale.US, "%.3f", nanos / 1_000_000.0);
    }

    private boolean deferRestoreResource() {
        if (!reportRestoreTiming || System.nanoTime() < restoreDeadlineNanos) return false;
        restoreDeferred = true;
        deferredResources += 1;
        return true;
    }

    private static DecodedSprite decode(JSONObject resolved, AndroidRasterPlan.Result plan)
            throws Exception {
        String encoding = resolved.getString("encoding");
        int width = resolved.getInt("width");
        int height = resolved.getInt("height");
        int targetWidth = plan.width;
        int targetHeight = plan.height;
        long pixels = (long)targetWidth * targetHeight;
        if (width <= 0 || height <= 0 || targetWidth > 16384 || targetHeight > 16384
                || pixels > 16_000_000L) {
            throw new IOException("sprite dimensions exceed Android decode limits");
        }
        File file = new File(resolved.getString("path"));
        if (!file.isFile() || file.length() > 64L * 1024L * 1024L) {
            throw new IOException("sprite file exceeds Android decode limits");
        }
        if ("svg".equals(encoding)) {
            int[] argb = MainActivity.nativeDecodeSvgSprite(
                    file.getAbsolutePath(), targetWidth, targetHeight);
            if (argb == null || argb.length != targetWidth * targetHeight) {
                throw new IOException("Android could not decode the SVG sprite");
            }
            Bitmap bitmap = Bitmap.createBitmap(
                    argb, targetWidth, targetHeight, Bitmap.Config.ARGB_8888);
            return new DecodedSprite(bitmap, width, height, false,
                    plan.preparedBytes(), plan.preparedBytes());
        }
        if (!"png".equals(encoding) && !"jpeg".equals(encoding) && !"webp".equals(encoding)) {
            throw new IOException("unsupported Android sprite encoding " + encoding);
        }
        android.graphics.BitmapFactory.Options bounds = new android.graphics.BitmapFactory.Options();
        bounds.inJustDecodeBounds = true;
        android.graphics.BitmapFactory.decodeFile(file.getAbsolutePath(), bounds);
        if (bounds.outWidth != width || bounds.outHeight != height) {
            throw new IOException("decoded sprite dimensions do not match the manifest");
        }
        boolean sourceUnderprovisioned = plan.sourceUnderprovisioned(width, height);
        Bitmap bitmap;
        if (Build.VERSION.SDK_INT >= 28) {
            ImageDecoder.Source source = ImageDecoder.createSource(file);
            bitmap = ImageDecoder.decodeBitmap(source, (decoder, info, source1) -> {
                decoder.setAllocator(ImageDecoder.ALLOCATOR_SOFTWARE);
                if (!sourceUnderprovisioned) {
                    decoder.setTargetSize(targetWidth, targetHeight);
                }
            });
        } else {
            android.graphics.BitmapFactory.Options options = new android.graphics.BitmapFactory.Options();
            options.inPreferredConfig = Bitmap.Config.ARGB_8888;
            options.inScaled = !sourceUnderprovisioned;
            if (!sourceUnderprovisioned) {
                options.inDensity = width;
                options.inTargetDensity = targetWidth;
            }
            bitmap = android.graphics.BitmapFactory.decodeFile(file.getAbsolutePath(), options);
        }
        if (bitmap == null) throw new IOException("Android could not decode the sprite");
        if (bitmap.getWidth() != targetWidth || bitmap.getHeight() != targetHeight) {
            Bitmap exact = Bitmap.createScaledBitmap(bitmap, targetWidth, targetHeight, true);
            bitmap.recycle();
            bitmap = exact;
        }
        return new DecodedSprite(bitmap, width, height, sourceUnderprovisioned,
                plan.decodedBytes(width, height), plan.preparedBytes());
    }

    private SpriteTexture uploadSprite(DecodedSprite decoded, String exactIdentity,
            long checkedStamp,
            int logicalWidth, int logicalHeight) throws IOException {
        Bitmap bitmap = decoded.bitmap;
        ensureAtlas();
        WorkshopSpriteAtlas.Region region = atlasLayout.allocate(
                bitmap.getWidth(), bitmap.getHeight());
        AtlasPage page;
        int x;
        int y;
        if (region != null) {
            while (atlasPages.size() <= region.page) atlasPages.add(createAtlasPage(
                    atlasLayout.pageSize(), atlasLayout.pageSize()));
            page = atlasPages.get(region.page);
            x = region.x;
            y = region.y;
        } else {
            int width = bitmap.getWidth() + WorkshopSpriteAtlas.DEDICATED_WIDTH_OVERHEAD;
            int height = bitmap.getHeight() + WorkshopSpriteAtlas.DEDICATED_HEIGHT_OVERHEAD;
            if (width > maximumTextureSize || height > maximumTextureSize) {
                throw new IOException("sprite dimensions exceed the GLES atlas limit");
            }
            page = createAtlasPage(width, height);
            dedicatedAtlasPages.add(page);
            x = WorkshopSpriteAtlas.PADDING;
            y = WorkshopSpriteAtlas.PADDING + WorkshopSpriteAtlas.PRIVATE_HEADER_HEIGHT;
        }
        Bitmap padded = extrude(bitmap);
        try {
            GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, page.texture);
            while (GLES20.glGetError() != GLES20.GL_NO_ERROR) {}
            android.opengl.GLUtils.texSubImage2D(GLES20.GL_TEXTURE_2D, 0,
                    x - WorkshopSpriteAtlas.PADDING,
                    y - WorkshopSpriteAtlas.PADDING, padded);
            int error = GLES20.glGetError();
            GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, 0);
            if (error != GLES20.GL_NO_ERROR) {
                throw new IOException("Android atlas upload failed with GL error " + error);
            }
            atlasUploadBytes += (long)padded.getWidth() * padded.getHeight() * 4L;
            atlasLiveRegions += 1;
        } finally {
            padded.recycle();
        }
        return new SpriteTexture(page.texture, exactIdentity, checkedStamp,
                logicalWidth, logicalHeight, bitmap.getWidth(), bitmap.getHeight(),
                decoded.sourceWidth, decoded.sourceHeight,
                decoded.sourceUnderprovisioned, decoded.decodedBytes, decoded.preparedBytes,
                (float)x / page.width, (float)y / page.height,
                (float)(x + bitmap.getWidth()) / page.width,
                (float)(y + bitmap.getHeight()) / page.height,
                surfaceGeneration, rendererGeneration);
    }

    private void ensureAtlas() throws IOException {
        if (atlasLayout != null) return;
        maximumTextTextureSize();
        atlasLayout = new WorkshopSpriteAtlas(maximumTextureSize);
        atlasPages.add(createAtlasPage(atlasLayout.pageSize(), atlasLayout.pageSize()));
    }

    private int maximumTextTextureSize() throws IOException {
        if (maximumTextureSize > 0) return maximumTextureSize;
        int[] maximum = new int[1];
        GLES20.glGetIntegerv(GLES20.GL_MAX_TEXTURE_SIZE, maximum, 0);
        if (maximum[0] <= 0) throw new IOException("GLES maximum texture size is unavailable");
        maximumTextureSize = maximum[0];
        return maximumTextureSize;
    }

    private SpriteTexture ensurePlaceholder() {
        if (placeholderRegion != null) return placeholderRegion;
        try {
            ensureAtlas();
            AtlasPage page = atlasPages.get(0);
            placeholderRegion = new SpriteTexture(page.texture, "<placeholder>",
                    manifestStamp, 2, 2, 2, 2,
                    2, 2, false, 16L, 16L,
                    4.0f / page.width, 1.0f / page.height,
                    6.0f / page.width, 3.0f / page.height,
                    surfaceGeneration, rendererGeneration);
        } catch (IOException error) {
            recordFailure("placeholder", 0, "<atlas>", 2, 2, error);
            placeholderRegion = new SpriteTexture(0, "<unavailable>",
                    manifestStamp, 1, 1, 1, 1,
                    1, 1, false, 4L, 4L, 0, 0, 1, 1,
                    surfaceGeneration, rendererGeneration);
        }
        return placeholderRegion;
    }

    private AtlasPage createAtlasPage(int width, int height) throws IOException {
        long requestedBytes = (long)width * height * 4L;
        if (requestedBytes > MAX_ATLAS_CAPACITY_BYTES
                || atlasCapacityBytes() + requestedBytes > MAX_ATLAS_CAPACITY_BYTES) {
            throw new IOException("Android atlas capacity exceeds 64 MiB bound");
        }
        int[] names = new int[1];
        GLES20.glGenTextures(1, names, 0);
        GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, names[0]);
        GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, GLES20.GL_TEXTURE_MIN_FILTER, GLES20.GL_LINEAR);
        GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, GLES20.GL_TEXTURE_MAG_FILTER, GLES20.GL_LINEAR);
        GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, GLES20.GL_TEXTURE_WRAP_S, GLES20.GL_CLAMP_TO_EDGE);
        GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, GLES20.GL_TEXTURE_WRAP_T, GLES20.GL_CLAMP_TO_EDGE);
        while (GLES20.glGetError() != GLES20.GL_NO_ERROR) {}
        GLES20.glTexImage2D(GLES20.GL_TEXTURE_2D, 0, GLES20.GL_RGBA, width, height, 0,
                GLES20.GL_RGBA, GLES20.GL_UNSIGNED_BYTE, null);
        Bitmap header = Bitmap.createBitmap(8, 4, Bitmap.Config.ARGB_8888);
        for (int y = 0; y < 3; y += 1) {
            for (int x = 0; x < 3; x += 1) header.setPixel(x, y, 0xffffffff);
        }
        for (int y = 0; y < 4; y += 1) {
            int sourceY = Math.max(0, Math.min(1, y - 1));
            for (int x = 3; x < 7; x += 1) {
                int sourceX = Math.max(0, Math.min(1, x - 4));
                header.setPixel(x, y, ((sourceX + sourceY) & 1) == 0
                        ? 0xffff00ff : 0xff232323);
            }
        }
        android.opengl.GLUtils.texSubImage2D(GLES20.GL_TEXTURE_2D, 0, 0, 0, header);
        header.recycle();
        int error = GLES20.glGetError();
        GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, 0);
        if (!StasisPreviewRenderer.textureCreationSucceeded(names[0], error)) {
            if (names[0] != 0) GLES20.glDeleteTextures(1, names, 0);
            throw new IOException("atlas page creation failed with GL error " + error);
        }
        atlasPageCreates += 1;
        return new AtlasPage(names[0], width, height);
    }

    private long atlasCapacityBytes() {
        long bytes = 0L;
        for (AtlasPage page : atlasPages) bytes += (long)page.width * page.height * 4L;
        for (AtlasPage page : dedicatedAtlasPages) bytes += (long)page.width * page.height * 4L;
        return bytes;
    }

    private static Bitmap extrude(Bitmap source) {
        int width = source.getWidth();
        int height = source.getHeight();
        Bitmap padded = Bitmap.createBitmap(width + 2, height + 2, Bitmap.Config.ARGB_8888);
        Canvas canvas = new Canvas(padded);
        canvas.drawBitmap(source, 1, 1, null);
        canvas.drawBitmap(source, new Rect(0, 0, width, 1), new Rect(1, 0, width + 1, 1), null);
        canvas.drawBitmap(source, new Rect(0, height - 1, width, height),
                new Rect(1, height + 1, width + 1, height + 2), null);
        canvas.drawBitmap(source, new Rect(0, 0, 1, height), new Rect(0, 1, 1, height + 1), null);
        canvas.drawBitmap(source, new Rect(width - 1, 0, width, height),
                new Rect(width + 1, 1, width + 2, height + 1), null);
        padded.setPixel(0, 0, source.getPixel(0, 0));
        padded.setPixel(width + 1, 0, source.getPixel(width - 1, 0));
        padded.setPixel(0, height + 1, source.getPixel(0, height - 1));
        padded.setPixel(width + 1, height + 1, source.getPixel(width - 1, height - 1));
        return padded;
    }

    private AtlasPage pageForTexture(int texture) {
        for (AtlasPage page : atlasPages) if (page.texture == texture) return page;
        for (AtlasPage page : dedicatedAtlasPages) if (page.texture == texture) return page;
        return null;
    }

    private static int uploadTextTexture(Bitmap bitmap) throws IOException {
        int[] names = new int[1];
        GLES20.glGenTextures(1, names, 0);
        GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, names[0]);
        GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, GLES20.GL_TEXTURE_MIN_FILTER, GLES20.GL_LINEAR);
        GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, GLES20.GL_TEXTURE_MAG_FILTER, GLES20.GL_LINEAR);
        GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, GLES20.GL_TEXTURE_WRAP_S, GLES20.GL_CLAMP_TO_EDGE);
        GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, GLES20.GL_TEXTURE_WRAP_T, GLES20.GL_CLAMP_TO_EDGE);
        while (GLES20.glGetError() != GLES20.GL_NO_ERROR) {}
        android.opengl.GLUtils.texImage2D(GLES20.GL_TEXTURE_2D, 0, bitmap, 0);
        int error = GLES20.glGetError();
        GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, 0);
        if (error != GLES20.GL_NO_ERROR) {
            GLES20.glDeleteTextures(1, names, 0);
            throw new IOException("Android texture upload failed with GL error " + error);
        }
        return names[0];
    }

    private static final class AtlasPage {
        final int texture;
        final int width;
        final int height;
        AtlasPage(int texture, int width, int height) {
            this.texture = texture;
            this.width = width;
            this.height = height;
        }
    }

    private static final class DecodedSprite {
        final Bitmap bitmap;
        final int sourceWidth;
        final int sourceHeight;
        final boolean sourceUnderprovisioned;
        final long decodedBytes;
        final long preparedBytes;

        DecodedSprite(Bitmap bitmap, int sourceWidth, int sourceHeight,
                boolean sourceUnderprovisioned, long decodedBytes, long preparedBytes) {
            this.bitmap = bitmap;
            this.sourceWidth = sourceWidth;
            this.sourceHeight = sourceHeight;
            this.sourceUnderprovisioned = sourceUnderprovisioned;
            this.decodedBytes = decodedBytes;
            this.preparedBytes = preparedBytes;
        }
    }

    private static final class SpriteTexture {
        final int texture;
        final String exactIdentity;
        long checkedManifestStamp;
        final int logicalWidth;
        final int logicalHeight;
        final int rasterWidth;
        final int rasterHeight;
        final int sourceWidth;
        final int sourceHeight;
        final boolean sourceUnderprovisioned;
        final long decodedBytes;
        final long preparedBytes;
        final float u0;
        final float v0;
        final float u1;
        final float v1;

        final int surfaceGeneration;
        final int rendererGeneration;

        SpriteTexture(int texture, String exactIdentity,
                long checkedManifestStamp, int logicalWidth, int logicalHeight,
                int rasterWidth, int rasterHeight,
                int sourceWidth, int sourceHeight, boolean sourceUnderprovisioned,
                long decodedBytes, long preparedBytes,
                float u0, float v0, float u1, float v1,
                int surfaceGeneration, int rendererGeneration) {
            this.texture = texture;
            this.exactIdentity = exactIdentity;
            this.checkedManifestStamp = checkedManifestStamp;
            this.logicalWidth = logicalWidth;
            this.logicalHeight = logicalHeight;
            this.rasterWidth = rasterWidth;
            this.rasterHeight = rasterHeight;
            this.sourceWidth = sourceWidth;
            this.sourceHeight = sourceHeight;
            this.sourceUnderprovisioned = sourceUnderprovisioned;
            this.decodedBytes = decodedBytes;
            this.preparedBytes = preparedBytes;
            this.u0 = u0;
            this.v0 = v0;
            this.u1 = u1;
            this.v1 = v1;
            this.surfaceGeneration = surfaceGeneration;
            this.rendererGeneration = rendererGeneration;
        }

        boolean matches(int surface, int renderer) {
            return generationMatches(surfaceGeneration, rendererGeneration, surface, renderer);
        }
    }

    private static final class TextTexture {
        final int texture;
        final String exactIdentity;
        final int width;
        final int height;
        final int rasterWidth;
        final int rasterHeight;
        final int surfaceGeneration;
        final int rendererGeneration;
        long lastUse;

        TextTexture(int texture, String exactIdentity,
                int width, int height, int rasterWidth, int rasterHeight,
                int surfaceGeneration, int rendererGeneration, long lastUse) {
            this.texture = texture;
            this.exactIdentity = exactIdentity;
            this.width = width;
            this.height = height;
            this.rasterWidth = rasterWidth;
            this.rasterHeight = rasterHeight;
            this.surfaceGeneration = surfaceGeneration;
            this.rendererGeneration = rendererGeneration;
            this.lastUse = lastUse;
        }

        boolean matches(int surface, int renderer) {
            return generationMatches(surfaceGeneration, rendererGeneration, surface, renderer);
        }

        long byteLength() {
            return (long)rasterWidth * rasterHeight * 4L;
        }
    }

    static final class TextRasterPlan {
        final int logicalWidth;
        final int logicalHeight;
        final int rasterWidth;
        final int rasterHeight;
        final boolean supported;

        TextRasterPlan(int logicalWidth, int logicalHeight,
                int rasterWidth, int rasterHeight, boolean supported) {
            this.logicalWidth = logicalWidth;
            this.logicalHeight = logicalHeight;
            this.rasterWidth = rasterWidth;
            this.rasterHeight = rasterHeight;
            this.supported = supported;
        }

        float scaleX() {
            return (float)rasterWidth / logicalWidth;
        }

        float scaleY() {
            return (float)rasterHeight / logicalHeight;
        }

        long byteLength() {
            return (long)rasterWidth * rasterHeight * 4L;
        }
    }

    private static final class FontInfo {
        final Typeface typeface;
        final int size;
        final long sourceBytes;
        final String identity;

        FontInfo(Typeface typeface, int size, long sourceBytes, String identity) {
            this.typeface = typeface;
            this.size = size;
            this.sourceBytes = sourceBytes;
            this.identity = identity;
        }
    }

    private static final class DynamicTextTexture {
        final int font;
        final byte[] text;
        final TextTexture texture;

        DynamicTextTexture(int font, byte[] text, TextTexture texture) {
            this.font = font;
            this.text = text;
            this.texture = texture;
        }

        boolean matches(int candidateFont, ByteBuffer utf8, int offset, int length) {
            if (font != candidateFont || text.length != length) return false;
            for (int index = 0; index < length; index += 1) {
                if (text[index] != utf8.get(offset + index)) return false;
            }
            return true;
        }
    }
}
