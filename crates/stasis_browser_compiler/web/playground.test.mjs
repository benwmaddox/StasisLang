import assert from "node:assert/strict";
import test from "node:test";

import { inspectPngHeader, normalizeAssetPath } from "./project_assets.mjs";
import { buildStoredZip, crc32, createExportEntries, normalizeArchivePath } from "./export_package.mjs";
import {
  applyEditorCompletion, buildHighlightedFragment, editorKeyAction,
  isCompletionContext, isCurrentEditorAnalysis,
} from "./editor_view.mjs";

function pngHeader(width, height) {
  const bytes = new Uint8Array(24);
  bytes.set([137, 80, 78, 71, 13, 10, 26, 10]);
  const view = new DataView(bytes.buffer);
  view.setUint32(8, 13, false);
  bytes.set([73, 72, 68, 82], 12);
  view.setUint32(16, width, false);
  view.setUint32(20, height, false);
  return bytes;
}

function readStoredZip(bytes) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const entries = new Map();
  let offset = 0;
  while (view.getUint32(offset, true) === 0x04034b50) {
    const checksum = view.getUint32(offset + 14, true);
    const compressedSize = view.getUint32(offset + 18, true);
    const nameLength = view.getUint16(offset + 26, true);
    const extraLength = view.getUint16(offset + 28, true);
    const nameStart = offset + 30;
    const dataStart = nameStart + nameLength + extraLength;
    const name = new TextDecoder().decode(bytes.subarray(nameStart, nameStart + nameLength));
    const data = bytes.subarray(dataStart, dataStart + compressedSize);
    entries.set(name, { data: data.slice(), checksum });
    offset = dataStart + compressedSize;
  }
  assert.equal(view.getUint32(offset, true), 0x02014b50, "central directory follows stored entries");
  const end = bytes.byteLength - 22;
  assert.equal(view.getUint32(end, true), 0x06054b50, "ZIP has an end-of-central-directory record");
  assert.equal(view.getUint16(end + 10, true), entries.size, "ZIP entry counts match");
  return entries;
}

test("PNG IHDR parsing accepts bounded positive dimensions and rejects malformed or oversized images", () => {
  assert.deepEqual(inspectPngHeader(pngHeader(640, 360)), { width: 640, height: 360 });
  assert.throws(() => inspectPngHeader(new Uint8Array(24)), /not a PNG/);
  assert.throws(() => inspectPngHeader(pngHeader(0, 8)), /dimensions/);
  assert.throws(() => inspectPngHeader(pngHeader(4097, 1)), /dimensions/);
  assert.throws(() => inspectPngHeader(pngHeader(4096, 4096)), /pixels/);
});

test("asset and archive paths reject traversal and keep project assets under assets/", () => {
  assert.equal(normalizeAssetPath("assets/ship-1.PNG"), "assets/ship-1.PNG");
  assert.throws(() => normalizeAssetPath("../ship.png"), /safe PNG path/);
  assert.throws(() => normalizeAssetPath("assets/../ship.png"), /safe PNG path/);
  assert.equal(normalizeArchivePath("project/stdlib/graphics.stasis"), "project/stdlib/graphics.stasis");
  for (const path of ["../x", "/x", "a\\b", "a//b", "a/./b", "a/../b", "C:/x"]) {
    assert.throws(() => normalizeArchivePath(path), /unsafe/);
  }
});

test("ZIP uses stored entries with correct CRCs and deterministic headers", () => {
  const entries = [
    { path: "index.html", bytes: "<h1>Stasis</h1>" },
    { path: "assets/player.png", bytes: Uint8Array.of(1, 2, 3, 4) },
  ];
  const zip = buildStoredZip(entries);
  assert.deepEqual(buildStoredZip(entries), zip);
  assert.equal(crc32("123456789"), 0xcbf43926);
  const decoded = readStoredZip(zip);
  assert.equal(new TextDecoder().decode(decoded.get("index.html").data), "<h1>Stasis</h1>");
  assert.deepEqual(decoded.get("assets/player.png").data, Uint8Array.of(1, 2, 3, 4));
  assert.equal(decoded.get("assets/player.png").checksum, crc32(Uint8Array.of(1, 2, 3, 4)));
  assert.throws(() => buildStoredZip([{ path: "same", bytes: "a" }, { path: "same", bytes: "b" }]), /duplicate/);
});

test("export entries include runnable files, source manifest, and local asset paths", async () => {
  const metadata = {
    schemaVersion: 1,
    layoutDigest: "0123456789abcdef",
    imports: ["web_begin_frame"],
    provenance: { compiler: "test", build: "browser" },
    config: { strings: {}, assets: {}, stringLiteralTable: {}, collectionViewAbiVersion: 2 },
  };
  const entries = await createExportEntries({
    entry: "main.stasis",
    files: [{ path: "main.stasis", source: "function main(): i32 { return 0; }" }],
    assets: [{ path: "assets/player.png", bytes: Uint8Array.of(1, 2, 3), width: 1, height: 1, sha256: "abc" }],
    metadata,
    gameWasm: Uint8Array.of(0, 97, 115, 109, 1, 0, 0, 0),
    runtimeScript: "async function wasmBytes(){return fetch('__STASIS_WASM_URL__')}" ,
  });
  const archive = readStoredZip(buildStoredZip(entries));
  for (const path of ["index.html", "game.js", "game.wasm", "manifest.json", "provenance.json", "assets/player.png", "project/main.stasis"]) {
    assert.ok(archive.has(path), `archive should include ${path}`);
  }
  const html = new TextDecoder().decode(archive.get("index.html").data);
  assert.match(html, /window\.STASIS_GAME=/);
  const runtime = new TextDecoder().decode(archive.get("game.js").data);
  assert.ok(runtime.includes("./game.wasm"));
  assert.ok(!runtime.includes("__STASIS_WASM_URL__"));
  const manifest = JSON.parse(new TextDecoder().decode(archive.get("manifest.json").data));
  assert.equal(manifest.entry, "main.stasis");
  assert.equal(manifest.assets[0].path, "assets/player.png");
  assert.ok(manifest.files.includes("provenance.json"));
});

test("syntax tokens preserve source text and render untrusted text only as DOM text", () => {
  const createdElements = [];
  const document = {
    createDocumentFragment() {
      return { children: [], append(...children) { this.children.push(...children); } };
    },
    createElement(tagName) {
      const element = { tagName, className: "", textContent: "" };
      createdElements.push(element);
      return element;
    },
    createTextNode(textContent) { return { tagName: "#text", textContent }; },
  };
  const source = 'let label = "<img src=x onerror=alert(1)>";';
  const stringStart = source.indexOf('"');
  const stringEnd = source.lastIndexOf('"') + 1;
  const fragment = buildHighlightedFragment(source, [
    { start: 0, end: 3, kind: "keyword" },
    { start: stringStart, end: stringEnd, kind: "string" },
  ], document);
  assert.equal(fragment.children.map(child => child.textContent).join(""), source);
  assert.deepEqual(createdElements.map(element => element.className), ["syntax-keyword", "syntax-string"]);
  assert.equal(createdElements[1].textContent, source.slice(stringStart, stringEnd));
});

test("completion replacement uses UTF-16 ranges and preserves source outside the replacement", () => {
  const source = "say 😀fo() and keep this";
  const start = source.indexOf("fo");
  const applied = applyEditorCompletion(source, { start, end: start + 2 }, "format");
  assert.equal(applied.source, "say 😀format() and keep this");
  assert.equal(applied.cursor, start + "format".length);
  assert.equal(applied.selectionStart, applied.selectionEnd);
});

test("completion visibility follows prefixes, stale analysis identity is rejected, and keyboard controls stay distinct", () => {
  assert.equal(isCompletionContext("call na", 7), true);
  assert.equal(isCompletionContext("call.", 5), true);
  assert.equal(isCompletionContext("call ", 5), false);
  assert.equal(isCompletionContext("call ", 5, true), true);
  const analysis = { revision: 4, path: "main.stasis", source: "call na", cursor: 7 };
  assert.equal(isCurrentEditorAnalysis(analysis, { ...analysis }), true);
  assert.equal(isCurrentEditorAnalysis(analysis, { ...analysis, source: "call name" }), false);
  assert.equal(editorKeyAction({ key: "Enter", ctrlKey: true }, true), "run");
  assert.equal(editorKeyAction({ key: " ", ctrlKey: true }, false), "open");
  assert.equal(editorKeyAction({ key: "ArrowDown" }, true), "next");
  assert.equal(editorKeyAction({ key: "Tab" }, true), "accept");
  assert.equal(editorKeyAction({ key: "Tab" }, false), "indent");
  assert.equal(editorKeyAction({ key: "Escape" }, true), "close");
});

function deferred() {
  let resolvePromise;
  let rejectPromise;
  const promise = new Promise((resolve, reject) => {
    resolvePromise = resolve;
    rejectPromise = reject;
  });
  return { promise, resolve: resolvePromise, reject: rejectPromise };
}

class FakeElement {
  static frames = [];

  constructor(tagName = "div") {
    this.tagName = tagName.toUpperCase();
    this.children = [];
    this.listeners = new Map();
    this.dataset = {};
    this.attributes = new Map();
    this.parentNode = null;
    this._textContent = "";
    this.value = "";
    this.className = "";
    this.hidden = false;
    this.selectionStart = 0;
    this.selectionEnd = 0;
    if (tagName === "iframe") {
      this.contextDiscarded = false;
      this.contentWindow = this.createBrowsingContext();
      FakeElement.frames.push(this);
    }
  }

  get textContent() {
    return this.children.length ? this.children.map(child => child.textContent).join("") : this._textContent;
  }

  set textContent(value) {
    for (const child of [...this.children]) child.remove();
    this.children = [];
    this._textContent = String(value ?? "");
  }

  withText(value) { this.textContent = value; return this; }

  createBrowsingContext() {
    this.readiness = deferred();
    return {
      __STASIS_RUNTIME_SCRIPT_READY__: Promise.resolve(),
      STASIS_PLAYGROUND: {
        ready: this.readiness.promise,
        generation: () => 1,
        swap: async () => ({ accepted: true }),
      },
    };
  }

  addEventListener(type, listener, options = {}) {
    const listeners = this.listeners.get(type) || [];
    listeners.push({ listener, once: Boolean(options.once) });
    this.listeners.set(type, listeners);
  }

  dispatch(type, event = {}) {
    const listeners = this.listeners.get(type) || [];
    for (const record of [...listeners]) {
      record.listener({ target: this, ...event });
      if (record.once) listeners.splice(listeners.indexOf(record), 1);
    }
  }

  setAttribute(name, value) { this.attributes.set(name, String(value)); }
  getAttribute(name) { return this.attributes.get(name) ?? null; }
  removeAttribute(name) { this.attributes.delete(name); }
  setSelectionRange(start, end) { this.selectionStart = start; this.selectionEnd = end; }
  focus() { this.focused = true; }
  scrollIntoView() {}
  setRangeText(replacement, start, end, selectionMode) {
    this.value = `${this.value.slice(0, start)}${replacement}${this.value.slice(end)}`;
    const cursor = start + replacement.length;
    this.setSelectionRange(cursor, selectionMode === "select" ? cursor + replacement.length : cursor);
  }

  append(...nodes) {
    for (const node of nodes) {
      if (node.tagName === "#FRAGMENT") {
        this.append(...node.children.slice());
        node.children = [];
        continue;
      }
      node.remove();
      if (node.tagName === "IFRAME" && node.contextDiscarded) {
        node.contentWindow = node.createBrowsingContext();
        node.contextDiscarded = false;
      }
      node.parentNode = this;
      this.children.push(node);
    }
  }

  replaceChildren(...nodes) {
    for (const child of [...this.children]) child.remove();
    this.append(...nodes);
  }

  remove() {
    if (this.parentNode) {
      this.parentNode.children = this.parentNode.children.filter(child => child !== this);
      this.parentNode = null;
      if (this.tagName === "IFRAME") this.contextDiscarded = true;
    }
  }

  get classList() {
    return { remove: name => { this.className = this.className.split(/\s+/).filter(item => item !== name).join(" "); } };
  }
}

async function waitUntil(predicate, message) {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if (predicate()) return;
    await new Promise(resolvePromise => setTimeout(resolvePromise, 0));
  }
  assert.fail(message);
}

test("overlapping initial run and restart publish only the newest ready iframe", async () => {
  const prior = new Map(["document", "window", "Worker", "fetch"].map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  const elements = new Map();
  const fakeDocument = {
    getElementById(id) {
      if (!elements.has(id)) elements.set(id, new FakeElement("div"));
      return elements.get(id);
    },
    createElement(tagName) { return new FakeElement(tagName); },
    createDocumentFragment() { return new FakeElement("#fragment"); },
    createTextNode(value) { return new FakeElement("#text").withText(value); },
  };
  const workerInstances = [];
  class FakeWorker {
    constructor() {
      this.listeners = new Map();
      this.requests = [];
      this.failNextAnalysis = false;
      this.pauseNextAnalysis = false;
      workerInstances.push(this);
      queueMicrotask(() => this.dispatch("message", { data: { type: "ready", abiVersion: 1 } }));
    }

    addEventListener(type, listener) {
      const listeners = this.listeners.get(type) || [];
      listeners.push(listener);
      this.listeners.set(type, listeners);
    }

    dispatch(type, event) {
      for (const listener of this.listeners.get(type) || []) listener(event);
    }

    postMessage(message) {
      this.requests.push(message);
      if (message.type === "analyze-editor") {
        if (this.pauseNextAnalysis) {
          this.pauseNextAnalysis = false;
          return;
        }
        queueMicrotask(() => {
          if (this.failNextAnalysis) {
            this.failNextAnalysis = false;
            this.dispatch("message", { data: {
              type: "editor-analysis-error", requestId: message.requestId,
              path: message.path, message: "analysis incomplete",
            } });
            return;
          }
          const prefix = message.source.slice(0, message.cursor);
          const completionContext = prefix.endsWith("fo");
          const hasHelper = message.files.some(file => file.path === "helpers.stasis" && file.source.includes("function helper"));
          const helperContext = hasHelper && prefix.endsWith("hel");
          const start = helperContext ? message.cursor - 3 : completionContext ? message.cursor - 2 : message.cursor;
          this.dispatch("message", { data: {
            type: "editor-analysis", requestId: message.requestId, path: message.path,
            tokens: message.source.startsWith("function") ? [{ start: 0, end: 8, kind: "keyword" }] : [],
            completions: helperContext ? [
              { label: "helper", insertText: "helper", kind: "function", signature: "helper(value: i32): i32" },
            ] : [
              { label: "foo", insertText: "foo", kind: "function", signature: "foo(): i32" },
              { label: "format", insertText: "format", kind: "function", detail: "Format a value" },
            ],
            replacementRange: { start, end: message.cursor },
          } });
        });
      }
    }
  }
  globalThis.document = fakeDocument;
  globalThis.window = {};
  globalThis.Worker = FakeWorker;
  globalThis.fetch = async path => path === "./game.js"
    ? { ok: true, text: async () => "const wasmUrl = '__STASIS_WASM_URL__';" }
    : { ok: true, json: async () => [{ path: "vendor/stasis/stdlib/test.stasis", source: "// local test standard library" }] };

  try {
    const moduleUrl = new URL("./playground.js", import.meta.url);
    moduleUrl.searchParams.set("concurrency-test", String(Date.now()));
    await import(moduleUrl.href);
    const api = globalThis.window.STASIS_PLAYGROUND_EDITOR;
    assert.equal(elements.get("source-highlight").textContent, elements.get("source-editor").value, "editor starts with visible plaintext before Worker analysis");
    await api.ready;
    const worker = workerInstances[0];
    const editor = elements.get("source-editor");
    api.setFile("main.stasis", "function fo() { }");
    editor.setSelectionRange(11, 11);
    api.refreshEditorAnalysis({ immediate: true });
    await waitUntil(() => api.getEditorView().completionOpen, "prefix analysis should show accessible suggestions");
    assert.ok(api.getEditorView().tokens.some(token => token.kind === "keyword"));
    assert.equal(elements.get("source-highlight").textContent, "function fo() { }");
    assert.equal(elements.get("source-editor").getAttribute("aria-expanded"), "true");
    const keyEvent = key => ({ key, preventDefault() { this.defaultPrevented = true; } });
    editor.dispatch("keydown", keyEvent("ArrowDown"));
    assert.equal(api.getEditorView().activeCompletionIndex, 1);
    editor.dispatch("keydown", keyEvent("Tab"));
    assert.equal(editor.value, "function format() { }");
    assert.equal(editor.selectionStart, 15);
    assert.equal(api.getEditorView().completionOpen, false);
    assert.equal(elements.get("editor-completions").hidden, true, "accepting a completion should not immediately reopen the same menu");
    editor.dispatch("keydown", { ...keyEvent(" "), ctrlKey: true });
    await waitUntil(() => api.getEditorView().completionOpen, "Ctrl+Space should explicitly open suggestions");
    assert.match(elements.get("editor-completions").textContent, /foo\(\): i32/);
    editor.dispatch("keydown", keyEvent("Escape"));
    assert.equal(api.getEditorView().completionOpen, false);

    api.setFile("helpers.stasis", "function helper(value: i32): i32 { return value; }");
    const incompleteMain = "function main(): i32 { return hel; }";
    api.setFile("main.stasis", incompleteMain);
    const helperCursor = incompleteMain.indexOf("hel") + 3;
    editor.setSelectionRange(helperCursor, helperCursor);
    api.refreshEditorAnalysis({ force: true, immediate: true });
    await waitUntil(() => api.getEditorView().completionOpen, "forced completion should include project modules during an incomplete draft");
    const helperAnalysisRequest = worker.requests.filter(request => request.type === "analyze-editor").at(-1);
    assert.equal(helperAnalysisRequest.path, "main.stasis");
    assert.equal(helperAnalysisRequest.source, incompleteMain);
    assert.ok(helperAnalysisRequest.files.some(file => file.path === "helpers.stasis" && file.source.includes("function helper")));
    assert.match(elements.get("editor-completions").textContent, /helper\(value: i32\): i32/);
    editor.dispatch("keydown", keyEvent("Enter"));
    assert.equal(editor.value, "function main(): i32 { return helper; }");
    assert.equal(editor.selectionStart, helperCursor + 3);

    const compileRequests = () => worker.requests.filter(request => request.type === "compile-project");
    const firstRun = api.run();
    await waitUntil(() => compileRequests().length === 1, "initial run should request compilation");

    const respondCompiled = (request, layoutDigest) => worker.dispatch("message", {
      data: {
        type: "compiled", requestId: request.requestId,
        gameWasm: Uint8Array.of(0, 97, 115, 109, 1, 0, 0, 0).buffer,
        metadata: { imports: [], layoutDigest, config: {} },
      },
    });
    respondCompiled(compileRequests()[0], "older-layout");
    await waitUntil(() => FakeElement.frames.length === 1, "initial run should create a candidate frame");
    const olderFrame = FakeElement.frames[0];
    olderFrame.dispatch("load");

    const restart = api.restart();
    const firstRunRejected = assert.rejects(firstRun, /playground operation superseded/);
    await firstRunRejected;
    await waitUntil(() => compileRequests().length === 2, "restart should request its own compilation");
    respondCompiled(compileRequests()[1], "newer-layout");
    await waitUntil(() => FakeElement.frames.length === 2, "restart should create a replacement candidate frame");
    const newerFrame = FakeElement.frames[1];
    newerFrame.dispatch("load");

    const readyBrowsingContext = newerFrame.contentWindow;
    newerFrame.readiness.resolve();
    const restarted = await restart;
    assert.equal(restarted.layoutDigest, "newer-layout");
    assert.equal(api.getIframe(), newerFrame);
    assert.equal(newerFrame.contentWindow, readyBrowsingContext, "publishing must preserve the loaded iframe browsing context");
    assert.equal(api.getState().generation, 1);
    assert.deepEqual(elements.get("preview-frame-host").children, [newerFrame]);

    olderFrame.readiness.resolve();
    assert.equal(api.getIframe(), newerFrame);
    assert.deepEqual(elements.get("preview-frame-host").children, [newerFrame]);
    assert.equal(api.getState().status, "Fresh game session started.");

    const failingRestart = api.restart();
    await waitUntil(() => compileRequests().length === 3, "a later restart should compile independently");
    respondCompiled(compileRequests()[2], "failed-candidate-layout");
    await waitUntil(() => FakeElement.frames.length === 3, "the failed candidate should reach readiness");
    const failingFrame = FakeElement.frames[2];
    failingFrame.dispatch("load");
    failingFrame.readiness.reject(new Error("candidate initialization failed"));
    await assert.rejects(failingRestart, /candidate initialization failed/);
    assert.equal(api.getIframe(), newerFrame, "failed candidate must leave the running game active");
    assert.deepEqual(elements.get("preview-frame-host").children, [newerFrame]);
    assert.equal(api.getState().running, true);

    const statusBeforeAnalysisError = api.getState().status;
    worker.failNextAnalysis = true;
    api.setFile("main.stasis", "function fail");
    editor.setSelectionRange(editor.value.length, editor.value.length);
    api.refreshEditorAnalysis({ immediate: true });
    await waitUntil(() => !api.getEditorView().analysisPending, "analysis errors should clear the pending request");
    assert.equal(api.getState().status, statusBeforeAnalysisError, "analysis errors must not replace compile/run diagnostics");

    const analysisRequests = () => worker.requests.filter(request => request.type === "analyze-editor");
    const previousAnalysisCount = analysisRequests().length;
    worker.pauseNextAnalysis = true;
    api.refreshEditorAnalysis({ immediate: true });
    await waitUntil(() => analysisRequests().length > previousAnalysisCount, "a stale analysis request should be pending");
    const staleAnalysis = analysisRequests().at(-1);
    api.setFile("main.stasis", "function current");
    editor.setSelectionRange(editor.value.length, editor.value.length);
    api.refreshEditorAnalysis({ immediate: true });
    await waitUntil(() => api.getEditorView().tokens.length > 0, "the newer editor analysis should render");
    worker.dispatch("message", { data: {
      type: "editor-analysis", requestId: staleAnalysis.requestId, path: "main.stasis",
      tokens: [{ start: 0, end: 8, kind: "number" }], completions: [], replacementRange: { start: 8, end: 8 },
    } });
    assert.equal(api.getEditorView().source, "function current");
    assert.equal(api.getEditorView().tokens[0].kind, "keyword", "late results must not replace newer highlighting");

    worker.pauseNextAnalysis = true;
    api.refreshEditorAnalysis({ immediate: true });
    await waitUntil(() => api.getEditorView().analysisPending, "analysis request should be pending before worker failure");
    worker.dispatch("error", {
      message: "Uncaught SyntaxError: Unexpected token",
      filename: "http://localhost/compiler_worker.js",
      lineno: 17,
      colno: 4,
    });
    await waitUntil(() => !api.getEditorView().analysisPending, "worker errors should reject and clear pending analysis");
    assert.equal(api.getState().ready, false);
    assert.match(api.getState().error, /Uncaught SyntaxError: Unexpected token/);
    assert.match(api.getState().error, /compiler_worker\.js:17:4/);
  } finally {
    for (const [key, descriptor] of prior) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  }
});
