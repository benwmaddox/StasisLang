package com.stasislang.workshop;

import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.Typeface;
import android.opengl.GLES20;
import android.util.SparseArray;

import org.json.JSONObject;

import java.io.File;
import java.io.IOException;
import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;

final class WorkshopTextureProvider implements StasisPreviewRenderer.TextureProvider {
    private static final long MANIFEST_CHECK_INTERVAL_NANOS = 500_000_000L;
    private final MainActivity activity;
    private final SparseArray<SpriteTexture> textures = new SparseArray<>();
    private final SparseArray<TextTexture> textTextures = new SparseArray<>();
    private final SparseArray<FontInfo> fonts = new SparseArray<>();
    private final ArrayList<DynamicTextTexture> dynamicTextTextures = new ArrayList<>();
    private final int[] deletedTexture = new int[1];
    private File manifest;
    private String projectRootPath;
    private int fallbackTexture;
    private long manifestStamp = Long.MIN_VALUE;
    private long nextManifestCheckNanos;
    private float rasterScale = 1.0f;
    private int densityGeneration = -1;
    private int surfaceGeneration;
    private int rendererGeneration;
    private int fallbackSurfaceGeneration;
    private int fallbackRendererGeneration;
    private String lastFailure;
    private String transitionReason = "none";

    WorkshopTextureProvider(MainActivity activity) {
        this.activity = activity;
    }

    @Override
    public void onResourceGenerationChanged(int nextSurfaceGeneration,
            int nextRendererGeneration, boolean discardGpuHandles,
            String nextTransitionReason) {
        clearTextures(!discardGpuHandles);
        surfaceGeneration = nextSurfaceGeneration;
        rendererGeneration = nextRendererGeneration;
        transitionReason = nextTransitionReason;
        setProjectRoot(activity.projectRootPath());
        manifestStamp = Long.MIN_VALUE;
        nextManifestCheckNanos = 0L;
    }

    @Override
    public void beginRestoreAttempt() {
        lastFailure = null;
    }

    @Override
    public String consumeFailure() {
        String failure = lastFailure;
        lastFailure = null;
        return failure;
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
    public void onDisplayMetricsChanged(float nextRasterScale, int nextDensityGeneration) {
        if (densityGeneration == nextDensityGeneration
                && Math.abs(rasterScale - nextRasterScale) < 0.001f) return;
        clearTextures();
        rasterScale = nextRasterScale;
        densityGeneration = nextDensityGeneration;
    }

    @Override
    public int textureFor(int handle) {
        SpriteTexture cached = textures.get(handle);
        if (cached != null && cached.matches(surfaceGeneration, rendererGeneration)
                && cached.checkedManifestStamp == manifestStamp) {
            return cached.texture;
        }
        if (cached != null && !cached.matches(surfaceGeneration, rendererGeneration)) {
            textures.remove(handle);
            cached = null;
        }
        JSONObject resolved = null;
        try {
            resolved = new JSONObject(MainActivity.nativeResolveSpriteAsset(
                    projectRootPath, handle));
            if (!"ok".equals(resolved.optString("status"))) {
                throw new IOException(resolved.optString("error", "sprite resolution failed"));
            }
            String hash = resolved.getString("content_sha256");
            if (cached != null && hash.equals(cached.contentHash)) {
                cached.checkedManifestStamp = manifestStamp;
                return cached.texture;
            }
            Bitmap bitmap = decode(resolved, rasterScale);
            int uploaded;
            try {
                uploaded = upload(bitmap);
            } finally {
                bitmap.recycle();
            }
            textures.put(handle, new SpriteTexture(uploaded, hash, manifestStamp,
                    surfaceGeneration, rendererGeneration));
            if (cached != null) deleteTexture(cached.texture);
            return uploaded;
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
    public int fallbackTexture() {
        if (fallbackTexture == 0 || fallbackSurfaceGeneration != surfaceGeneration
                || fallbackRendererGeneration != rendererGeneration) {
            try {
                fallbackTexture = createFallbackTexture();
            } catch (IOException error) {
                fallbackTexture = 0;
                recordFailure("fallback", 0, "<procedural>", 2, 2, error);
                return 0;
            }
            fallbackSurfaceGeneration = surfaceGeneration;
            fallbackRendererGeneration = rendererGeneration;
        }
        return fallbackTexture;
    }

    @Override
    public long cachedTextTextureFor(int runHandle) {
        ensureCurrentProject();
        TextTexture cached = textTextures.get(runHandle);
        if (cached != null && cached.matches(surfaceGeneration, rendererGeneration)) {
            return StasisPreviewRenderer.packTexture(
                cached.texture, cached.width, cached.height);
        }
        if (cached != null) textTextures.remove(runHandle);
        try {
            JSONObject resolved = new JSONObject(MainActivity.nativeResolveCachedText(
                    activity.projectRootPath(), runHandle));
            if (!"ok".equals(resolved.optString("status"))) {
                throw new IOException(resolved.optString("error", "cached text resolution failed"));
            }
            Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG | Paint.SUBPIXEL_TEXT_FLAG);
            paint.setColor(0xffffffff);
            paint.setTextSize(resolved.getInt("font_size") * rasterScale);
            paint.setTypeface(Typeface.createFromFile(resolved.getString("font_path")));
            String text = resolved.getString("text");
            Paint.FontMetrics metrics = paint.getFontMetrics();
            int width = Math.max(1, (int)Math.ceil(paint.measureText(text)));
            int height = Math.max(1, (int)Math.ceil(metrics.descent - metrics.ascent));
            Bitmap bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888);
            new Canvas(bitmap).drawText(text, 0.0f, -metrics.ascent, paint);
            int texture;
            try {
                texture = upload(bitmap);
            } finally {
                bitmap.recycle();
            }
            cached = new TextTexture(texture,
                    Math.max(1, Math.round(width / rasterScale)),
                    Math.max(1, Math.round(height / rasterScale)),
                    surfaceGeneration, rendererGeneration);
            textTextures.put(runHandle, cached);
            return StasisPreviewRenderer.packTexture(texture, cached.width, cached.height);
        } catch (Exception error) {
            recordFailure("cached_text", runHandle, "<resolved-cached-text>", 0, 0, error);
            return 0L;
        }
    }

    @Override
    public long textTextureFor(int font, ByteBuffer utf8, int offset, int length) {
        ensureCurrentProject();
        for (int index = 0; index < dynamicTextTextures.size(); index += 1) {
            DynamicTextTexture cached = dynamicTextTextures.get(index);
            if (cached.texture.matches(surfaceGeneration, rendererGeneration)
                    && cached.matches(font, utf8, offset, length)) {
                return StasisPreviewRenderer.packTexture(
                        cached.texture.texture, cached.texture.width, cached.texture.height);
            }
        }
        try {
            if (dynamicTextTextures.size() >= 4096) throw new IOException("dynamic text cache is full");
            byte[] bytes = new byte[length];
            for (int index = 0; index < length; index += 1) bytes[index] = utf8.get(offset + index);
            FontInfo fontInfo = fontInfo(font);
            TextTexture texture = rasterText(
                    fontInfo, new String(bytes, StandardCharsets.UTF_8), rasterScale,
                    surfaceGeneration, rendererGeneration);
            dynamicTextTextures.add(new DynamicTextTexture(font, bytes, texture));
            return StasisPreviewRenderer.packTexture(texture.texture, texture.width, texture.height);
        } catch (Exception error) {
            recordFailure("text", font, "<resolved-font>", 0, 0, error);
            return 0L;
        }
    }

    private String ensureCurrentProject() {
        String currentProjectRoot = activity.projectRootPath();
        if (projectChanged(projectRootPath, currentProjectRoot)) {
            clearTextures();
            setProjectRoot(currentProjectRoot);
        }
        return currentProjectRoot;
    }

    private FontInfo fontInfo(int handle) throws Exception {
        FontInfo cached = fonts.get(handle);
        if (cached != null) return cached;
        JSONObject resolved = new JSONObject(MainActivity.nativeResolveFont(projectRootPath, handle));
        if (!"ok".equals(resolved.optString("status"))) {
            throw new IOException(resolved.optString("error", "font resolution failed"));
        }
        cached = new FontInfo(Typeface.createFromFile(resolved.getString("font_path")),
                resolved.getInt("font_size"));
        fonts.put(handle, cached);
        return cached;
    }

    private static TextTexture rasterText(FontInfo font, String text, float rasterScale,
            int surfaceGeneration, int rendererGeneration) {
        Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG | Paint.SUBPIXEL_TEXT_FLAG);
        paint.setColor(0xffffffff);
        paint.setTextSize(font.size * rasterScale);
        paint.setTypeface(font.typeface);
        Paint.FontMetrics metrics = paint.getFontMetrics();
        int width = Math.max(1, (int)Math.ceil(paint.measureText(text)));
        int height = Math.max(1, (int)Math.ceil(metrics.descent - metrics.ascent));
        Bitmap bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888);
        new Canvas(bitmap).drawText(text, 0.0f, -metrics.ascent, paint);
        int texture;
        try {
            texture = upload(bitmap);
        } catch (IOException error) {
            throw new IllegalStateException(error);
        } finally {
            bitmap.recycle();
        }
        return new TextTexture(texture,
                Math.max(1, Math.round(width / rasterScale)),
                Math.max(1, Math.round(height / rasterScale)),
                surfaceGeneration, rendererGeneration);
    }

    static boolean projectChanged(String boundRoot, String currentRoot) {
        return boundRoot == null || !boundRoot.equals(currentRoot);
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
        for (int index = 0; index < textures.size(); index++) {
            if (deleteGpuHandles) deleteTexture(textures.valueAt(index).texture);
        }
        textures.clear();
        for (int index = 0; index < textTextures.size(); index++) {
            if (deleteGpuHandles) deleteTexture(textTextures.valueAt(index).texture);
        }
        textTextures.clear();
        for (DynamicTextTexture texture : dynamicTextTextures) {
            if (deleteGpuHandles) deleteTexture(texture.texture.texture);
        }
        dynamicTextTextures.clear();
        fonts.clear();
        if (fallbackTexture != 0 && deleteGpuHandles) deleteTexture(fallbackTexture);
        fallbackTexture = 0;
        fallbackSurfaceGeneration = 0;
        fallbackRendererGeneration = 0;
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

    private static Bitmap decode(JSONObject resolved, float rasterScale) throws Exception {
        String encoding = resolved.getString("encoding");
        int width = resolved.getInt("width");
        int height = resolved.getInt("height");
        if ("svg".equals(encoding)) {
            width = Math.max(1, (int)Math.ceil(width * rasterScale));
            height = Math.max(1, (int)Math.ceil(height * rasterScale));
        }
        long pixels = (long)width * height;
        if (width <= 0 || height <= 0 || width > 16384 || height > 16384
                || pixels > 16_000_000L) {
            throw new IOException("sprite dimensions exceed Android decode limits");
        }
        File file = new File(resolved.getString("path"));
        if (!file.isFile() || file.length() > 64L * 1024L * 1024L) {
            throw new IOException("sprite file exceeds Android decode limits");
        }
        if ("svg".equals(encoding)) {
            int[] argb = MainActivity.nativeDecodeSvgSprite(file.getAbsolutePath(), width, height);
            if (argb == null || argb.length != width * height) {
                throw new IOException("Android could not decode the SVG sprite");
            }
            return Bitmap.createBitmap(argb, width, height, Bitmap.Config.ARGB_8888);
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
        android.graphics.BitmapFactory.Options options = new android.graphics.BitmapFactory.Options();
        options.inPreferredConfig = Bitmap.Config.ARGB_8888;
        options.inScaled = false;
        Bitmap bitmap = android.graphics.BitmapFactory.decodeFile(file.getAbsolutePath(), options);
        if (bitmap == null) throw new IOException("Android could not decode the sprite");
        return bitmap;
    }

    private static int upload(Bitmap bitmap) throws IOException {
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

    private static int createFallbackTexture() throws IOException {
        ByteBuffer pixels = ByteBuffer.allocateDirect(16);
        pixels.put(new byte[]{
                (byte)255, 0, (byte)255, (byte)255,
                35, 35, 35, (byte)255,
                35, 35, 35, (byte)255,
                (byte)255, 0, (byte)255, (byte)255});
        pixels.flip();
        int[] names = new int[1];
        GLES20.glGenTextures(1, names, 0);
        GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, names[0]);
        GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, GLES20.GL_TEXTURE_MIN_FILTER, GLES20.GL_NEAREST);
        GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, GLES20.GL_TEXTURE_MAG_FILTER, GLES20.GL_NEAREST);
        GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, GLES20.GL_TEXTURE_WRAP_S, GLES20.GL_CLAMP_TO_EDGE);
        GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, GLES20.GL_TEXTURE_WRAP_T, GLES20.GL_CLAMP_TO_EDGE);
        GLES20.glTexImage2D(GLES20.GL_TEXTURE_2D, 0, GLES20.GL_RGBA, 2, 2, 0,
                GLES20.GL_RGBA, GLES20.GL_UNSIGNED_BYTE, pixels);
        int error = GLES20.glGetError();
        GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, 0);
        if (!StasisPreviewRenderer.textureCreationSucceeded(names[0], error)) {
            if (names[0] != 0) GLES20.glDeleteTextures(1, names, 0);
            throw new IOException("fallback texture upload failed with GL error " + error);
        }
        return names[0];
    }

    private static final class SpriteTexture {
        final int texture;
        final String contentHash;
        long checkedManifestStamp;

        final int surfaceGeneration;
        final int rendererGeneration;

        SpriteTexture(int texture, String contentHash, long checkedManifestStamp,
                int surfaceGeneration, int rendererGeneration) {
            this.texture = texture;
            this.contentHash = contentHash;
            this.checkedManifestStamp = checkedManifestStamp;
            this.surfaceGeneration = surfaceGeneration;
            this.rendererGeneration = rendererGeneration;
        }

        boolean matches(int surface, int renderer) {
            return surfaceGeneration == surface && rendererGeneration == renderer;
        }
    }

    private static final class TextTexture {
        final int texture;
        final int width;
        final int height;
        final int surfaceGeneration;
        final int rendererGeneration;

        TextTexture(int texture, int width, int height,
                int surfaceGeneration, int rendererGeneration) {
            this.texture = texture;
            this.width = width;
            this.height = height;
            this.surfaceGeneration = surfaceGeneration;
            this.rendererGeneration = rendererGeneration;
        }

        boolean matches(int surface, int renderer) {
            return surfaceGeneration == surface && rendererGeneration == renderer;
        }
    }

    private static final class FontInfo {
        final Typeface typeface;
        final int size;

        FontInfo(Typeface typeface, int size) {
            this.typeface = typeface;
            this.size = size;
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
