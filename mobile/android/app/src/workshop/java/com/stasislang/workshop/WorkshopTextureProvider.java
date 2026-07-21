package com.stasislang.workshop;

import android.graphics.Bitmap;
import android.opengl.GLES20;
import android.util.SparseArray;

import org.json.JSONObject;

import java.io.File;
import java.io.IOException;
import java.nio.ByteBuffer;

final class WorkshopTextureProvider implements StasisPreviewRenderer.TextureProvider {
    private final MainActivity activity;
    private final SparseArray<SpriteTexture> textures = new SparseArray<>();
    private final int[] deletedTexture = new int[1];
    private File manifest;
    private String projectRootPath;
    private int fallbackTexture;
    private long manifestStamp = Long.MIN_VALUE;

    WorkshopTextureProvider(MainActivity activity) {
        this.activity = activity;
    }

    @Override
    public void onSurfaceCreated() {
        textures.clear();
        setProjectRoot(activity.projectRootPath());
        manifestStamp = Long.MIN_VALUE;
        fallbackTexture = createFallbackTexture();
    }

    @Override
    public int textureFor(int handle) {
        String currentProjectRoot = activity.projectRootPath();
        if (projectChanged(projectRootPath, currentProjectRoot)) {
            clearTextures();
            setProjectRoot(currentProjectRoot);
        }
        long currentStamp = manifest.isFile()
                ? manifest.lastModified() ^ (manifest.length() << 7) : 0L;
        if (currentStamp != manifestStamp) manifestStamp = currentStamp;
        SpriteTexture cached = textures.get(handle);
        if (cached != null && cached.checkedManifestStamp == manifestStamp) {
            return cached.texture;
        }
        try {
            JSONObject resolved = new JSONObject(MainActivity.nativeResolveSpriteAsset(
                    currentProjectRoot, handle));
            if (!"ok".equals(resolved.optString("status"))) {
                throw new IOException(resolved.optString("error", "sprite resolution failed"));
            }
            String hash = resolved.getString("content_sha256");
            if (cached != null && hash.equals(cached.contentHash)) {
                cached.checkedManifestStamp = manifestStamp;
                return cached.texture;
            }
            Bitmap bitmap = decode(resolved);
            int uploaded;
            try {
                uploaded = upload(bitmap);
            } finally {
                bitmap.recycle();
            }
            textures.put(handle, new SpriteTexture(uploaded, hash, manifestStamp));
            if (cached != null) deleteTexture(cached.texture);
            return uploaded;
        } catch (Exception error) {
            if (cached != null) {
                cached.checkedManifestStamp = manifestStamp;
                return cached.texture;
            }
            return fallbackTexture;
        }
    }

    static boolean projectChanged(String boundRoot, String currentRoot) {
        return boundRoot == null || !boundRoot.equals(currentRoot);
    }

    private void setProjectRoot(String root) {
        projectRootPath = root;
        manifest = new File(root, WorkshopAssetManifest.RELATIVE_PATH);
        manifestStamp = Long.MIN_VALUE;
    }

    private void clearTextures() {
        for (int index = 0; index < textures.size(); index++) {
            deleteTexture(textures.valueAt(index).texture);
        }
        textures.clear();
    }

    private void deleteTexture(int texture) {
        deletedTexture[0] = texture;
        GLES20.glDeleteTextures(1, deletedTexture, 0);
    }

    private static Bitmap decode(JSONObject resolved) throws Exception {
        String encoding = resolved.getString("encoding");
        int width = resolved.getInt("width");
        int height = resolved.getInt("height");
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

    private static int createFallbackTexture() {
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
        GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, 0);
        return names[0];
    }

    private static final class SpriteTexture {
        final int texture;
        final String contentHash;
        long checkedManifestStamp;

        SpriteTexture(int texture, String contentHash, long checkedManifestStamp) {
            this.texture = texture;
            this.contentHash = contentHash;
            this.checkedManifestStamp = checkedManifestStamp;
        }
    }
}
