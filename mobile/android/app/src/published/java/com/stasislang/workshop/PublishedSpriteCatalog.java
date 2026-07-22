package com.stasislang.workshop;

import android.content.res.AssetManager;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.Typeface;
import android.opengl.GLES20;
import android.opengl.GLUtils;
import android.util.SparseArray;
import android.util.SparseBooleanArray;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.util.ArrayList;

final class PublishedSpriteCatalog implements StasisPreviewRenderer.TextureProvider {
    private static final String ROOT = "stasis_game/";
    private static final String MANIFEST = ROOT + "assets/manifest.json";
    private static final int MAX_MANIFEST_BYTES = 1024 * 1024;
    private static final int MAX_ASSET_BYTES = 64 * 1024 * 1024;
    private static final int MAX_ASSETS = 4096;
    private static final int MAX_DIMENSION = 16384;
    private static final long MAX_PIXELS = 16_000_000L;

    private final AssetManager assets;
    private final MainActivity activity;
    private final SparseArray<SpriteAsset> sprites = new SparseArray<>();
    private final SparseArray<SpriteTexture> textures = new SparseArray<>();
    private final SparseBooleanArray failedHandles = new SparseBooleanArray();
    private final SparseArray<TextTexture> textTextures = new SparseArray<>();
    private final SparseArray<FontInfo> fonts = new SparseArray<>();
    private final ArrayList<DynamicTextTexture> dynamicTextTextures = new ArrayList<>();
    private boolean manifestRead;
    private boolean manifestValid;
    private int fallbackTexture;
    private float rasterScale = 1.0f;
    private int densityGeneration = -1;
    private final int[] deletedTexture = new int[1];
    private int surfaceGeneration;
    private int rendererGeneration;
    private int fallbackSurfaceGeneration;
    private int fallbackRendererGeneration;
    private String lastFailure;
    private String transitionReason = "none";

    PublishedSpriteCatalog(MainActivity activity, AssetManager assets) {
        this.activity = activity;
        this.assets = assets;
    }

    @Override
    public void onResourceGenerationChanged(int nextSurfaceGeneration,
            int nextRendererGeneration, boolean discardGpuHandles,
            String nextTransitionReason) {
        clearDensityTextures(!discardGpuHandles);
        surfaceGeneration = nextSurfaceGeneration;
        rendererGeneration = nextRendererGeneration;
        transitionReason = nextTransitionReason;
    }

    @Override
    public void beginRestoreAttempt() {
        lastFailure = null;
        failedHandles.clear();
    }

    @Override
    public String consumeFailure() {
        String failure = lastFailure;
        lastFailure = null;
        return failure;
    }

    @Override
    public void onDisplayMetricsChanged(float nextRasterScale, int nextDensityGeneration) {
        if (densityGeneration == nextDensityGeneration
                && Math.abs(rasterScale - nextRasterScale) < 0.001f) return;
        clearDensityTextures(true);
        rasterScale = nextRasterScale;
        densityGeneration = nextDensityGeneration;
    }

    @Override
    public int textureFor(int handle) {
        SpriteTexture cached = textures.get(handle);
        if (cached != null && cached.matches(surfaceGeneration, rendererGeneration)) {
            return cached.texture;
        }
        if (cached != null) textures.remove(handle);
        if (failedHandles.get(handle)) return fallbackTexture();
        SpriteAsset sprite = null;
        try {
            ensureManifest();
            sprite = sprites.get(handle);
            if (!manifestValid || sprite == null) throw new IOException("sprite handle is not packaged");
            byte[] bytes = readAsset(ROOT + sprite.path, MAX_ASSET_BYTES);
            if (!sprite.sha256.equals(sha256(bytes))) throw new IOException("sprite content hash mismatch");
            Bitmap bitmap = decode(sprite, bytes, rasterScale);
            int texture;
            try {
                texture = upload(bitmap);
            } finally {
                bitmap.recycle();
            }
            textures.put(handle, new SpriteTexture(
                    texture, surfaceGeneration, rendererGeneration));
            return texture;
        } catch (Exception error) {
            failedHandles.put(handle, true);
            recordFailure("sprite", handle,
                    sprite == null ? ROOT : ROOT + sprite.path,
                    sprite == null ? 0 : sprite.width,
                    sprite == null ? 0 : sprite.height, error);
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
        TextTexture cached = textTextures.get(runHandle);
        if (cached != null && cached.matches(surfaceGeneration, rendererGeneration)) {
            return StasisPreviewRenderer.packTexture(
                cached.texture, cached.width, cached.height);
        }
        if (cached != null) textTextures.remove(runHandle);
        try {
            JSONObject resolved = new JSONObject(MainActivity.nativeResolveCachedText("", runHandle));
            if (!"ok".equals(resolved.optString("status"))) {
                throw new IOException(resolved.optString("error", "cached text resolution failed"));
            }
            Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG | Paint.SUBPIXEL_TEXT_FLAG);
            paint.setColor(0xffffffff);
            paint.setTextSize(resolved.getInt("font_size") * rasterScale);
            paint.setTypeface(Typeface.createFromAsset(assets, resolved.getString("font_asset")));
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
            recordFailure("cached_text", runHandle, ROOT + "manifest.json", 0, 0, error);
            return 0L;
        }
    }

    @Override
    public long textTextureFor(int font, ByteBuffer utf8, int offset, int length) {
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
            recordFailure("text", font, ROOT + "manifest.json", 0, 0, error);
            return 0L;
        }
    }

    private FontInfo fontInfo(int handle) throws Exception {
        FontInfo cached = fonts.get(handle);
        if (cached != null) return cached;
        JSONObject resolved = new JSONObject(MainActivity.nativeResolveFont("", handle));
        if (!"ok".equals(resolved.optString("status"))) {
            throw new IOException(resolved.optString("error", "font resolution failed"));
        }
        cached = new FontInfo(Typeface.createFromAsset(
                assets, resolved.getString("font_asset")), resolved.getInt("font_size"));
        fonts.put(handle, cached);
        return cached;
    }

    private static TextTexture rasterText(
            FontInfo font, String text, float rasterScale,
            int surfaceGeneration, int rendererGeneration) throws IOException {
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
        } finally {
            bitmap.recycle();
        }
        return new TextTexture(texture,
                Math.max(1, Math.round(width / rasterScale)),
                Math.max(1, Math.round(height / rasterScale)),
                surfaceGeneration, rendererGeneration);
    }

    private void clearDensityTextures(boolean deleteGpuHandles) {
        for (int index = 0; index < textures.size(); index += 1) {
            if (deleteGpuHandles) deleteTexture(textures.valueAt(index).texture);
        }
        textures.clear();
        failedHandles.clear();
        for (int index = 0; index < textTextures.size(); index += 1) {
            if (deleteGpuHandles) deleteTexture(textTextures.valueAt(index).texture);
        }
        textTextures.clear();
        for (DynamicTextTexture texture : dynamicTextTextures) {
            if (deleteGpuHandles) deleteTexture(texture.texture.texture);
        }
        dynamicTextTextures.clear();
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

    private void ensureManifest() throws Exception {
        if (manifestRead) return;
        manifestRead = true;
        JSONObject root = new JSONObject(new String(readAsset(MANIFEST, MAX_MANIFEST_BYTES), "UTF-8"));
        if (!"stasis-assets".equals(root.getString("schema")) || root.getInt("version") != 1) {
            throw new IOException("unsupported packaged asset manifest");
        }
        JSONArray entries = root.getJSONArray("assets");
        if (entries.length() > MAX_ASSETS) throw new IOException("too many packaged assets");
        for (int index = 0; index < entries.length(); index += 1) {
            JSONObject entry = entries.getJSONObject(index);
            JSONObject format = entry.getJSONObject("format");
            if (!"sprite".equals(format.getString("kind"))) continue;
            String id = entry.getString("id");
            String path = entry.getString("path");
            String encoding = format.getString("encoding");
            String hash = entry.getString("content_sha256");
            int width = format.getInt("width");
            int height = format.getInt("height");
            if (!isValidId(id) || !isSafeAssetPath(path) || !hash.matches("[0-9a-f]{64}")
                    || width <= 0 || height <= 0 || width > MAX_DIMENSION || height > MAX_DIMENSION
                    || (long)width * height > MAX_PIXELS || !encodingMatchesPath(encoding, path)) {
                throw new IOException("invalid packaged sprite metadata");
            }
            int handle = stableHandle("sprite:" + id);
            if (sprites.get(handle) != null) {
                throw new IOException("packaged sprite handle collision");
            }
            sprites.put(handle, new SpriteAsset(path, hash, encoding, width, height));
        }
        manifestValid = true;
    }

    private Bitmap decode(SpriteAsset sprite, byte[] bytes, float rasterScale) throws IOException {
        if ("svg".equals(sprite.encoding)) {
            int width = Math.max(1, (int)Math.ceil(sprite.width * rasterScale));
            int height = Math.max(1, (int)Math.ceil(sprite.height * rasterScale));
            if (width > MAX_DIMENSION || height > MAX_DIMENSION
                    || (long)width * height > MAX_PIXELS) {
                throw new IOException("density-scaled SVG exceeds packaged decode limits");
            }
            int[] argb = MainActivity.nativeDecodeSvgSpriteBytes(bytes, width, height);
            if (argb == null || argb.length != width * height) {
                throw new IOException("could not decode packaged SVG sprite");
            }
            return Bitmap.createBitmap(argb, width, height, Bitmap.Config.ARGB_8888);
        }
        BitmapFactory.Options bounds = new BitmapFactory.Options();
        bounds.inJustDecodeBounds = true;
        BitmapFactory.decodeByteArray(bytes, 0, bytes.length, bounds);
        if (bounds.outWidth != sprite.width || bounds.outHeight != sprite.height) {
            throw new IOException("decoded sprite dimensions do not match manifest");
        }
        BitmapFactory.Options options = new BitmapFactory.Options();
        options.inPreferredConfig = Bitmap.Config.ARGB_8888;
        options.inScaled = false;
        Bitmap bitmap = BitmapFactory.decodeByteArray(bytes, 0, bytes.length, options);
        if (bitmap == null) throw new IOException("could not decode packaged sprite");
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
        GLUtils.texImage2D(GLES20.GL_TEXTURE_2D, 0, bitmap, 0);
        int error = GLES20.glGetError();
        GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, 0);
        if (error != GLES20.GL_NO_ERROR) {
            GLES20.glDeleteTextures(1, names, 0);
            throw new IOException("texture upload failed with GL error " + error);
        }
        return names[0];
    }

    private static int createFallbackTexture() throws IOException {
        ByteBuffer pixels = ByteBuffer.allocateDirect(16);
        pixels.put(new byte[]{(byte)255, 0, (byte)255, (byte)255, 35, 35, 35, (byte)255,
                35, 35, 35, (byte)255, (byte)255, 0, (byte)255, (byte)255});
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

    private byte[] readAsset(String path, int limit) throws IOException {
        try (InputStream input = assets.open(path); ByteArrayOutputStream output = new ByteArrayOutputStream()) {
            byte[] buffer = new byte[8192];
            int total = 0;
            int read;
            while ((read = input.read(buffer)) >= 0) {
                total += read;
                if (total > limit) throw new IOException("packaged asset exceeds byte limit");
                output.write(buffer, 0, read);
            }
            return output.toByteArray();
        }
    }

    private static boolean isSafeAssetPath(String path) {
        return path.startsWith("assets/") && !path.contains("\\") && !path.contains("//")
                && !path.contains("/../") && !path.endsWith("/..") && !path.contains("/./");
    }

    private static boolean isValidId(String id) {
        if (id.isEmpty() || id.length() > 128) return false;
        for (int index = 0; index < id.length(); index += 1) {
            char value = id.charAt(index);
            if (!((value >= 'a' && value <= 'z') || (value >= 'A' && value <= 'Z')
                    || (value >= '0' && value <= '9') || value == '.' || value == '_'
                    || value == '-')) return false;
        }
        return true;
    }

    private static boolean encodingMatchesPath(String encoding, String path) {
        if ("jpeg".equals(encoding)) return path.endsWith(".jpg") || path.endsWith(".jpeg");
        return ("png".equals(encoding) && path.endsWith(".png"))
                || ("svg".equals(encoding) && path.endsWith(".svg"))
                || ("webp".equals(encoding) && path.endsWith(".webp"));
    }

    private static int stableHandle(String value) {
        int hash = 0x811c9dc5;
        for (int index = 0; index < value.length(); index += 1) {
            hash ^= value.charAt(index) & 0xff;
            hash *= 0x01000193;
        }
        return hash == 0 ? 1 : hash;
    }

    private static String sha256(byte[] bytes) throws Exception {
        byte[] digest = MessageDigest.getInstance("SHA-256").digest(bytes);
        StringBuilder out = new StringBuilder(64);
        for (byte value : digest) out.append(String.format("%02x", value & 255));
        return out.toString();
    }

    private static final class SpriteAsset {
        final String path;
        final String sha256;
        final String encoding;
        final int width;
        final int height;

        SpriteAsset(String path, String sha256, String encoding, int width, int height) {
            this.path = path;
            this.sha256 = sha256;
            this.encoding = encoding;
            this.width = width;
            this.height = height;
        }
    }

    private static final class SpriteTexture {
        final int texture;
        final int surfaceGeneration;
        final int rendererGeneration;

        SpriteTexture(int texture, int surfaceGeneration, int rendererGeneration) {
            this.texture = texture;
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
