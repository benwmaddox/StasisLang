package com.stasislang.workshop;

import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.HttpURLConnection;
import java.net.URL;
import java.net.URLEncoder;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.util.Base64;

import org.json.JSONArray;
import org.json.JSONObject;

final class WorkshopGitHubApi {
    private static final int NETWORK_TIMEOUT_MS = 15_000;
    private static final String DEFAULT_API_ROOT = "https://api.github.com/repos/";

    private final String token;
    private final String repository;
    private final String apiRoot;

    WorkshopGitHubApi(String token, String repository) {
        this(token, repository, DEFAULT_API_ROOT);
    }

    WorkshopGitHubApi(String token, String repository, String apiRoot) {
        this.token = token;
        this.repository = repository;
        this.apiRoot = apiRoot.endsWith("/") ? apiRoot : apiRoot + "/";
    }

    void validateTarget(String branch) throws Exception {
        getJson(apiUrl(""));
        getJson(apiUrl("/git/ref/heads/" + encodePath(branch)));
    }

    String applyFileChange(String branch, String path, byte[] content,
            String expectedRemoteSha) throws Exception {
        String base = apiUrl("/contents/" + encodePath(path));
        Response existing = request("GET", base + "?ref=" + encodeQuery(branch), null);
        if (existing.code != 200 && existing.code != 404) {
            throw new IOException("read " + path + " HTTP " + existing.code);
        }
        String remoteSha = existing.code == 200
                ? new JSONObject(existing.body).optString("sha", "") : "";
        boolean desiredContentAlreadyPresent = content == null ? existing.code == 404
                : remoteSha.equals(blobSha(content));
        try {
            WorkshopGitHubSyncPolicy.requireNoRemoteConflict(
                    path, expectedRemoteSha, remoteSha, desiredContentAlreadyPresent);
        } catch (IllegalStateException conflict) {
            throw new IOException(conflict.getMessage());
        }
        if (desiredContentAlreadyPresent) return remoteSha;
        if (content == null) {
            writeJson("DELETE", base,
                    new JSONObject().put("message", "stasis workshop delete: " + path)
                            .put("sha", remoteSha).put("branch", branch), 200);
            return "";
        }
        JSONObject body = new JSONObject().put("message", "stasis workshop sync: " + path)
                .put("content", Base64.getEncoder().encodeToString(content))
                .put("branch", branch);
        if (!remoteSha.isEmpty()) body.put("sha", remoteSha);
        JSONObject result = writeJson("PUT", base, body, remoteSha.isEmpty() ? 201 : 200);
        JSONObject written = result.optJSONObject("content");
        String writtenSha = written == null ? "" : written.optString("sha", "");
        if (writtenSha.isEmpty()) throw new IOException("write " + path + " returned no content SHA");
        return writtenSha;
    }

    void ensureReviewBranch(String baseBranch, String reviewBranch) throws Exception {
        String reviewRefUrl = apiUrl("/git/ref/heads/" + encodePath(reviewBranch));
        Response review = request("GET", reviewRefUrl, null);
        if (review.code == 200) return;
        if (review.code != 404) throw new IOException("review branch HTTP " + review.code);
        JSONObject baseRef = getJson(apiUrl("/git/ref/heads/" + encodePath(baseBranch)));
        JSONObject object = baseRef.optJSONObject("object");
        String baseSha = object == null ? "" : object.optString("sha", "");
        if (baseSha.isEmpty()) throw new IOException("base branch has no commit SHA");
        writeJson("POST", apiUrl("/git/refs"),
                new JSONObject().put("ref", "refs/heads/" + reviewBranch).put("sha", baseSha), 201);
    }

    String createOrFindPullRequest(String baseBranch, String reviewBranch, String body)
            throws Exception {
        String owner = repository.substring(0, repository.indexOf('/'));
        String query = "?state=open&head=" + encodeQuery(owner + ":" + reviewBranch)
                + "&base=" + encodeQuery(baseBranch);
        JSONArray existing = new JSONArray(read(apiUrl("/pulls" + query)));
        if (existing.length() > 0) {
            return existing.getJSONObject(0).optString("html_url", "existing PR");
        }
        JSONObject created = writeJson("POST", apiUrl("/pulls"),
                new JSONObject().put("title", "Stasis Workshop Android changes")
                        .put("head", reviewBranch).put("base", baseBranch).put("body", body), 201);
        return created.optString("html_url", "created PR");
    }

    private String apiUrl(String path) {
        return apiRoot + repository + path;
    }

    private static String encodePath(String value) throws Exception {
        return encodeQuery(value).replace("%2F", "/");
    }

    private static String encodeQuery(String value) throws Exception {
        return URLEncoder.encode(value, "UTF-8").replace("+", "%20");
    }

    private JSONObject getJson(String url) throws Exception {
        return new JSONObject(read(url));
    }

    private String read(String url) throws Exception {
        Response response = request("GET", url, null);
        if (response.code != 200) throw new IOException("GitHub read HTTP " + response.code);
        return response.body;
    }

    private JSONObject writeJson(String method, String url, JSONObject body,
            int expectedCode) throws Exception {
        Response response = request(method, url, body.toString().getBytes(StandardCharsets.UTF_8));
        if (response.code != expectedCode) {
            throw new IOException("GitHub write HTTP " + response.code);
        }
        return response.body.isEmpty() ? new JSONObject() : new JSONObject(response.body);
    }

    private Response request(String method, String url, byte[] body) throws Exception {
        HttpURLConnection connection = (HttpURLConnection)new URL(url).openConnection();
        try {
            connection.setRequestMethod(method);
            connection.setConnectTimeout(NETWORK_TIMEOUT_MS);
            connection.setReadTimeout(NETWORK_TIMEOUT_MS);
            connection.setRequestProperty("Accept", "application/vnd.github+json");
            connection.setRequestProperty("Authorization", "Bearer " + token);
            connection.setRequestProperty("X-GitHub-Api-Version", "2022-11-28");
            if (body != null) {
                connection.setDoOutput(true);
                connection.setRequestProperty("Content-Type", "application/json");
                try (OutputStream output = connection.getOutputStream()) {
                    output.write(body);
                }
            }
            int code = connection.getResponseCode();
            InputStream input = code >= 400 ? connection.getErrorStream() : connection.getInputStream();
            return new Response(code, input == null ? "" : readAll(input));
        } finally {
            connection.disconnect();
        }
    }

    private static String readAll(InputStream input) throws IOException {
        try (InputStream source = input; ByteArrayOutputStream output = new ByteArrayOutputStream()) {
            byte[] buffer = new byte[4096];
            int read;
            while ((read = source.read(buffer)) != -1) output.write(buffer, 0, read);
            return new String(output.toByteArray(), StandardCharsets.UTF_8);
        }
    }

    private static String blobSha(byte[] content) throws Exception {
        MessageDigest digest = MessageDigest.getInstance("SHA-1");
        digest.update(("blob " + content.length + "\0").getBytes(StandardCharsets.UTF_8));
        digest.update(content);
        byte[] bytes = digest.digest();
        StringBuilder sha = new StringBuilder(bytes.length * 2);
        String digits = "0123456789abcdef";
        for (byte item : bytes) {
            int value = item & 0xff;
            sha.append(digits.charAt(value >>> 4)).append(digits.charAt(value & 0x0f));
        }
        return sha.toString();
    }

    private static final class Response {
        final int code;
        final String body;

        Response(int code, String body) {
            this.code = code;
            this.body = body;
        }
    }
}
