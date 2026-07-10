package com.stasislang.workshop;

import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.OutputStream;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.List;
import java.util.zip.ZipEntry;
import java.util.zip.ZipOutputStream;

final class WorkshopProjectArchive {
    static final int MAX_FILES = 512;
    static final long MAX_ENTRY_BYTES = 32L * 1024L * 1024L;
    static final long MAX_TOTAL_BYTES = 128L * 1024L * 1024L;

    private WorkshopProjectArchive() {}

    static ExportSummary exportProject(File projectRoot, OutputStream destination) throws Exception {
        String canonicalRoot = projectRoot.getCanonicalPath();
        ArrayList<File> files = new ArrayList<>();
        collectFiles(projectRoot, projectRoot, files);
        Collections.sort(files, new Comparator<File>() {
            @Override public int compare(File left, File right) {
                return left.getAbsolutePath().compareTo(right.getAbsolutePath());
            }
        });
        if (files.size() > MAX_FILES) throw new IOException("project archive exceeds " + MAX_FILES + " files");

        long totalBytes = 0L;
        for (File file : files) {
            long length = file.length();
            if (length > MAX_ENTRY_BYTES) throw new IOException("project file exceeds archive limit: " + file.getName());
            totalBytes += length;
            if (totalBytes > MAX_TOTAL_BYTES) throw new IOException("project archive exceeds total size limit");
        }

        ZipOutputStream zip = new ZipOutputStream(destination);
        byte[] buffer = new byte[16 * 1024];
        try {
            for (File file : files) {
                String canonicalFile = file.getCanonicalPath();
                if (!canonicalFile.startsWith(canonicalRoot + File.separator)) {
                    throw new IOException("project file escaped project root");
                }
                String relative = canonicalFile.substring(canonicalRoot.length() + 1)
                        .replace(File.separatorChar, '/');
                ZipEntry entry = new ZipEntry(relative);
                entry.setTime(0L);
                zip.putNextEntry(entry);
                FileInputStream input = new FileInputStream(file);
                try {
                    int read;
                    while ((read = input.read(buffer)) >= 0) zip.write(buffer, 0, read);
                } finally {
                    input.close();
                }
                zip.closeEntry();
            }
            zip.finish();
        } finally {
            zip.close();
        }
        return new ExportSummary(files.size(), totalBytes);
    }

    private static void collectFiles(File projectRoot, File current, List<File> files) throws Exception {
        if (!current.exists()) return;
        if (current.isDirectory()) {
            if (!current.equals(projectRoot) && "build".equals(current.getName())) return;
            File[] children = current.listFiles();
            if (children == null) throw new IOException("unable to list project directory: " + current.getName());
            for (File child : children) collectFiles(projectRoot, child, files);
            return;
        }
        if (current.getName().endsWith(".tmp")) return;
        files.add(current);
        if (files.size() > MAX_FILES) throw new IOException("project archive exceeds " + MAX_FILES + " files");
    }

    static final class ExportSummary {
        final int fileCount;
        final long totalBytes;

        ExportSummary(int fileCount, long totalBytes) {
            this.fileCount = fileCount;
            this.totalBytes = totalBytes;
        }
    }
}
