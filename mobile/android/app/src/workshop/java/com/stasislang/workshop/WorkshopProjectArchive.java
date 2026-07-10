package com.stasislang.workshop;

import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.List;
import java.util.HashSet;
import java.util.Set;
import java.util.zip.ZipEntry;
import java.util.zip.ZipInputStream;
import java.util.zip.ZipOutputStream;

final class WorkshopProjectArchive {
    static final int MAX_FILES = 512;
    static final long MAX_ENTRY_BYTES = 32L * 1024L * 1024L;
    static final long MAX_TOTAL_BYTES = 128L * 1024L * 1024L;
    private static final String METADATA_ENTRY = ".stasis-workshop.json";

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

    static ImportSummary importProject(InputStream archive, File targetRoot) throws Exception {
        String canonicalRoot = targetRoot.getCanonicalPath();
        Set<String> paths = new HashSet<>();
        ZipInputStream zip = new ZipInputStream(archive);
        byte[] buffer = new byte[16 * 1024];
        int fileCount = 0;
        long totalBytes = 0L;
        boolean metadataFound = false;
        boolean entryFound = false;
        try {
            ZipEntry entry;
            while ((entry = zip.getNextEntry()) != null) {
                String rawPath = entry.getName();
                if (entry.isDirectory() && rawPath.endsWith("/")) rawPath = rawPath.substring(0, rawPath.length() - 1);
                String path = validateArchivePath(rawPath);
                if (!paths.add(path)) throw new IOException("project archive contains duplicate path: " + path);
                if (entry.isDirectory()) {
                    zip.closeEntry();
                    continue;
                }
                fileCount += 1;
                if (fileCount > MAX_FILES) throw new IOException("project archive exceeds " + MAX_FILES + " files");
                ByteArrayOutputStreamBuffer entryBytes = readBoundedEntry(zip, buffer, path);
                totalBytes += entryBytes.length();
                if (totalBytes > MAX_TOTAL_BYTES) throw new IOException("project archive exceeds total size limit");
                if (METADATA_ENTRY.equals(path)) {
                    String metadata = entryBytes.utf8();
                    if (!metadata.matches("(?s).*\\\"format_version\\\"\\s*:\\s*(?:1|2|3)(?:\\D.*|\\s*)")) {
                        throw new IOException("project archive metadata format is unsupported");
                    }
                    metadataFound = true;
                } else if (!path.startsWith("build/") && !path.endsWith(".tmp")) {
                    File target = new File(targetRoot, path.replace('/', File.separatorChar));
                    String canonicalTarget = target.getCanonicalPath();
                    if (!canonicalTarget.startsWith(canonicalRoot + File.separator)) {
                        throw new IOException("project archive path escaped target root");
                    }
                    File parent = target.getParentFile();
                    if (!parent.isDirectory() && !parent.mkdirs()) throw new IOException("unable to create archive directory");
                    FileOutputStream output = new FileOutputStream(target);
                    try {
                        output.write(entryBytes.bytes(), 0, entryBytes.length());
                        output.getFD().sync();
                    } finally {
                        output.close();
                    }
                    if ("src/main.stasis".equals(path)) entryFound = true;
                }
                zip.closeEntry();
            }
        } finally {
            zip.close();
        }
        if (!metadataFound) throw new IOException("project archive metadata is missing");
        if (!entryFound) throw new IOException("project archive needs src/main.stasis");
        return new ImportSummary(fileCount, totalBytes);
    }

    private static String validateArchivePath(String rawPath) throws IOException {
        if (rawPath == null || rawPath.isEmpty() || rawPath.startsWith("/") || rawPath.indexOf('\\') >= 0) {
            throw new IOException("project archive path is invalid");
        }
        String[] segments = rawPath.split("/", -1);
        for (String segment : segments) {
            if (segment.isEmpty() || ".".equals(segment) || "..".equals(segment) || segment.indexOf(':') >= 0) {
                throw new IOException("project archive path is invalid: " + rawPath);
            }
        }
        return rawPath;
    }

    private static ByteArrayOutputStreamBuffer readBoundedEntry(ZipInputStream zip, byte[] buffer, String path) throws Exception {
        ByteArrayOutputStreamBuffer output = new ByteArrayOutputStreamBuffer();
        int read;
        while ((read = zip.read(buffer)) >= 0) {
            if ((long)output.length() + read > MAX_ENTRY_BYTES) {
                throw new IOException("project file exceeds archive limit: " + path);
            }
            output.write(buffer, read);
        }
        return output;
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

    static final class ImportSummary {
        final int fileCount;
        final long totalBytes;

        ImportSummary(int fileCount, long totalBytes) {
            this.fileCount = fileCount;
            this.totalBytes = totalBytes;
        }
    }

    private static final class ByteArrayOutputStreamBuffer {
        private byte[] bytes = new byte[4096];
        private int length;

        void write(byte[] source, int count) {
            int required = length + count;
            if (required > bytes.length) {
                int capacity = bytes.length;
                while (capacity < required) capacity *= 2;
                byte[] expanded = new byte[capacity];
                System.arraycopy(bytes, 0, expanded, 0, length);
                bytes = expanded;
            }
            System.arraycopy(source, 0, bytes, length, count);
            length += count;
        }

        int length() { return length; }
        byte[] bytes() { return bytes; }
        String utf8() throws Exception { return new String(bytes, 0, length, "UTF-8"); }
    }
}
