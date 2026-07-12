package com.stasislang.workshop;

import android.content.res.AssetManager;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.opengl.GLES20;
import android.opengl.GLUtils;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.nio.ByteBuffer;
import java.security.MessageDigest;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;
import java.util.Set;

final class PublishedSpriteCatalog {
    private static final String ROOT = "stasis_game/";
    private static final String MANIFEST = ROOT + "assets/manifest.json";
    private static final int MAX_MANIFEST_BYTES = 1024 * 1024;
    private static final int MAX_ASSET_BYTES = 64 * 1024 * 1024;
    private static final int MAX_ASSETS = 4096;
    private static final int MAX_DIMENSION = 16384;
    private static final long MAX_PIXELS = 16_000_000L;

    private final AssetManager assets;
    private final Map<Integer, SpriteAsset> sprites = new HashMap<>();
    private final Map<Integer, Integer> textures = new HashMap<>();
    private final Set<Integer> failedHandles = new HashSet<>();
    private boolean manifestRead;
    private boolean manifestValid;
    private int fallbackTexture;

    PublishedSpriteCatalog(AssetManager assets) {
        this.assets = assets;
    }

    void onSurfaceCreated() {
        textures.clear();
        failedHandles.clear();
        fallbackTexture = createFallbackTexture();
    }

    int textureFor(int handle) {
        Integer cached = textures.get(handle);
        if (cached != null) return cached;
        if (failedHandles.contains(handle)) return fallbackTexture;
        try {
            ensureManifest();
            SpriteAsset sprite = sprites.get(handle);
            if (!manifestValid || sprite == null) throw new IOException("sprite handle is not packaged");
            byte[] bytes = readAsset(ROOT + sprite.path, MAX_ASSET_BYTES);
            if (!sprite.sha256.equals(sha256(bytes))) throw new IOException("sprite content hash mismatch");
            Bitmap bitmap = decode(sprite, bytes);
            int texture;
            try {
                texture = upload(bitmap);
            } finally {
                bitmap.recycle();
            }
            textures.put(handle, texture);
            return texture;
        } catch (Exception error) {
            failedHandles.add(handle);
            return fallbackTexture;
        }
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
            if (sprites.put(handle, new SpriteAsset(path, hash, encoding, width, height)) != null) {
                throw new IOException("packaged sprite handle collision");
            }
        }
        manifestValid = true;
    }

    private Bitmap decode(SpriteAsset sprite, byte[] bytes) throws IOException {
        if ("svg".equals(sprite.encoding)) {
            int[] argb = MainActivity.nativeDecodeSvgSpriteBytes(bytes, sprite.width, sprite.height);
            if (argb == null || argb.length != sprite.width * sprite.height) {
                throw new IOException("could not decode packaged SVG sprite");
            }
            return Bitmap.createBitmap(argb, sprite.width, sprite.height, Bitmap.Config.ARGB_8888);
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

    private static int createFallbackTexture() {
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
        GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, 0);
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
}
