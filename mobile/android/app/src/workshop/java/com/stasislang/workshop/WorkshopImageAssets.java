package com.stasislang.workshop;

import android.content.ContentResolver;
import android.database.Cursor;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.net.Uri;
import android.provider.OpenableColumns;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileOutputStream;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.List;

final class WorkshopImageAssets {
    static final int MAX_IMPORT_BYTES = 8 * 1024 * 1024;
    static final int MAX_DIMENSION = 4096;
    static final long MAX_PIXELS = 16_000_000L;
    private static final int MAX_PREVIEW_DIMENSION = 640;
    private static final int MAX_TRASH_FILES = 20;
    private static final String IMAGE_DIRECTORY = "assets/images";
    private static final String TRASH_DIRECTORY = ".stasis-trash/images";

    static final class AssetInfo {
        final File file;
        final String relativePath;
        final int width;
        final int height;
        final long bytes;

        AssetInfo(File file, String relativePath, int width, int height, long bytes) {
            this.file = file;
            this.relativePath = relativePath;
            this.width = width;
            this.height = height;
            this.bytes = bytes;
        }
    }

    private WorkshopImageAssets() {}

    static AssetInfo importImage(ContentResolver resolver, Uri source, File projectRoot) throws IOException {
        byte[] encoded = readBounded(resolver, source);
        BitmapFactory.Options bounds = decodeBounds(encoded);
        String extension = extensionFor(bounds.outMimeType);
        validateBounds(bounds);

        File directory = confinedImageDirectory(projectRoot);
        if (!directory.isDirectory() && !directory.mkdirs()) {
            throw new IOException("could not create project image directory");
        }
        String baseName = safeBaseName(displayName(resolver, source));
        File target = uniqueTarget(directory, baseName, extension);
        requireInside(projectRoot, target);
        File temporary = File.createTempFile(".import-", ".tmp", directory);
        boolean published = false;
        try {
            FileOutputStream output = new FileOutputStream(temporary);
            try {
                output.write(encoded);
                output.flush();
                output.getFD().sync();
            } finally {
                output.close();
            }
            if (!temporary.renameTo(target)) {
                throw new IOException("could not publish imported image");
            }
            published = true;
            return new AssetInfo(target, relativePath(projectRoot, target), bounds.outWidth,
                    bounds.outHeight, target.length());
        } finally {
            if (!published && temporary.exists()) temporary.delete();
        }
    }

    static List<AssetInfo> list(File projectRoot) throws IOException {
        File directory = confinedImageDirectory(projectRoot);
        File[] files = directory.listFiles();
        if (files == null) return Collections.emptyList();
        ArrayList<AssetInfo> assets = new ArrayList<>();
        for (File file : files) {
            if (!file.isFile() || file.getName().startsWith(".")) continue;
            requireInside(projectRoot, file);
            BitmapFactory.Options bounds = new BitmapFactory.Options();
            bounds.inJustDecodeBounds = true;
            BitmapFactory.decodeFile(file.getAbsolutePath(), bounds);
            if (bounds.outWidth <= 0 || bounds.outHeight <= 0) continue;
            assets.add(new AssetInfo(file, relativePath(projectRoot, file), bounds.outWidth,
                    bounds.outHeight, file.length()));
        }
        Collections.sort(assets, new Comparator<AssetInfo>() {
            @Override public int compare(AssetInfo left, AssetInfo right) {
                return left.relativePath.compareTo(right.relativePath);
            }
        });
        return assets;
    }

    static Bitmap decodePreview(AssetInfo asset) throws IOException {
        BitmapFactory.Options bounds = new BitmapFactory.Options();
        bounds.inJustDecodeBounds = true;
        BitmapFactory.decodeFile(asset.file.getAbsolutePath(), bounds);
        if (bounds.outWidth <= 0 || bounds.outHeight <= 0) {
            throw new IOException("image preview could not be decoded");
        }
        int sample = 1;
        while (bounds.outWidth / sample > MAX_PREVIEW_DIMENSION
                || bounds.outHeight / sample > MAX_PREVIEW_DIMENSION) {
            sample *= 2;
        }
        BitmapFactory.Options options = new BitmapFactory.Options();
        options.inSampleSize = sample;
        Bitmap bitmap = BitmapFactory.decodeFile(asset.file.getAbsolutePath(), options);
        if (bitmap == null) throw new IOException("image preview could not be decoded");
        return bitmap;
    }

    static byte[] readForSync(AssetInfo asset) throws IOException {
        if (asset.bytes > MAX_IMPORT_BYTES) {
            throw new IOException(asset.relativePath + " exceeds the image sync limit");
        }
        FileInputStream input = new FileInputStream(asset.file);
        try {
            ByteArrayOutputStream output = new ByteArrayOutputStream((int) asset.bytes);
            byte[] buffer = new byte[16 * 1024];
            int total = 0;
            int read;
            while ((read = input.read(buffer)) >= 0) {
                total += read;
                if (total > MAX_IMPORT_BYTES) {
                    throw new IOException(asset.relativePath + " exceeds the image sync limit");
                }
                output.write(buffer, 0, read);
            }
            return output.toByteArray();
        } finally {
            input.close();
        }
    }

    static AssetInfo rename(AssetInfo asset, File projectRoot, String requestedName) throws IOException {
        requireInside(projectRoot, asset.file);
        String extension = fileExtension(asset.file.getName());
        String base = requestedName == null ? "" : requestedName.trim();
        if (!extension.isEmpty() && base.toLowerCase().endsWith(extension)) {
            base = base.substring(0, base.length() - extension.length());
        }
        if (!base.matches("[A-Za-z0-9][A-Za-z0-9_-]{0,63}")) {
            throw new IOException("image name must use 1-64 letters, numbers, underscores, or hyphens");
        }
        File target = new File(asset.file.getParentFile(), base + extension);
        requireInside(projectRoot, target);
        if (target.equals(asset.file)) return asset;
        if (target.exists()) throw new IOException("an image already uses that name");
        if (!asset.file.renameTo(target)) throw new IOException("could not rename image");
        return inspect(projectRoot, target);
    }

    static void moveToTrash(AssetInfo asset, File projectRoot) throws IOException {
        requireInside(projectRoot, asset.file);
        File trash = new File(projectRoot, TRASH_DIRECTORY);
        requireInside(projectRoot, trash);
        if (!trash.isDirectory() && !trash.mkdirs()) throw new IOException("could not create image recovery directory");
        File target = uniqueTarget(trash, Long.toString(System.currentTimeMillis()) + "-"
                + baseWithoutExtension(asset.file.getName()), fileExtension(asset.file.getName()));
        if (!asset.file.renameTo(target)) throw new IOException("could not move image to recovery");
        pruneTrash(trash);
    }

    static AssetInfo restoreLatest(File projectRoot) throws IOException {
        File trash = new File(projectRoot, TRASH_DIRECTORY);
        requireInside(projectRoot, trash);
        File[] files = trash.listFiles();
        if (files == null || files.length == 0) throw new IOException("no deleted image is available to restore");
        File latest = newestFile(files);
        String original = latest.getName().replaceFirst("^[0-9]+-", "");
        String extension = fileExtension(original);
        File directory = confinedImageDirectory(projectRoot);
        if (!directory.isDirectory() && !directory.mkdirs()) throw new IOException("could not create project image directory");
        File target = uniqueTarget(directory, baseWithoutExtension(original), extension);
        if (!latest.renameTo(target)) throw new IOException("could not restore deleted image");
        return inspect(projectRoot, target);
    }

    static AssetInfo savePainted(Bitmap bitmap, File projectRoot, String requestedName) throws IOException {
        if (bitmap == null || bitmap.getWidth() > WorkshopPaintView.MAX_CANVAS_DIMENSION
                || bitmap.getHeight() > WorkshopPaintView.MAX_CANVAS_DIMENSION) {
            throw new IOException("painted image exceeds the canvas limit");
        }
        String base = requestedName == null ? "" : requestedName.trim();
        if (base.toLowerCase().endsWith(".png")) base = base.substring(0, base.length() - 4);
        if (!base.matches("[A-Za-z0-9][A-Za-z0-9_-]{0,63}")) {
            throw new IOException("image name must use 1-64 letters, numbers, underscores, or hyphens");
        }
        File directory = confinedImageDirectory(projectRoot);
        if (!directory.isDirectory() && !directory.mkdirs()) throw new IOException("could not create project image directory");
        File target = uniqueTarget(directory, base, ".png");
        requireInside(projectRoot, target);
        File temporary = File.createTempFile(".paint-", ".tmp", directory);
        boolean published = false;
        try {
            FileOutputStream output = new FileOutputStream(temporary);
            try {
                if (!bitmap.compress(Bitmap.CompressFormat.PNG, 100, output)) {
                    throw new IOException("could not encode painted image");
                }
                output.flush();
                output.getFD().sync();
            } finally {
                output.close();
            }
            if (temporary.length() > MAX_IMPORT_BYTES) throw new IOException("painted image exceeds the 8 MiB asset limit");
            if (!temporary.renameTo(target)) throw new IOException("could not publish painted image");
            published = true;
            return inspect(projectRoot, target);
        } finally {
            if (!published && temporary.exists()) temporary.delete();
        }
    }

    static Bitmap decodeForPaint(AssetInfo asset) throws IOException {
        if (asset.width > WorkshopPaintView.MAX_CANVAS_DIMENSION
                || asset.height > WorkshopPaintView.MAX_CANVAS_DIMENSION) {
            throw new IOException("paint editing supports images up to 1024x1024");
        }
        Bitmap decoded = BitmapFactory.decodeFile(asset.file.getAbsolutePath());
        if (decoded == null) throw new IOException("image could not be decoded for painting");
        Bitmap mutable = decoded.copy(Bitmap.Config.ARGB_8888, true);
        decoded.recycle();
        return mutable;
    }

    private static byte[] readBounded(ContentResolver resolver, Uri source) throws IOException {
        InputStream input = resolver.openInputStream(source);
        if (input == null) throw new IOException("document provider did not open the image");
        try {
            ByteArrayOutputStream output = new ByteArrayOutputStream();
            byte[] buffer = new byte[16 * 1024];
            int total = 0;
            int read;
            while ((read = input.read(buffer)) >= 0) {
                total += read;
                if (total > MAX_IMPORT_BYTES) {
                    throw new IOException("image exceeds the 8 MiB import limit");
                }
                output.write(buffer, 0, read);
            }
            return output.toByteArray();
        } finally {
            input.close();
        }
    }

    private static BitmapFactory.Options decodeBounds(byte[] encoded) throws IOException {
        BitmapFactory.Options bounds = new BitmapFactory.Options();
        bounds.inJustDecodeBounds = true;
        BitmapFactory.decodeByteArray(encoded, 0, encoded.length, bounds);
        if (bounds.outWidth <= 0 || bounds.outHeight <= 0) {
            throw new IOException("selected document is not a supported image");
        }
        return bounds;
    }

    private static void validateBounds(BitmapFactory.Options bounds) throws IOException {
        long pixels = (long) bounds.outWidth * (long) bounds.outHeight;
        if (bounds.outWidth > MAX_DIMENSION || bounds.outHeight > MAX_DIMENSION || pixels > MAX_PIXELS) {
            throw new IOException("image exceeds the 4096 px or 16 megapixel import limit");
        }
    }

    private static String extensionFor(String mimeType) throws IOException {
        if ("image/png".equals(mimeType)) return ".png";
        if ("image/jpeg".equals(mimeType)) return ".jpg";
        if ("image/webp".equals(mimeType)) return ".webp";
        throw new IOException("only PNG, JPEG, and WebP images are supported");
    }

    private static String displayName(ContentResolver resolver, Uri source) {
        Cursor cursor = null;
        try {
            cursor = resolver.query(source, new String[] {OpenableColumns.DISPLAY_NAME}, null, null, null);
            if (cursor != null && cursor.moveToFirst()) {
                int column = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME);
                if (column >= 0) return cursor.getString(column);
            }
        } catch (RuntimeException ignored) {
            // The provider name is optional; a deterministic fallback is used below.
        } finally {
            if (cursor != null) cursor.close();
        }
        return "image";
    }

    private static String safeBaseName(String displayName) {
        String name = displayName == null ? "image" : displayName.trim();
        int dot = name.lastIndexOf('.');
        if (dot > 0) name = name.substring(0, dot);
        name = name.replaceAll("[^A-Za-z0-9_-]+", "_");
        name = name.replaceAll("^_+|_+$", "");
        if (name.isEmpty()) name = "image";
        return name.length() > 64 ? name.substring(0, 64) : name;
    }

    private static File uniqueTarget(File directory, String baseName, String extension) throws IOException {
        for (int suffix = 0; suffix <= 999; suffix++) {
            String name = suffix == 0 ? baseName + extension : baseName + "_" + suffix + extension;
            File candidate = new File(directory, name);
            if (!candidate.exists()) return candidate;
        }
        throw new IOException("too many images share this name");
    }

    private static AssetInfo inspect(File projectRoot, File file) throws IOException {
        BitmapFactory.Options bounds = new BitmapFactory.Options();
        bounds.inJustDecodeBounds = true;
        BitmapFactory.decodeFile(file.getAbsolutePath(), bounds);
        if (bounds.outWidth <= 0 || bounds.outHeight <= 0) throw new IOException("image could not be decoded");
        return new AssetInfo(file, relativePath(projectRoot, file), bounds.outWidth, bounds.outHeight, file.length());
    }

    private static String fileExtension(String name) {
        int dot = name.lastIndexOf('.');
        return dot < 0 ? "" : name.substring(dot).toLowerCase();
    }

    private static String baseWithoutExtension(String name) {
        int dot = name.lastIndexOf('.');
        return dot <= 0 ? name : name.substring(0, dot);
    }

    private static File newestFile(File[] files) throws IOException {
        File newest = null;
        for (File file : files) {
            if (!file.isFile()) continue;
            if (newest == null || file.getName().compareTo(newest.getName()) > 0) newest = file;
        }
        if (newest == null) throw new IOException("no deleted image is available to restore");
        return newest;
    }

    private static void pruneTrash(File trash) {
        File[] files = trash.listFiles();
        if (files == null || files.length <= MAX_TRASH_FILES) return;
        ArrayList<File> ordered = new ArrayList<>();
        for (File file : files) if (file.isFile()) ordered.add(file);
        Collections.sort(ordered, new Comparator<File>() {
            @Override public int compare(File left, File right) {
                return left.getName().compareTo(right.getName());
            }
        });
        for (int index = 0; index < ordered.size() - MAX_TRASH_FILES; index++) ordered.get(index).delete();
    }

    private static File confinedImageDirectory(File projectRoot) throws IOException {
        File directory = new File(projectRoot, IMAGE_DIRECTORY);
        requireInside(projectRoot, directory);
        return directory;
    }

    private static void requireInside(File projectRoot, File target) throws IOException {
        String root = projectRoot.getCanonicalPath();
        String path = target.getCanonicalPath();
        if (!path.startsWith(root + File.separator)) {
            throw new IOException("image path escapes the active project");
        }
    }

    private static String relativePath(File projectRoot, File target) throws IOException {
        String root = projectRoot.getCanonicalPath();
        String path = target.getCanonicalPath();
        requireInside(projectRoot, target);
        return path.substring(root.length() + 1).replace(File.separatorChar, '/');
    }
}
