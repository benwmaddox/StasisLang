package com.stasislang.workshop;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.StandardCopyOption;
import java.security.MessageDigest;
import java.util.Locale;
import java.util.HashMap;

final class WorkshopAssetManifest {
    static final String RELATIVE_PATH = "assets/manifest.json";
    private static final int MAX_MANIFEST_BYTES = 1024 * 1024;

    private WorkshopAssetManifest() {}

    static byte[] readForSync(File root) throws IOException {
        File manifest = confined(root, RELATIVE_PATH);
        if (!manifest.exists()) return null;
        if (!manifest.isFile() || manifest.length() > MAX_MANIFEST_BYTES) {
            throw new IOException("asset manifest exceeds its size limit");
        }
        FileInputStream input = new FileInputStream(manifest);
        try {
            ByteArrayOutputStream output = new ByteArrayOutputStream((int)manifest.length());
            byte[] buffer = new byte[8192];
            int read;
            while ((read = input.read(buffer)) >= 0) {
                if (output.size() + read > MAX_MANIFEST_BYTES) {
                    throw new IOException("asset manifest exceeds its size limit");
                }
                output.write(buffer, 0, read);
            }
            return output.toByteArray();
        } finally {
            input.close();
        }
    }

    static void putSprite(File root, WorkshopImageAssets.AssetInfo asset, String previousPath)
            throws IOException {
        try {
            put(root, asset.file, asset.relativePath, previousPath, "sprite", spriteFormat(asset));
        } catch (Exception error) {
            throw io("could not update sprite manifest", error);
        }
    }

    static void putAudio(File root, WorkshopAudioAssets.AssetInfo asset, String previousPath)
            throws IOException {
        try {
            put(root, asset.file, asset.relativePath, previousPath, "audio", audioFormat(asset));
        } catch (Exception error) {
            throw io("could not update audio manifest", error);
        }
    }

    static void remove(File root, String relativePath) throws IOException {
        try {
            JSONObject manifest = read(root);
            JSONArray current = manifest.getJSONArray("assets");
            seedMissing(root, current);
            JSONArray updated = new JSONArray();
            for (int index = 0; index < current.length(); index++) {
                JSONObject entry = current.getJSONObject(index);
                if (!relativePath.equals(entry.getString("path"))) updated.put(entry);
            }
            manifest.put("assets", updated);
            write(root, manifest);
        } catch (Exception error) {
            throw io("could not remove asset manifest entry", error);
        }
    }

    private static void put(File root, File file, String relativePath, String previousPath,
            String kind, JSONObject format) throws Exception {
        JSONObject manifest = read(root);
        JSONArray assets = manifest.getJSONArray("assets");
        JSONObject match = findByPath(assets, relativePath, previousPath);
        if (match == null) {
            seedMissing(root, assets);
            match = findByPath(assets, relativePath, previousPath);
        }
        if (match == null) {
            match = new JSONObject();
            match.put("id", uniqueId(assets, kind + "." + baseName(file.getName())));
            match.put("dependencies", new JSONArray());
            assets.put(match);
        }
        match.put("path", relativePath);
        match.put("content_sha256", sha256(file));
        match.put("format", format);
        sortAssets(assets);
        write(root, manifest);
    }

    private static JSONObject findByPath(JSONArray assets, String path, String alternate)
            throws Exception {
        for (int index = 0; index < assets.length(); index++) {
            JSONObject entry = assets.getJSONObject(index);
            String candidate = entry.getString("path");
            if (path.equals(candidate) || alternate != null && alternate.equals(candidate)) {
                return entry;
            }
        }
        return null;
    }

    private static void seedMissing(File root, JSONArray assets) throws Exception {
        for (WorkshopImageAssets.AssetInfo image : WorkshopImageAssets.list(root)) {
            if (!containsPath(assets, image.relativePath)) {
                addEntry(assets, image.file, image.relativePath, "sprite", spriteFormat(image));
            }
        }
        for (WorkshopAudioAssets.AssetInfo audio : WorkshopAudioAssets.list(root)) {
            if (!containsPath(assets, audio.relativePath)) {
                addEntry(assets, audio.file, audio.relativePath, "audio", audioFormat(audio));
            }
        }
    }

    private static boolean containsPath(JSONArray assets, String path) throws Exception {
        for (int index = 0; index < assets.length(); index++) {
            if (path.equals(assets.getJSONObject(index).getString("path"))) return true;
        }
        return false;
    }

    private static void addEntry(JSONArray assets, File file, String path, String kind,
            JSONObject format) throws Exception {
        JSONObject entry = new JSONObject();
        entry.put("id", uniqueId(assets, kind + "." + baseName(file.getName())));
        entry.put("path", path);
        entry.put("content_sha256", sha256(file));
        entry.put("format", format);
        entry.put("dependencies", new JSONArray());
        assets.put(entry);
    }

    private static JSONObject spriteFormat(WorkshopImageAssets.AssetInfo asset) throws Exception {
        return new JSONObject().put("kind", "sprite")
                .put("encoding", imageEncoding(asset.file.getName()))
                .put("width", asset.width).put("height", asset.height);
    }

    private static JSONObject audioFormat(WorkshopAudioAssets.AssetInfo asset) throws Exception {
        long frames = Math.max(1L, asset.durationMs * (long)asset.sampleRate / 1000L);
        return new JSONObject().put("kind", "audio")
                .put("encoding", audioEncoding(asset.file.getName()))
                .put("sample_rate", asset.sampleRate).put("channels", asset.channels)
                .put("duration_frames", frames);
    }

    private static JSONObject read(File root) throws Exception {
        File manifest = confined(root, RELATIVE_PATH);
        if (!manifest.exists()) {
            return new JSONObject().put("schema", "stasis-assets").put("version", 1)
                    .put("assets", new JSONArray());
        }
        JSONObject parsed = new JSONObject(new String(readForSync(root), StandardCharsets.UTF_8));
        if (!"stasis-assets".equals(parsed.optString("schema")) || parsed.optInt("version") != 1
                || parsed.optJSONArray("assets") == null) {
            throw new IOException("asset manifest schema or version is unsupported");
        }
        return parsed;
    }

    private static void write(File root, JSONObject manifest) throws Exception {
        validateStableHandles(manifest.getJSONArray("assets"));
        File target = confined(root, RELATIVE_PATH);
        File directory = target.getParentFile();
        if (!directory.isDirectory() && !directory.mkdirs()) {
            throw new IOException("could not create asset manifest directory");
        }
        byte[] bytes = manifest.toString(2).getBytes(StandardCharsets.UTF_8);
        if (bytes.length > MAX_MANIFEST_BYTES) throw new IOException("asset manifest exceeds its size limit");
        File temporary = File.createTempFile(".manifest-", ".tmp", directory);
        try {
            FileOutputStream output = new FileOutputStream(temporary);
            try {
                output.write(bytes);
                output.flush();
                output.getFD().sync();
            } finally {
                output.close();
            }
            Files.move(temporary.toPath(), target.toPath(), StandardCopyOption.ATOMIC_MOVE,
                    StandardCopyOption.REPLACE_EXISTING);
        } finally {
            if (temporary.exists()) temporary.delete();
        }
    }

    private static void validateStableHandles(JSONArray assets) throws Exception {
        HashMap<Integer, String> handles = new HashMap<>();
        for (int index = 0; index < assets.length(); index++) {
            JSONObject entry = assets.getJSONObject(index);
            String id = entry.getString("id");
            String kind = entry.getJSONObject("format").getString("kind");
            int hash = WorkshopAssetIdentity.stableHandle(kind, id);
            String previous = handles.put(hash, id);
            if (previous != null && !previous.equals(id)) {
                throw new IOException("asset stable handle collision between " + previous + " and " + id);
            }
        }
    }

    private static File confined(File root, String relative) throws IOException {
        File target = new File(root, relative.replace('/', File.separatorChar));
        String canonicalRoot = root.getCanonicalPath();
        String canonicalTarget = target.getCanonicalPath();
        if (!canonicalTarget.startsWith(canonicalRoot + File.separator)) {
            throw new IOException("asset manifest path escaped project root");
        }
        return target;
    }

    private static String sha256(File file) throws Exception {
        MessageDigest digest = MessageDigest.getInstance("SHA-256");
        FileInputStream input = new FileInputStream(file);
        try {
            byte[] buffer = new byte[16 * 1024];
            int read;
            while ((read = input.read(buffer)) >= 0) digest.update(buffer, 0, read);
        } finally {
            input.close();
        }
        StringBuilder result = new StringBuilder(64);
        for (byte value : digest.digest()) result.append(String.format(Locale.US, "%02x", value & 0xff));
        return result.toString();
    }

    private static void sortAssets(JSONArray assets) throws Exception {
        java.util.ArrayList<JSONObject> sorted = new java.util.ArrayList<>();
        for (int index = 0; index < assets.length(); index++) sorted.add(assets.getJSONObject(index));
        java.util.Collections.sort(sorted, (left, right) -> left.optString("id").compareTo(right.optString("id")));
        for (int index = assets.length() - 1; index >= 0; index--) assets.remove(index);
        for (JSONObject entry : sorted) assets.put(entry);
    }

    private static String uniqueId(JSONArray assets, String requested) throws Exception {
        String base = requested.replaceAll("[^A-Za-z0-9_.-]", "_");
        for (int suffix = 0; suffix <= 999; suffix++) {
            String candidate = suffix == 0 ? base : base + "." + suffix;
            boolean used = false;
            for (int index = 0; index < assets.length(); index++) {
                if (candidate.equals(assets.getJSONObject(index).optString("id"))) { used = true; break; }
            }
            if (!used) return candidate;
        }
        throw new IOException("too many asset IDs share this name");
    }

    private static String baseName(String name) {
        int dot = name.lastIndexOf('.');
        return dot <= 0 ? name : name.substring(0, dot);
    }

    private static String imageEncoding(String name) throws IOException {
        String lower = name.toLowerCase(Locale.US);
        if (lower.endsWith(".png")) return "png";
        if (lower.endsWith(".jpg") || lower.endsWith(".jpeg")) return "jpeg";
        if (lower.endsWith(".webp")) return "webp";
        throw new IOException("unsupported sprite manifest format");
    }

    private static String audioEncoding(String name) throws IOException {
        String lower = name.toLowerCase(Locale.US);
        if (lower.endsWith(".wav")) return "wav";
        if (lower.endsWith(".ogg")) return "ogg";
        if (lower.endsWith(".mp3")) return "mp3";
        if (lower.endsWith(".m4a")) return "m4a";
        throw new IOException("unsupported audio manifest format");
    }

    private static IOException io(String message, Exception error) {
        return error instanceof IOException ? (IOException)error : new IOException(message + ": " + error.getMessage());
    }
}
