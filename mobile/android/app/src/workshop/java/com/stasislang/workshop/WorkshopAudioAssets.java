package com.stasislang.workshop;

import android.content.ContentResolver;
import android.database.Cursor;
import android.media.MediaMetadataRetriever;
import android.net.Uri;
import android.provider.OpenableColumns;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.List;

final class WorkshopAudioAssets {
    static final int MAX_AUDIO_BYTES = 16 * 1024 * 1024;
    static final long MAX_DURATION_MS = 5L * 60L * 1000L;
    private static final int MAX_TRASH_FILES = 20;
    private static final String AUDIO_DIRECTORY = "assets/audio";
    private static final String TRASH_DIRECTORY = ".stasis-trash/audio";

    static final class AssetInfo {
        final File file;
        final String relativePath;
        final long bytes;
        final long durationMs;
        final String mimeType;

        AssetInfo(File file, String relativePath, long bytes, long durationMs, String mimeType) {
            this.file = file;
            this.relativePath = relativePath;
            this.bytes = bytes;
            this.durationMs = durationMs;
            this.mimeType = mimeType;
        }
    }

    private WorkshopAudioAssets() {}

    static AssetInfo importAudio(ContentResolver resolver, Uri source, File projectRoot) throws IOException {
        String mimeType = normalizedMimeType(resolver.getType(source));
        String extension = extensionFor(mimeType);
        byte[] encoded = readBounded(resolver, source);
        File directory = confinedAudioDirectory(projectRoot);
        if (!directory.isDirectory() && !directory.mkdirs()) throw new IOException("could not create project audio directory");
        File target = uniqueTarget(directory, safeBaseName(displayName(resolver, source)), extension);
        requireInside(projectRoot, target);
        File temporary = File.createTempFile(".import-", extension + ".tmp", directory);
        boolean published = false;
        try {
            writeSynced(temporary, encoded);
            inspect(projectRoot, temporary, mimeType);
            if (!temporary.renameTo(target)) throw new IOException("could not publish imported audio");
            published = true;
            return inspect(projectRoot, target, mimeType);
        } finally {
            if (!published && temporary.exists()) temporary.delete();
        }
    }

    static List<AssetInfo> list(File projectRoot) throws IOException {
        File directory = confinedAudioDirectory(projectRoot);
        File[] files = directory.listFiles();
        if (files == null) return Collections.emptyList();
        ArrayList<AssetInfo> assets = new ArrayList<>();
        for (File file : files) {
            if (!file.isFile() || file.getName().startsWith(".")) continue;
            String mime = mimeForExtension(file.getName());
            if (mime.isEmpty()) continue;
            try {
                assets.add(inspect(projectRoot, file, mime));
            } catch (IOException ignored) {
                // Invalid files are not exposed as playable project assets.
            }
        }
        Collections.sort(assets, new Comparator<AssetInfo>() {
            @Override public int compare(AssetInfo left, AssetInfo right) {
                return left.relativePath.compareTo(right.relativePath);
            }
        });
        return assets;
    }

    static byte[] readForSync(AssetInfo asset) throws IOException {
        if (asset.bytes > MAX_AUDIO_BYTES) throw new IOException(asset.relativePath + " exceeds the audio sync limit");
        FileInputStream input = new FileInputStream(asset.file);
        try {
            ByteArrayOutputStream output = new ByteArrayOutputStream((int)asset.bytes);
            byte[] buffer = new byte[16 * 1024];
            int total = 0;
            int read;
            while ((read = input.read(buffer)) >= 0) {
                total += read;
                if (total > MAX_AUDIO_BYTES) throw new IOException(asset.relativePath + " exceeds the audio sync limit");
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
        String base = normalizeRequestedBase(requestedName, extension);
        File target = new File(asset.file.getParentFile(), base + extension);
        requireInside(projectRoot, target);
        if (target.equals(asset.file)) return asset;
        if (target.exists()) throw new IOException("an audio asset already uses that name");
        if (!asset.file.renameTo(target)) throw new IOException("could not rename audio asset");
        return inspect(projectRoot, target, asset.mimeType);
    }

    static void moveToTrash(AssetInfo asset, File projectRoot) throws IOException {
        requireInside(projectRoot, asset.file);
        File trash = new File(projectRoot, TRASH_DIRECTORY);
        requireInside(projectRoot, trash);
        if (!trash.isDirectory() && !trash.mkdirs()) throw new IOException("could not create audio recovery directory");
        String extension = fileExtension(asset.file.getName());
        File target = uniqueTarget(trash, Long.toString(System.currentTimeMillis()) + "-"
                + baseWithoutExtension(asset.file.getName()), extension);
        if (!asset.file.renameTo(target)) throw new IOException("could not move audio to recovery");
        pruneTrash(trash);
    }

    static AssetInfo restoreLatest(File projectRoot) throws IOException {
        File trash = new File(projectRoot, TRASH_DIRECTORY);
        requireInside(projectRoot, trash);
        File[] files = trash.listFiles();
        if (files == null || files.length == 0) throw new IOException("no deleted audio is available to restore");
        File latest = newestFile(files);
        String original = latest.getName().replaceFirst("^[0-9]+-", "");
        String extension = fileExtension(original);
        String mime = mimeForExtension(original);
        if (mime.isEmpty()) throw new IOException("deleted audio format is unsupported");
        File directory = confinedAudioDirectory(projectRoot);
        if (!directory.isDirectory() && !directory.mkdirs()) throw new IOException("could not create project audio directory");
        File target = uniqueTarget(directory, baseWithoutExtension(original), extension);
        if (!latest.renameTo(target)) throw new IOException("could not restore deleted audio");
        return inspect(projectRoot, target, mime);
    }

    private static AssetInfo inspect(File projectRoot, File file, String fallbackMime) throws IOException {
        requireInside(projectRoot, file);
        if (file.length() <= 0 || file.length() > MAX_AUDIO_BYTES) throw new IOException("audio exceeds the 16 MiB asset limit");
        MediaMetadataRetriever metadata = new MediaMetadataRetriever();
        try {
            metadata.setDataSource(file.getAbsolutePath());
            String durationText = metadata.extractMetadata(MediaMetadataRetriever.METADATA_KEY_DURATION);
            long duration = durationText == null ? -1L : Long.parseLong(durationText);
            if (duration <= 0L || duration > MAX_DURATION_MS) {
                throw new IOException("audio duration must be between 1 ms and 5 minutes");
            }
            String detected = normalizedMimeType(
                    metadata.extractMetadata(MediaMetadataRetriever.METADATA_KEY_MIMETYPE));
            if (detected.isEmpty()) detected = fallbackMime;
            extensionFor(detected);
            return new AssetInfo(file, relativePath(projectRoot, file), file.length(), duration, detected);
        } catch (NumberFormatException error) {
            throw new IOException("audio duration metadata is invalid");
        } catch (RuntimeException error) {
            throw new IOException("audio could not be decoded: " + error.getMessage());
        } finally {
            metadata.release();
        }
    }

    private static byte[] readBounded(ContentResolver resolver, Uri source) throws IOException {
        InputStream input = resolver.openInputStream(source);
        if (input == null) throw new IOException("document provider did not open the audio");
        try {
            ByteArrayOutputStream output = new ByteArrayOutputStream();
            byte[] buffer = new byte[16 * 1024];
            int total = 0;
            int read;
            while ((read = input.read(buffer)) >= 0) {
                total += read;
                if (total > MAX_AUDIO_BYTES) throw new IOException("audio exceeds the 16 MiB import limit");
                output.write(buffer, 0, read);
            }
            return output.toByteArray();
        } finally {
            input.close();
        }
    }

    private static void writeSynced(File target, byte[] bytes) throws IOException {
        FileOutputStream output = new FileOutputStream(target);
        try {
            output.write(bytes);
            output.flush();
            output.getFD().sync();
        } finally {
            output.close();
        }
    }

    private static String extensionFor(String mime) throws IOException {
        if ("audio/mpeg".equals(mime)) return ".mp3";
        if ("audio/ogg".equals(mime)) return ".ogg";
        if ("audio/wav".equals(mime)) return ".wav";
        if ("audio/mp4".equals(mime)) return ".m4a";
        throw new IOException("only MP3, Ogg, WAV, and M4A audio is supported");
    }

    private static String normalizedMimeType(String mime) {
        if (mime == null) return "";
        String value = mime.toLowerCase();
        if (value.equals("audio/x-wav") || value.equals("audio/wave")) return "audio/wav";
        if (value.equals("audio/x-m4a") || value.equals("audio/aac")) return "audio/mp4";
        if (value.equals("application/ogg")) return "audio/ogg";
        return value;
    }

    private static String mimeForExtension(String name) {
        String extension = fileExtension(name);
        if (".mp3".equals(extension)) return "audio/mpeg";
        if (".ogg".equals(extension)) return "audio/ogg";
        if (".wav".equals(extension)) return "audio/wav";
        if (".m4a".equals(extension)) return "audio/mp4";
        return "";
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
        } finally {
            if (cursor != null) cursor.close();
        }
        return "audio";
    }

    private static String safeBaseName(String displayName) {
        String name = displayName == null ? "audio" : baseWithoutExtension(displayName.trim());
        name = name.replaceAll("[^A-Za-z0-9_-]+", "_").replaceAll("^_+|_+$", "");
        if (name.isEmpty()) name = "audio";
        return name.length() > 64 ? name.substring(0, 64) : name;
    }

    private static String normalizeRequestedBase(String requestedName, String extension) throws IOException {
        String base = requestedName == null ? "" : requestedName.trim();
        if (!extension.isEmpty() && base.toLowerCase().endsWith(extension)) {
            base = base.substring(0, base.length() - extension.length());
        }
        if (!base.matches("[A-Za-z0-9][A-Za-z0-9_-]{0,63}")) {
            throw new IOException("audio name must use 1-64 letters, numbers, underscores, or hyphens");
        }
        return base;
    }

    private static File uniqueTarget(File directory, String base, String extension) throws IOException {
        for (int suffix = 0; suffix <= 999; suffix++) {
            File candidate = new File(directory, suffix == 0 ? base + extension : base + "_" + suffix + extension);
            if (!candidate.exists()) return candidate;
        }
        throw new IOException("too many audio assets share this name");
    }

    private static File confinedAudioDirectory(File projectRoot) throws IOException {
        File directory = new File(projectRoot, AUDIO_DIRECTORY);
        requireInside(projectRoot, directory);
        return directory;
    }

    private static void requireInside(File projectRoot, File target) throws IOException {
        String root = projectRoot.getCanonicalPath();
        String path = target.getCanonicalPath();
        if (!path.startsWith(root + File.separator)) throw new IOException("audio path escapes the active project");
    }

    private static String relativePath(File projectRoot, File target) throws IOException {
        String root = projectRoot.getCanonicalPath();
        requireInside(projectRoot, target);
        return target.getCanonicalPath().substring(root.length() + 1).replace(File.separatorChar, '/');
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
        for (File file : files) if (file.isFile() && (newest == null || file.getName().compareTo(newest.getName()) > 0)) newest = file;
        if (newest == null) throw new IOException("no deleted audio is available to restore");
        return newest;
    }

    private static void pruneTrash(File trash) {
        File[] files = trash.listFiles();
        if (files == null || files.length <= MAX_TRASH_FILES) return;
        ArrayList<File> ordered = new ArrayList<>();
        for (File file : files) if (file.isFile()) ordered.add(file);
        Collections.sort(ordered, new Comparator<File>() {
            @Override public int compare(File left, File right) { return left.getName().compareTo(right.getName()); }
        });
        for (int index = 0; index < ordered.size() - MAX_TRASH_FILES; index++) ordered.get(index).delete();
    }
}
