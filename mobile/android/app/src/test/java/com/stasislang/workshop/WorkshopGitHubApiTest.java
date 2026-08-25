package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.fail;

import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.net.URI;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;

public final class WorkshopGitHubApiTest {
    private static final String BASE_SHA = repeat('1');
    private static final String OLD_SHA = repeat('a');
    private static final String NEW_SHA = repeat('b');
    private static final String CONFLICT_SHA = repeat('c');

    private final List<Request> requests = new ArrayList<>();
    private ServerSocket server;
    private Thread serverThread;
    private volatile boolean serverRunning;
    private volatile boolean existingPullRequest;
    private volatile Throwable serverFailure;

    @Before
    public void startServer() throws Exception {
        server = new ServerSocket();
        server.bind(new InetSocketAddress(InetAddress.getLoopbackAddress(), 0));
        serverRunning = true;
        serverThread = new Thread(new Runnable() {
            @Override public void run() { serveRequests(); }
        }, "workshop-github-test-server");
        serverThread.start();
    }

    @After
    public void stopServer() {
        serverRunning = false;
        if (server != null) {
            try {
                server.close();
            } catch (IOException ignored) {
            }
        }
        if (serverThread != null) {
            try {
                serverThread.join(2_000L);
            } catch (InterruptedException interrupted) {
                Thread.currentThread().interrupt();
            }
        }
    }

    @Test
    public void validatesTargetAppliesChangesAndCreatesOrFindsPullRequest() throws Exception {
        WorkshopGitHubApi api = new WorkshopGitHubApi("test-token", "owner/game",
                "http://127.0.0.1:" + server.getLocalPort() + "/repos/");

        api.validateTarget("main");
        assertEquals(NEW_SHA, api.applyFileChange("main", "src/new.stasis",
                "source".getBytes(StandardCharsets.UTF_8), null));
        assertEquals("", api.applyFileChange("main", "src/old.stasis", null, OLD_SHA));
        api.ensureReviewBranch("main", "stasis-workshop-project");
        assertEquals("https://example.test/pull/7", api.createOrFindPullRequest(
                "main", "stasis-workshop-project", "reviewed changes"));
        existingPullRequest = true;
        assertEquals("https://example.test/pull/7", api.createOrFindPullRequest(
                "main", "stasis-workshop-project", "reviewed changes"));

        assertRequest("GET", "/repos/owner/game");
        assertRequest("GET", "/repos/owner/game/git/ref/heads/main");
        assertRequest("PUT", "/repos/owner/game/contents/src/new.stasis");
        assertRequest("DELETE", "/repos/owner/game/contents/src/old.stasis");
        assertRequest("POST", "/repos/owner/game/git/refs");
        assertRequest("POST", "/repos/owner/game/pulls");
        for (Request request : requests) assertEquals("Bearer test-token", request.authorization);
        assertTrue(requestBody("PUT", "/repos/owner/game/contents/src/new.stasis")
                .contains("\"branch\":\"main\""));
        assertTrue(requestBody("DELETE", "/repos/owner/game/contents/src/old.stasis")
                .contains("\"sha\":\"" + OLD_SHA + "\""));
        assertNull(serverFailure);
    }

    @Test
    public void remoteShaConflictStopsBeforeWrite() throws Exception {
        WorkshopGitHubApi api = new WorkshopGitHubApi("test-token", "owner/game",
                "http://127.0.0.1:" + server.getLocalPort() + "/repos/");

        try {
            api.applyFileChange("main", "src/conflict.stasis",
                    "local".getBytes(StandardCharsets.UTF_8), OLD_SHA);
            fail("expected a remote conflict");
        } catch (IOException expected) {
            assertTrue(expected.getMessage().contains("changed remotely since the last backup"));
        }
        assertRequest("GET", "/repos/owner/game/contents/src/conflict.stasis");
        assertEquals(0, requestCount("PUT", "/repos/owner/game/contents/src/conflict.stasis"));
        assertNull(serverFailure);
    }

    private void assertRequest(String method, String path) {
        assertTrue(method + " " + path, requestCount(method, path) > 0);
    }

    private int requestCount(String method, String path) {
        int count = 0;
        for (Request request : requests) {
            if (method.equals(request.method) && path.equals(request.path)) count += 1;
        }
        return count;
    }

    private String requestBody(String method, String path) {
        for (Request request : requests) {
            if (method.equals(request.method) && path.equals(request.path)) return request.body;
        }
        return "";
    }

    private void serveRequests() {
        while (serverRunning) {
            try (Socket socket = server.accept()) {
                handle(socket);
            } catch (IOException error) {
                if (serverRunning) serverFailure = error;
            } catch (Throwable error) {
                serverFailure = error;
            }
        }
    }

    private void handle(Socket socket) throws Exception {
            InputStream input = socket.getInputStream();
            String requestLine = readLine(input);
            String[] requestParts = requestLine.split(" ", 3);
            String method = requestParts[0];
            String path = new URI(requestParts[1]).getPath();
            int contentLength = 0;
            String authorization = null;
            String line;
            while (!(line = readLine(input)).isEmpty()) {
                int separator = line.indexOf(':');
                if (separator <= 0) continue;
                String name = line.substring(0, separator).trim();
                String value = line.substring(separator + 1).trim();
                if ("Content-Length".equalsIgnoreCase(name)) contentLength = Integer.parseInt(value);
                else if ("Authorization".equalsIgnoreCase(name)) authorization = value;
            }
            byte[] bodyBytes = new byte[contentLength];
            int offset = 0;
            while (offset < bodyBytes.length) {
                int read = input.read(bodyBytes, offset, bodyBytes.length - offset);
                if (read < 0) throw new IOException("unexpected end of request body");
                offset += read;
            }
            String body = new String(bodyBytes, StandardCharsets.UTF_8);
            requests.add(new Request(method, path, body, authorization));

            int code = 200;
            String response = "{}";
            if (path.endsWith("/contents/src/new.stasis")) {
                if ("GET".equals(method)) code = 404;
                else {
                    code = 201;
                    response = "{\"content\":{\"sha\":\"" + NEW_SHA + "\"}}";
                }
            } else if (path.endsWith("/contents/src/old.stasis")) {
                if ("GET".equals(method)) response = "{\"sha\":\"" + OLD_SHA + "\"}";
            } else if (path.endsWith("/contents/src/conflict.stasis")) {
                response = "{\"sha\":\"" + CONFLICT_SHA + "\"}";
            } else if (path.endsWith("/git/ref/heads/stasis-workshop-project")) {
                code = 404;
            } else if (path.endsWith("/git/ref/heads/main")) {
                response = "{\"object\":{\"sha\":\"" + BASE_SHA + "\"}}";
            } else if (path.endsWith("/git/refs") && "POST".equals(method)) {
                code = 201;
            } else if (path.endsWith("/pulls") && "GET".equals(method)) {
                response = existingPullRequest
                        ? "[{\"html_url\":\"https://example.test/pull/7\"}]" : "[]";
            } else if (path.endsWith("/pulls") && "POST".equals(method)) {
                code = 201;
                response = "{\"html_url\":\"https://example.test/pull/7\"}";
            }
            byte[] bytes = response.getBytes(StandardCharsets.UTF_8);
            String status = code == 200 ? "OK" : (code == 201 ? "Created" : "Not Found");
            OutputStream output = socket.getOutputStream();
            output.write(("HTTP/1.1 " + code + " " + status + "\r\n"
                    + "Content-Type: application/json\r\n"
                    + "Content-Length: " + bytes.length + "\r\n"
                    + "Connection: close\r\n\r\n").getBytes(StandardCharsets.US_ASCII));
            output.write(bytes);
            output.flush();
    }

    private static String readLine(InputStream input) throws IOException {
        ByteArrayOutputStream output = new ByteArrayOutputStream();
        int value;
        while ((value = input.read()) != -1) {
            if (value == '\n') break;
            if (value != '\r') output.write(value);
        }
        if (value == -1 && output.size() == 0) throw new IOException("unexpected end of headers");
        return new String(output.toByteArray(), StandardCharsets.US_ASCII);
    }

    private static String repeat(char value) {
        StringBuilder result = new StringBuilder();
        while (result.length() < 40) result.append(value);
        return result.toString();
    }

    private static final class Request {
        final String method;
        final String path;
        final String body;
        final String authorization;

        Request(String method, String path, String body, String authorization) {
            this.method = method;
            this.path = path;
            this.body = body;
            this.authorization = authorization;
        }
    }
}
