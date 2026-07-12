package com.stasislang.workshop;

import java.io.File;
import java.io.FileOutputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.StandardCopyOption;
import java.util.ArrayList;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

final class WorkshopAiProjectTransaction {
    static final class Snapshot {
        final Map<String, String> editableFiles;

        Snapshot(Map<String, String> editableFiles) {
            this.editableFiles = Collections.unmodifiableMap(
                    new LinkedHashMap<String, String>(editableFiles));
        }
    }

    private WorkshopAiProjectTransaction() {}

    static Snapshot capture(File projectRoot) throws Exception {
        requireRoot(projectRoot);
        Map<String, String> files = new LinkedHashMap<>();
        for (File file : editableFiles(projectRoot)) {
            files.put(relativePath(projectRoot, file), new String(
                    Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8));
        }
        return new Snapshot(files);
    }

    static void restore(File projectRoot, Snapshot snapshot) throws Exception {
        requireRoot(projectRoot);
        if (snapshot == null) throw new IllegalArgumentException("AI transaction snapshot is missing");
        for (File current : editableFiles(projectRoot)) {
            String relative = relativePath(projectRoot, current);
            if (!snapshot.editableFiles.containsKey(relative)
                    && !current.delete() && current.exists()) {
                throw new IllegalStateException("failed to remove provisional file: " + relative);
            }
        }
        for (Map.Entry<String, String> entry : snapshot.editableFiles.entrySet()) {
            writeAtomic(safeFile(projectRoot, entry.getKey()), entry.getValue());
        }
    }

    private static List<File> editableFiles(File projectRoot) throws Exception {
        ArrayList<File> files = new ArrayList<>();
        collect(projectRoot, new File(projectRoot, "src"), files);
        collect(projectRoot, new File(projectRoot, "tests"), files);
        Collections.sort(files, (left, right) -> left.getPath().compareTo(right.getPath()));
        return files;
    }

    private static void collect(File projectRoot, File directory, List<File> files) throws Exception {
        if (!directory.isDirectory()) return;
        File[] children = directory.listFiles();
        if (children == null) throw new IllegalStateException("project directory could not be read");
        for (File child : children) {
            if (child.isDirectory()) {
                collect(projectRoot, child, files);
            } else if (child.isFile() && child.getName().endsWith(".stasis")) {
                safeFile(projectRoot, relativePath(projectRoot, child));
                files.add(child);
            }
        }
    }

    private static String relativePath(File projectRoot, File file) throws Exception {
        String root = projectRoot.getCanonicalPath() + File.separator;
        String path = file.getCanonicalPath();
        if (!path.startsWith(root)) throw new IllegalArgumentException("editable file escaped project");
        return path.substring(root.length()).replace(File.separatorChar, '/');
    }

    private static File safeFile(File projectRoot, String relative) throws Exception {
        if (relative == null || (!relative.startsWith("src/") && !relative.startsWith("tests/"))
                || !relative.endsWith(".stasis") || relative.contains("..")) {
            throw new IllegalArgumentException("AI transaction path is invalid");
        }
        File file = new File(projectRoot, relative.replace('/', File.separatorChar));
        String root = projectRoot.getCanonicalPath() + File.separator;
        if (!file.getCanonicalPath().startsWith(root)) {
            throw new IllegalArgumentException("AI transaction path escaped project");
        }
        return file;
    }

    private static void writeAtomic(File file, String source) throws Exception {
        File parent = file.getParentFile();
        if (!parent.isDirectory() && !parent.mkdirs()) {
            throw new IllegalStateException("AI transaction parent directory could not be created");
        }
        File temporary = new File(parent, file.getName() + ".transaction.tmp");
        FileOutputStream output = new FileOutputStream(temporary);
        try {
            output.write(source.getBytes(StandardCharsets.UTF_8));
            output.getFD().sync();
        } finally {
            output.close();
        }
        try {
            Files.move(temporary.toPath(), file.toPath(), StandardCopyOption.ATOMIC_MOVE,
                    StandardCopyOption.REPLACE_EXISTING);
        } catch (Exception error) {
            temporary.delete();
            throw error;
        }
    }

    private static void requireRoot(File projectRoot) {
        if (projectRoot == null || !projectRoot.isDirectory()) {
            throw new IllegalArgumentException("AI transaction project root is invalid");
        }
    }
}
