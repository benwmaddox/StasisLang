import { ProjectAssetStore } from "./project_assets.mjs";
import { exportProjectZip } from "./export_package.mjs";
import {
  applyEditorCompletion, buildHighlightedFragment, editorKeyAction,
  isCompletionContext, isCurrentEditorAnalysis,
} from "./editor_view.mjs";

const DEFAULT_SOURCE = `function @extern("web_begin_frame") web_begin_frame(red: i32, green: i32, blue: i32): void;
function @extern("web_draw_rect") web_draw_rect(x: i32, y: i32, width: i32, height: i32, red: i32, green: i32, blue: i32): void;
function @extern("web_input_axis") web_input_axis(): i32;

global shape_x: i32;

function main(): i32 {
    shape_x = 288;
    return 0;
}

function tick(): i32 {
    shape_x += web_input_axis() * 3;
    if (shape_x < 0) {
        shape_x = 0;
    }
    if (shape_x > 608) {
        shape_x = 608;
    }
    return 0;
}

function render(): i32 {
    web_begin_frame(7, 18, 28);
    web_draw_rect(0, 326, 640, 34, 19, 42, 50);
    web_draw_rect(shape_x, 263, 32, 32, 236, 192, 102);
    web_draw_rect(61, 296, 3, 30, 75, 177, 139);
    web_draw_rect(545, 296, 3, 30, 75, 177, 139);
    return 0;
}
`;

const MAX_PROJECT_FILES = 64;
const MAX_SOURCE_BYTES = 1024 * 1024;
const MAX_PROJECT_SOURCE_BYTES = 4 * 1024 * 1024;
const encoder = new TextEncoder();
const elements = Object.fromEntries([
  "readiness", "run-button", "restart-button", "export-button", "file-count", "new-module-name", "add-module-button",
  "file-list", "delete-module-button", "asset-input", "asset-count", "asset-list", "asset-sample-button",
  "editor-file-name", "source-editor", "source-highlight", "editor-completions", "diagnostics", "generation-badge", "preview-frame-host", "preview-layout",
].map(id => [id.replace(/-([a-z])/g, (_match, letter) => letter.toUpperCase()), document.getElementById(id)]));

const files = new Map([["main.stasis", DEFAULT_SOURCE]]);
const assets = new ProjectAssetStore();
const pending = new Map();
const pendingAnalyses = new Map();
let stdlibFiles = [];
let runtimeScript = "";
let currentPath = "main.stasis";
let nextRequestId = 0;
let workerFailure = null;
let editorStatus = "Loading local compiler and runtime...";
let lastError = "";
let lastCompilation = null;
let iframe = null;
let running = false;
let activeLayoutDigest = null;
let selectedAssetPath = null;
let nextSessionOperationId = 0;
let currentSessionOperation = null;
let analysisRevision = 0;
let analysisTimer = null;
let currentEditorAnalysis = null;
let activeCompletionIndex = 0;
let suppressedCompletionRevision = null;
let lastEditorQuery = "";

const worker = new Worker("./compiler_worker.js", { type: "module" });
let resolveWorkerReady;
let rejectWorkerReady;
const workerReady = new Promise((resolve, reject) => {
  resolveWorkerReady = resolve;
  rejectWorkerReady = reject;
});

function rejectAllWorkerRequests(error) {
  for (const request of pending.values()) request.reject(error);
  pending.clear();
  for (const request of pendingAnalyses.values()) request.reject(error);
  pendingAnalyses.clear();
}

function failCompilerWorker(error) {
  if (!workerFailure) workerFailure = error;
  rejectWorkerReady(error);
  rejectAllWorkerRequests(error);
  if (analysisTimer !== null) clearTimeout(analysisTimer);
  analysisTimer = null;
  currentEditorAnalysis = null;
  hideCompletionList();
  if (api.isReady) {
    api.isReady = false;
    updateRunState();
    status(error.message || "compiler worker failed", "error");
  }
}

function describeWorkerError(event) {
  const message = event.message || event.error?.message || "compiler worker failed to load or execute";
  const filename = typeof event.filename === "string" ? event.filename : "";
  const line = Number.isInteger(event.lineno) && event.lineno > 0 ? event.lineno : null;
  const column = Number.isInteger(event.colno) && event.colno > 0 ? event.colno : null;
  let location = filename;
  if (line !== null) location += `:${line}`;
  if (column !== null) location += `:${column}`;
  return new Error(location ? `${message} (${location})` : message);
}

worker.addEventListener("message", event => {
  const response = event.data;
  if (response?.type === "ready") {
    resolveWorkerReady(response);
    return;
  }
  if (response?.type === "fatal") {
    const error = new Error(response.message || "compiler worker failed to start");
    failCompilerWorker(error);
    return;
  }
  if (response?.type === "editor-analysis" || response?.type === "editor-analysis-error") {
    const request = pendingAnalyses.get(response?.requestId);
    if (!request) return;
    pendingAnalyses.delete(response.requestId);
    if (response.type === "editor-analysis") request.resolve(response);
    else request.reject(new Error(response.message || "editor analysis failed"));
    return;
  }
  const request = pending.get(response?.requestId);
  if (!request) return;
  pending.delete(response.requestId);
  if (response.type === "compiled") request.resolve(response);
  else request.reject(new Error(response.message || "project compilation failed"));
});
worker.addEventListener("error", event => {
  failCompilerWorker(describeWorkerError(event));
});
worker.addEventListener("messageerror", event => {
  failCompilerWorker(describeWorkerError({
    ...event,
    message: "compiler worker message could not be decoded",
  }));
});

function safeJson(value) {
  return JSON.stringify(value).replace(/[<>&\u2028\u2029]/g, character => ({
    "<": "\\u003c", ">": "\\u003e", "&": "\\u0026", "\u2028": "\\u2028", "\u2029": "\\u2029",
  })[character]);
}

function safeBytes(value) {
  if (value instanceof Uint8Array) return value.slice();
  if (value instanceof ArrayBuffer) return new Uint8Array(value.slice(0));
  if (ArrayBuffer.isView(value)) return new Uint8Array(value.buffer.slice(value.byteOffset, value.byteOffset + value.byteLength));
  throw new Error("compiler returned no WebAssembly module");
}

function toBase64(bytes) {
  let binary = "";
  const chunkSize = 0x8000;
  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + chunkSize));
  }
  return btoa(binary);
}

function safeModulePath(path) {
  if (typeof path !== "string") throw new Error("module name is required");
  const normalized = path.trim();
  if (normalized.length > 120 || !/^[A-Za-z0-9][A-Za-z0-9._/-]*\.stasis$/.test(normalized)
      || normalized.startsWith("/") || normalized.includes("\\")
      || normalized.split("/").some(segment => !segment || segment === "." || segment === "..")
      || normalized.startsWith("stdlib/") || normalized.startsWith("vendor/stasis/stdlib/")) {
    throw new Error("module names must be relative .stasis paths outside reserved standard library directories");
  }
  return normalized;
}

function status(message, kind = "info") {
  editorStatus = String(message);
  lastError = kind === "error" ? editorStatus : "";
  elements.readiness.textContent = editorStatus;
  elements.readiness.dataset.state = kind === "ready" ? "ready" : kind === "error" ? "error" : "";
  elements.diagnostics.textContent = editorStatus;
  elements.diagnostics.dataset.state = kind === "error" ? "error" : kind === "success" ? "success" : "";
}

function updateRunState() {
  const ready = Boolean(window.STASIS_PLAYGROUND_EDITOR?.isReady);
  elements.runButton.disabled = !ready;
  elements.restartButton.disabled = !ready;
  elements.exportButton.disabled = !ready;
}

function setCurrentEditorValue() {
  if (currentPath && files.has(currentPath)) files.set(currentPath, elements.sourceEditor.value);
}

function renderFileList() {
  const fragment = document.createDocumentFragment();
  for (const path of files.keys()) {
    const item = document.createElement("li");
    const button = document.createElement("button");
    button.type = "button";
    button.className = "file-button";
    button.textContent = path;
    if (path === currentPath) button.setAttribute("aria-current", "page");
    button.addEventListener("click", () => selectFile(path));
    item.append(button);
    fragment.append(item);
  }
  elements.fileList.replaceChildren(fragment);
  elements.fileCount.textContent = String(files.size);
  elements.deleteModuleButton.disabled = currentPath === "main.stasis" || files.size <= 1;
}

function selectFile(path) {
  setCurrentEditorValue();
  currentPath = path;
  elements.editorFileName.textContent = path;
  elements.sourceEditor.value = files.get(path) || "";
  elements.sourceEditor.setSelectionRange(0, 0);
  showPlainEditorText();
  lastEditorQuery = editorQueryKey();
  scheduleEditorAnalysis();
  renderFileList();
}

function renderAssetList() {
  const fragment = document.createDocumentFragment();
  const list = assets.list();
  for (const asset of list) {
    const item = document.createElement("li");
    item.className = "asset-item";
    const image = document.createElement("img");
    image.src = asset.previewUrl;
    image.alt = "";
    const info = document.createElement("div");
    info.className = "asset-info";
    const name = document.createElement("div");
    name.className = "asset-name";
    name.textContent = asset.name;
    const details = document.createElement("div");
    details.className = "asset-details";
    details.textContent = `${asset.width} × ${asset.height} · ${(asset.byteLength / 1024).toFixed(1)} KiB`;
    info.append(name, details);
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "asset-remove";
    remove.textContent = "×";
    remove.setAttribute("aria-label", `Remove ${asset.name}`);
    remove.addEventListener("click", () => removeAsset(asset.path));
    item.append(image, info, remove);
    item.addEventListener("click", event => {
      if (event.target === remove) return;
      selectedAssetPath = asset.path;
      elements.assetSampleButton.dataset.path = asset.path;
    });
    fragment.append(item);
  }
  elements.assetList.replaceChildren(fragment);
  elements.assetCount.textContent = String(list.length);
  elements.assetSampleButton.hidden = list.length === 0;
  if (!list.some(asset => asset.path === selectedAssetPath)) selectedAssetPath = list[0]?.path || null;
  if (selectedAssetPath) elements.assetSampleButton.dataset.path = selectedAssetPath;
}

function updateProjectFromEditor({ suppressCompletions = false } = {}) {
  setCurrentEditorValue();
  renderFileList();
  showPlainEditorText();
  lastEditorQuery = editorQueryKey();
  scheduleEditorAnalysis();
  if (suppressCompletions) suppressedCompletionRevision = analysisRevision;
}

function cloneJson(value) {
  return value === undefined ? undefined : structuredClone(value);
}

async function fetchStaticDependencies() {
  const [runtimeResponse, libraryResponse] = await Promise.all([
    fetch("./game.js", { cache: "no-store" }),
    fetch("./stdlib.json", { cache: "no-store" }),
  ]);
  if (!runtimeResponse.ok) throw new Error(`local production runtime request failed with HTTP ${runtimeResponse.status}`);
  if (!libraryResponse.ok) throw new Error(`local standard library request failed with HTTP ${libraryResponse.status}`);
  runtimeScript = await runtimeResponse.text();
  if (!runtimeScript.includes("__STASIS_WASM_URL__")) throw new Error("local production runtime is missing its wasm URL marker");
  const libraryPayload = await libraryResponse.json();
  const rawFiles = Array.isArray(libraryPayload) ? libraryPayload : libraryPayload?.files;
  if (!Array.isArray(rawFiles) || rawFiles.length === 0) throw new Error("local Stasis standard library file map is empty");
  stdlibFiles = rawFiles.map(file => {
    const path = String(file.path || "");
    if (!path.startsWith("vendor/stasis/stdlib/") || !path.endsWith(".stasis") || typeof file.source !== "string") {
      throw new Error(`invalid standard library entry ${path || "(unnamed)"}`);
    }
    return { path, source: file.source };
  }).sort((left, right) => left.path.localeCompare(right.path));
}

function allSourceFiles() {
  setCurrentEditorValue();
  if (!files.has("main.stasis")) throw new Error("the project entry file main.stasis is required");
  if (files.size > MAX_PROJECT_FILES) throw new Error(`a project may contain at most ${MAX_PROJECT_FILES} source files`);
  let userSourceBytes = 0;
  const userFiles = Array.from(files, ([path, source]) => {
    const safePath = safeModulePath(path);
    const sourceBytes = encoder.encode(source).byteLength;
    if (sourceBytes > MAX_SOURCE_BYTES) throw new Error(`${path} exceeds the 1 MiB source limit`);
    userSourceBytes += sourceBytes;
    return { path: safePath, source };
  });
  if (userSourceBytes > MAX_PROJECT_SOURCE_BYTES) throw new Error("project source exceeds the 4 MiB limit");
  const merged = [...stdlibFiles, ...userFiles].sort((left, right) => left.path.localeCompare(right.path));
  const names = new Set();
  let combinedSourceBytes = 0;
  for (const file of merged) {
    if (names.has(file.path)) throw new Error(`duplicate project source path ${file.path}`);
    names.add(file.path);
    combinedSourceBytes += encoder.encode(file.source).byteLength;
  }
  if (merged.length > 128) throw new Error("project and standard library exceed the 128 file compilation limit");
  if (combinedSourceBytes > MAX_PROJECT_SOURCE_BYTES) throw new Error("project and standard library exceed the 4 MiB source limit");
  return merged;
}

function syncEditorScroll() {
  elements.sourceHighlight.scrollTop = elements.sourceEditor.scrollTop;
  elements.sourceHighlight.scrollLeft = elements.sourceEditor.scrollLeft;
}

function showPlainEditorText(source = elements.sourceEditor.value) {
  elements.sourceHighlight.textContent = source;
  syncEditorScroll();
}

function hideCompletionList() {
  elements.editorCompletions.hidden = true;
  elements.editorCompletions.replaceChildren();
  elements.sourceEditor.setAttribute("aria-expanded", "false");
  elements.sourceEditor.removeAttribute("aria-activedescendant");
  activeCompletionIndex = 0;
}

function analysisIdentity(force = false) {
  return {
    revision: analysisRevision,
    path: currentPath,
    source: elements.sourceEditor.value,
    cursor: elements.sourceEditor.selectionStart ?? 0,
    selectionEnd: elements.sourceEditor.selectionEnd ?? elements.sourceEditor.selectionStart ?? 0,
    force,
  };
}

function isAnalysisIdentityCurrent(identity) {
  const current = analysisIdentity(identity.force);
  return isCurrentEditorAnalysis(identity, current) && identity.selectionEnd === current.selectionEnd;
}

function cancelPendingAnalyses() {
  for (const request of pendingAnalyses.values()) request.resolve(null);
  pendingAnalyses.clear();
}

function requestEditorAnalysis(identity, projectFiles) {
  if (workerFailure) return Promise.reject(new Error(workerFailure.message || "compiler worker failed"));
  const requestId = ++nextRequestId;
  const result = new Promise((resolve, reject) => pendingAnalyses.set(requestId, { resolve, reject }));
  try {
    worker.postMessage({
      type: "analyze-editor", requestId, path: identity.path, source: identity.source,
      files: projectFiles, cursor: identity.cursor,
    });
  } catch (error) {
    pendingAnalyses.delete(requestId);
    return Promise.reject(error);
  }
  return result;
}

function renderCompletionList(analysis) {
  const completions = Array.isArray(analysis.completions) ? analysis.completions.slice(0, 100) : [];
  if (!completions.length) {
    hideCompletionList();
    return;
  }
  const fragment = document.createDocumentFragment();
  completions.forEach((completion, index) => {
    const option = document.createElement("li");
    const optionId = `editor-completion-option-${analysis.revision}-${index}`;
    option.id = optionId;
    option.className = "editor-completion-option";
    option.setAttribute("role", "option");
    option.setAttribute("aria-selected", index === 0 ? "true" : "false");
    option.setAttribute("data-completion-index", String(index));
    const label = document.createElement("span");
    label.className = "editor-completion-label";
    label.textContent = String(completion.label ?? completion.insertText ?? "");
    const kind = document.createElement("span");
    kind.className = "editor-completion-kind";
    kind.textContent = String(completion.kind ?? "symbol");
    option.append(label, kind);
    const detailText = completion.signature || completion.detail;
    if (detailText) {
      const detail = document.createElement("span");
      detail.className = "editor-completion-detail";
      detail.textContent = String(detailText);
      option.append(detail);
    }
    option.addEventListener("mousedown", event => event.preventDefault());
    option.addEventListener("click", () => acceptCompletion(index));
    fragment.append(option);
  });
  elements.editorCompletions.replaceChildren(fragment);
  activeCompletionIndex = 0;
  elements.editorCompletions.hidden = false;
  elements.sourceEditor.setAttribute("aria-expanded", "true");
  elements.sourceEditor.setAttribute("aria-activedescendant", `editor-completion-option-${analysis.revision}-0`);
}

function setActiveCompletion(index) {
  const options = Array.from(elements.editorCompletions.children);
  if (!options.length) return;
  activeCompletionIndex = (index + options.length) % options.length;
  options.forEach((option, optionIndex) => {
    option.setAttribute("aria-selected", optionIndex === activeCompletionIndex ? "true" : "false");
  });
  const option = options[activeCompletionIndex];
  elements.sourceEditor.setAttribute("aria-activedescendant", option.id);
  option.scrollIntoView?.({ block: "nearest" });
}

function acceptCompletion(index = activeCompletionIndex) {
  const analysis = currentEditorAnalysis;
  if (!analysis || elements.editorCompletions.hidden || !isAnalysisIdentityCurrent(analysis)) {
    hideCompletionList();
    return false;
  }
  const completion = analysis.completions?.[index];
  if (!completion || typeof completion.insertText !== "string") return false;
  const applied = applyEditorCompletion(elements.sourceEditor.value, analysis.replacementRange, completion.insertText);
  elements.sourceEditor.value = applied.source;
  elements.sourceEditor.setSelectionRange(applied.cursor, applied.cursor);
  updateProjectFromEditor({ suppressCompletions: true });
  elements.sourceEditor.focus();
  return true;
}

async function runEditorAnalysis(identity) {
  let projectFiles;
  try {
    projectFiles = allSourceFiles();
  } catch {
    return;
  }
  try {
    const response = await requestEditorAnalysis(identity, projectFiles);
    if (!response || !isAnalysisIdentityCurrent(identity) || response.path !== identity.path) return;
    const tokens = Array.isArray(response.tokens) ? response.tokens : [];
    const completions = Array.isArray(response.completions) ? response.completions : [];
    currentEditorAnalysis = {
      ...identity, tokens, completions,
      replacementRange: response.replacementRange || { start: identity.cursor, end: identity.cursor },
    };
    elements.sourceHighlight.replaceChildren(buildHighlightedFragment(identity.source, tokens, document));
    syncEditorScroll();
    const shouldSuggest = identity.force
      || (identity.selectionEnd === identity.cursor && isCompletionContext(identity.source, identity.cursor));
    if (shouldSuggest && suppressedCompletionRevision !== identity.revision) renderCompletionList(currentEditorAnalysis);
  } catch {
    if (isAnalysisIdentityCurrent(identity)) hideCompletionList();
  }
}

function scheduleEditorAnalysis({ force = false, immediate = false } = {}) {
  if (analysisTimer !== null) clearTimeout(analysisTimer);
  analysisTimer = null;
  analysisRevision += 1;
  suppressedCompletionRevision = null;
  currentEditorAnalysis = null;
  cancelPendingAnalyses();
  hideCompletionList();
  const identity = analysisIdentity(force);
  if (!api.isReady || workerFailure) return;
  const start = () => {
    if (identity.revision !== analysisRevision) return;
    analysisTimer = null;
    void runEditorAnalysis(identity);
  };
  if (immediate || force) start();
  else analysisTimer = setTimeout(start, 85);
}

function editorQueryKey() {
  const start = elements.sourceEditor.selectionStart ?? 0;
  const end = elements.sourceEditor.selectionEnd ?? start;
  return `${currentPath}\u0000${elements.sourceEditor.value}\u0000${start}\u0000${end}`;
}

function scheduleAnalysisForCaret() {
  const query = editorQueryKey();
  if (query === lastEditorQuery) return;
  lastEditorQuery = query;
  scheduleEditorAnalysis();
}

function handleEditorKeydown(event) {
  const action = editorKeyAction(event, !elements.editorCompletions.hidden);
  if (action === "run") {
    event.preventDefault();
    void runProject().catch(() => {});
  } else if (action === "open") {
    event.preventDefault();
    lastEditorQuery = editorQueryKey();
    scheduleEditorAnalysis({ force: true, immediate: true });
  } else if (action === "close") {
    event.preventDefault();
    suppressedCompletionRevision = analysisRevision;
    hideCompletionList();
  } else if (action === "next") {
    event.preventDefault();
    setActiveCompletion(activeCompletionIndex + 1);
  } else if (action === "previous") {
    event.preventDefault();
    setActiveCompletion(activeCompletionIndex - 1);
  } else if (action === "accept") {
    event.preventDefault();
    acceptCompletion();
  } else if (action === "indent") {
    event.preventDefault();
    const start = elements.sourceEditor.selectionStart;
    const end = elements.sourceEditor.selectionEnd;
    elements.sourceEditor.setRangeText("    ", start, end, "end");
    updateProjectFromEditor();
  }
}

function beginSessionOperation() {
  currentSessionOperation?.cancel();
  let resolveSuperseded;
  const operation = {
    id: ++nextSessionOperationId,
    superseded: new Promise(resolve => { resolveSuperseded = resolve; }),
    cancel() { resolveSuperseded(); },
  };
  currentSessionOperation = operation;
  return operation;
}

function isCurrentSessionOperation(operation) {
  return operation === currentSessionOperation;
}

function assertCurrentSessionOperation(operation) {
  if (!isCurrentSessionOperation(operation)) throw new Error("playground operation superseded");
}

async function awaitSessionOperation(operation, pendingResult) {
  const outcome = await Promise.race([
    Promise.resolve(pendingResult).then(value => ({ value }), error => ({ error })),
    operation.superseded.then(() => ({ superseded: true })),
  ]);
  if (outcome.superseded || !isCurrentSessionOperation(operation)) {
    throw new Error("playground operation superseded");
  }
  if (Object.hasOwn(outcome, "error")) throw outcome.error;
  return outcome.value;
}

async function requestCompile(operation = null) {
  if (operation) await awaitSessionOperation(operation, api.ready);
  else await api.ready;
  if (workerFailure) throw new Error(workerFailure.message || "compiler worker failed");
  if (operation) assertCurrentSessionOperation(operation);
  const requestFiles = allSourceFiles();
  const requestId = ++nextRequestId;
  const result = new Promise((resolve, reject) => pending.set(requestId, { resolve, reject }));
  try {
    if (operation) assertCurrentSessionOperation(operation);
    worker.postMessage({ type: "compile-project", requestId, entry: "main.stasis", files: requestFiles });
    const response = operation ? await awaitSessionOperation(operation, result) : await result;
    const gameWasm = safeBytes(response.gameWasm);
    if (!response.metadata || !Array.isArray(response.metadata.imports)) throw new Error("compiler returned incomplete playground metadata");
    const compilation = { gameWasm, metadata: response.metadata };
    lastCompilation = compilation;
    return { gameWasm: gameWasm.slice(), metadata: cloneJson(response.metadata) };
  } catch (error) {
    pending.delete(requestId);
    throw new Error(error?.message || String(error));
  }
}

function compilationWithAssets(compilation) {
  const gameWasm = safeBytes(compilation.gameWasm);
  const metadata = cloneJson(compilation.metadata);
  if (!metadata || !metadata.config) throw new Error("compiler response did not include a runtime configuration");
  const overrides = assets.configOverrides();
  metadata.config.assets = overrides.assets;
  metadata.config.asset_urls = {};
  metadata.config.asset_metadata = overrides.asset_metadata;
  return { gameWasm, metadata, config: metadata.config };
}

function iframeHtml(compilation) {
  const boot = compilationWithAssets(compilation);
  const code = `(() => {\n` +
    `window.STASIS_GAME = ${safeJson(boot.config)};\n` +
    `const encoded = ${JSON.stringify(toBase64(boot.gameWasm))};\n` +
    `const binary = atob(encoded);\n` +
    `const wasmBytes = new Uint8Array(binary.length);\n` +
    `for (let i = 0; i < binary.length; i += 1) wasmBytes[i] = binary.charCodeAt(i);\n` +
    `window.STASIS_PLAYGROUND_BOOT = { wasmBytes, layoutDigest: ${safeJson(boot.metadata.layoutDigest)}, metadata: ${safeJson(boot.metadata)} };\n` +
    `const runtime = ${JSON.stringify(runtimeScript)};\n` +
    `const scriptUrl = URL.createObjectURL(new Blob([runtime], { type: "text/javascript" }));\n` +
    `const script = document.createElement("script");\n` +
    `script.src = scriptUrl;\n` +
    `window.__STASIS_RUNTIME_SCRIPT_READY__ = new Promise((resolve, reject) => { script.addEventListener("load", () => { URL.revokeObjectURL(scriptUrl); resolve(); }, { once: true }); script.addEventListener("error", () => reject(new Error("local runtime script failed to load")), { once: true }); });\n` +
    `document.head.append(script);\n` +
    `})();`;
  return `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline' blob: 'wasm-unsafe-eval'; connect-src blob:; img-src blob: data:; media-src blob: data:; font-src blob: data:; style-src 'unsafe-inline'; object-src 'none'; base-uri 'none'; form-action 'none"><title>Stasis game preview</title><style>html,body{width:100%;height:100%;margin:0;overflow:hidden;background:#081613}body{display:grid;place-items:center}main{position:relative;width:100%;max-width:100%;aspect-ratio:16 / 9}canvas{display:block;width:100%;height:100%;background:#091b2d;touch-action:none}#stasis-loading{position:fixed;inset:0;z-index:10;display:grid;place-items:center;background:#081613;color:#d6c17c;font:14px system-ui}#stasis-loading[data-hidden="true"]{display:none}#stasis-error{position:absolute;inset:0;margin:0;padding:1rem;overflow:auto;color:#ff8f8f;white-space:pre-wrap;pointer-events:none}#stasis-error:empty{display:none}</style></head><body><main><canvas id="stasis-canvas" width="640" height="360" data-logical-width="640" data-logical-height="360" aria-label="Stasis game canvas" tabindex="0"></canvas><div id="stasis-loading" role="status"><span id="stasis-loading-status">Loading game...</span></div><div id="stasis-ad-controls" hidden></div><button id="stasis-ad-watch" hidden></button><button id="stasis-ad-cancel" hidden></button><div id="stasis-ad-status" hidden></div><div id="stasis-hud" hidden></div><pre id="stasis-error"></pre></main><script>${code.replace(/<\/script/gi, "<\\/script")}</script></body></html>`;
}

function runtimeGeneration() {
  try {
    const value = iframe?.contentWindow?.STASIS_PLAYGROUND?.generation;
    return typeof value === "function" ? value() : Number.isFinite(value) ? value : null;
  } catch { return null; }
}

function updateRunningUi(layoutDigest) {
  running = true;
  activeLayoutDigest = layoutDigest || null;
  elements.generationBadge.textContent = `Generation ${runtimeGeneration() ?? "active"}`;
  elements.generationBadge.dataset.running = "true";
  elements.previewLayout.textContent = layoutDigest ? "Run again to hot swap" : "Running";
}

function waitForFrameLoad(frame, operation) {
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error("game preview did not start within 20 seconds")), 20000);
    operation.superseded.then(() => clearTimeout(timeout));
    frame.addEventListener("load", () => { clearTimeout(timeout); resolve(); }, { once: true });
    frame.addEventListener("error", () => { clearTimeout(timeout); reject(new Error("game preview frame failed to load")); }, { once: true });
  });
}

async function waitForRuntime(frame) {
  const child = frame.contentWindow;
  if (child?.__STASIS_RUNTIME_SCRIPT_READY__?.then) await child.__STASIS_RUNTIME_SCRIPT_READY__;
  const runtimeReady = child?.STASIS_PLAYGROUND?.ready;
  if (runtimeReady && typeof runtimeReady.then === "function") await runtimeReady;
  else if (child?.STASIS_RUNTIME_PROMISE && typeof child.STASIS_RUNTIME_PROMISE.then === "function") await child.STASIS_RUNTIME_PROMISE;
  else if (!child?.STASIS_PLAYGROUND) throw new Error("production runtime did not expose the playground host API");
}

async function startFreshFrame(compilation, operation) {
  assertCurrentSessionOperation(operation);
  const boot = compilationWithAssets(compilation);
  const frame = document.createElement("iframe");
  frame.title = "Stasis game preview";
  frame.setAttribute("sandbox", "allow-scripts allow-same-origin");
  frame.className = "preview-candidate";
  const loaded = waitForFrameLoad(frame, operation);
  frame.srcdoc = iframeHtml({ gameWasm: boot.gameWasm, metadata: boot.metadata });
  elements.previewFrameHost.append(frame);
  try {
    await awaitSessionOperation(operation, loaded);
    await awaitSessionOperation(operation, waitForRuntime(frame));
  } catch (error) {
    frame.remove();
    if (!isCurrentSessionOperation(operation)) throw new Error("playground operation superseded");
    throw error;
  }
  assertCurrentSessionOperation(operation);
  const hadPreviousFrame = Boolean(iframe);
  for (const child of Array.from(elements.previewFrameHost.children)) {
    if (child !== frame) child.remove();
  }
  frame.classList.remove("preview-candidate");
  iframe = frame;
  updateRunningUi(boot.metadata.layoutDigest);
  return { mode: hadPreviousFrame ? "restarted" : "booted", generation: runtimeGeneration(), layoutDigest: boot.metadata.layoutDigest, metadata: cloneJson(boot.metadata) };
}

async function applyCompilation(compilation, operation) {
  assertCurrentSessionOperation(operation);
  const boot = compilationWithAssets(compilation);
  if (!iframe) return startFreshFrame(compilation, operation);
  const runtime = iframe.contentWindow?.STASIS_PLAYGROUND;
  if (!runtime || typeof runtime.swap !== "function") throw new Error("production runtime cannot hot-swap this game");
  const result = await awaitSessionOperation(operation, runtime.swap({
    wasmBytes: boot.gameWasm.slice(), config: cloneJson(boot.config),
    layoutDigest: boot.metadata.layoutDigest, metadata: cloneJson(boot.metadata),
  }));
  assertCurrentSessionOperation(operation);
  updateRunningUi(boot.metadata.layoutDigest);
  return { mode: "swapped", generation: runtimeGeneration(), layoutDigest: boot.metadata.layoutDigest, metadata: cloneJson(boot.metadata), result };
}

async function runProject() {
  const operation = beginSessionOperation();
  status("Compiling project in the browser...", "info");
  try {
    const compilation = await requestCompile(operation);
    assertCurrentSessionOperation(operation);
    const result = await applyCompilation(compilation, operation);
    assertCurrentSessionOperation(operation);
    status(result.mode === "booted" ? "Game started. Edit source and run again to hot-swap." : "Hot-swap accepted; the game session is still running.", "success");
    updateRunningUi(result.layoutDigest);
    return result;
  } catch (error) {
    const message = error?.message || String(error);
    if (isCurrentSessionOperation(operation)) status(message, "error");
    throw error;
  }
}

async function restartProject() {
  const operation = beginSessionOperation();
  status("Compiling a fresh game session...", "info");
  try {
    const compilation = await requestCompile(operation);
    assertCurrentSessionOperation(operation);
    const result = await startFreshFrame(compilation, operation);
    assertCurrentSessionOperation(operation);
    status("Fresh game session started.", "success");
    return result;
  } catch (error) {
    if (isCurrentSessionOperation(operation)) status(error?.message || String(error), "error");
    throw error;
  }
}

function createSpriteExample(path) {
  const escaped = JSON.stringify(path);
  return `import "vendor/stasis/stdlib/graphics.stasis";

function @extern("web_input_axis") web_input_axis(): i32;

global sample_sprite: Sprite;
global sprite_x: i32;

function main(): i32 {
    sprite_x = 272;
    sample_sprite.load_sprite_from(${escaped}, 96, 96);
    return 0;
}

function tick(): i32 {
    sprite_x += web_input_axis() * 3;
    if (sprite_x < 0) {
        sprite_x = 0;
    }
    if (sprite_x > 544) {
        sprite_x = 544;
    }
    return 0;
}

function render(): i32 {
    let draw_x: f32 = 0.0;
    draw_x.from_i32(sprite_x);
    clear(0.04, 0.09, 0.11, 1.0);
    fill_rect(0.0, 292.0, 640.0, 68.0, 0.08, 0.18, 0.19, 1.0);
    sample_sprite.draw(draw_x, 150.0, 255, 0);
    return 0;
}
`;
}

async function importAsset(file) {
  const asset = await assets.importFile(file);
  selectedAssetPath = asset.path;
  renderAssetList();
  status(`Imported ${asset.name} (${asset.width} × ${asset.height}, ${asset.byteLength} bytes).`, "success");
  return asset;
}

function removeAsset(path) {
  const removed = assets.remove(path);
  if (removed) {
    renderAssetList();
    status(`Removed ${path}.`, "success");
  }
  return removed;
}

async function exportProject() {
  status("Compiling project for offline export...", "info");
  try {
    const compilation = await requestCompile();
    const entries = assets.exportEntries().map(asset => ({ ...asset, ...assets.list().find(item => item.path === asset.path) }));
    const blob = await exportProjectZip({
      entry: "main.stasis", files: Array.from(files, ([path, source]) => ({ path, source })).concat(stdlibFiles),
      assets: entries, metadata: compilation.metadata, gameWasm: compilation.gameWasm, runtimeScript,
    });
    status(`Export ready (${(blob.size / 1024).toFixed(1)} KiB).`, "success");
    return blob;
  } catch (error) {
    status(error?.message || String(error), "error");
    throw error;
  }
}

const api = {
  ready: null,
  isReady: false,
  getProject() {
    setCurrentEditorValue();
    return { entry: "main.stasis", files: Array.from(files, ([path, source]) => ({ path, source })), assets: assets.list().map(({ previewUrl, ...asset }) => asset), status: editorStatus };
  },
  getFile(path) {
    const safe = safeModulePath(path);
    return files.has(safe) ? { path: safe, source: files.get(safe) } : null;
  },
  setFile(path, source) {
    const safe = safeModulePath(path);
    if (typeof source !== "string") throw new Error("module source must be text");
    if (encoder.encode(source).byteLength > MAX_SOURCE_BYTES) throw new Error("a module may contain at most 1 MiB of source");
    if (!files.has(safe) && files.size >= MAX_PROJECT_FILES) throw new Error(`a project may contain at most ${MAX_PROJECT_FILES} source files`);
    files.set(safe, source);
    if (safe === currentPath) {
      const cursor = Math.min(elements.sourceEditor.selectionStart ?? 0, source.length);
      elements.sourceEditor.value = source;
      elements.sourceEditor.setSelectionRange(cursor, cursor);
    }
    showPlainEditorText();
    lastEditorQuery = editorQueryKey();
    scheduleEditorAnalysis();
    renderFileList();
    return { path: safe, source };
  },
  deleteFile(path) {
    const safe = safeModulePath(path);
    if (safe === "main.stasis") {
      files.set(safe, "");
      if (currentPath === safe) {
        elements.sourceEditor.value = "";
        elements.sourceEditor.setSelectionRange(0, 0);
        showPlainEditorText();
      }
      lastEditorQuery = editorQueryKey();
      scheduleEditorAnalysis();
      renderFileList();
      return true;
    }
    if (!files.has(safe)) return false;
    files.delete(safe);
    if (currentPath === safe) selectFile(files.keys().next().value);
    else {
      renderFileList();
      showPlainEditorText();
      lastEditorQuery = editorQueryKey();
      scheduleEditorAnalysis();
    }
    return true;
  },
  setEntry(path) {
    const safe = safeModulePath(path);
    if (!files.has(safe)) throw new Error(`entry module ${safe} does not exist`);
    if (safe !== "main.stasis") throw new Error("the browser compiler entry must be named main.stasis");
    return safe;
  },
  async compile() {
    status("Compiling project in the browser...", "info");
    try {
      const result = await requestCompile();
      status(`Compiled ${files.size} project module${files.size === 1 ? "" : "s"}.`, "success");
      return result;
    } catch (error) {
      status(error?.message || String(error), "error");
      throw error;
    }
  },
  async run() { return runProject(); },
  async restart() { return restartProject(); },
  async swap(compilation) {
    const operation = beginSessionOperation();
    status("Compiling project in the browser...", "info");
    try {
      const selected = compilation || await requestCompile(operation);
      assertCurrentSessionOperation(operation);
      const result = await applyCompilation(selected, operation);
      assertCurrentSessionOperation(operation);
      status(result.mode === "booted" ? "Game started." : "Hot-swap accepted; the previous session continues.", "success");
      return result;
    } catch (error) {
      if (isCurrentSessionOperation(operation)) status(error?.message || String(error), "error");
      throw error;
    }
  },
  importAsset,
  removeAsset,
  getAssets() { return assets.list().map(({ previewUrl, ...asset }) => asset); },
  getAsset(path) {
    const safePath = String(path);
    const asset = assets.list().find(candidate => candidate.path === safePath);
    if (!asset) return null;
    const { previewUrl, ...record } = asset;
    return record;
  },
  exportProject,
  getState() {
    return { ready: api.isReady, running, status: editorStatus, error: lastError, generation: runtimeGeneration(), layoutDigest: activeLayoutDigest };
  },
  getEditorView() {
    const analysis = currentEditorAnalysis;
    return {
      path: currentPath, source: elements.sourceEditor.value,
      analysisPending: analysisTimer !== null || pendingAnalyses.size > 0,
      tokens: analysis?.tokens || [], completions: analysis?.completions || [],
      replacementRange: analysis?.replacementRange || null,
      completionOpen: !elements.editorCompletions.hidden,
      activeCompletionIndex,
    };
  },
  refreshEditorAnalysis(options = {}) {
    lastEditorQuery = editorQueryKey();
    scheduleEditorAnalysis({ force: Boolean(options.force), immediate: Boolean(options.immediate) });
  },
  getIframe() { return iframe; },
};
window.STASIS_PLAYGROUND_EDITOR = api;

elements.sourceEditor.value = DEFAULT_SOURCE;
showPlainEditorText(DEFAULT_SOURCE);
elements.sourceEditor.addEventListener("input", updateProjectFromEditor);
elements.sourceEditor.addEventListener("keydown", handleEditorKeydown);
elements.sourceEditor.addEventListener("keyup", scheduleAnalysisForCaret);
elements.sourceEditor.addEventListener("click", scheduleAnalysisForCaret);
elements.sourceEditor.addEventListener("select", scheduleAnalysisForCaret);
elements.sourceEditor.addEventListener("scroll", syncEditorScroll, { passive: true });
elements.sourceEditor.addEventListener("blur", hideCompletionList);
elements.runButton.addEventListener("click", () => void runProject().catch(() => {}));
elements.restartButton.addEventListener("click", () => void restartProject().catch(() => {}));
elements.exportButton.addEventListener("click", async () => {
  try {
    const blob = await exportProject();
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = "stasis-playground.zip";
    anchor.click();
    setTimeout(() => URL.revokeObjectURL(url), 30000);
  } catch {}
});
elements.addModuleButton.addEventListener("click", () => {
  const path = elements.newModuleName.value;
  try {
    const safe = safeModulePath(path);
    if (files.has(safe)) throw new Error(`${safe} already exists`);
    api.setFile(safe, "// New Stasis module\n");
    elements.newModuleName.value = "";
    selectFile(safe);
    status(`Added ${safe}.`, "success");
  } catch (error) {
    status(error?.message || String(error), "error");
  }
});
elements.newModuleName.addEventListener("keydown", event => {
  if (event.key === "Enter") { event.preventDefault(); elements.addModuleButton.click(); }
});
elements.deleteModuleButton.addEventListener("click", () => {
  if (currentPath !== "main.stasis") api.deleteFile(currentPath);
});
elements.assetInput.addEventListener("change", async () => {
  const chosen = Array.from(elements.assetInput.files || []);
  for (const file of chosen) {
    try { await importAsset(file); }
    catch (error) { status(`${file.name}: ${error?.message || error}`, "error"); }
  }
  elements.assetInput.value = "";
});
elements.assetSampleButton.addEventListener("click", () => {
  const selected = selectedAssetPath || elements.assetSampleButton.dataset.path;
  if (!selected || !assets.has(selected)) return;
  api.setFile("main.stasis", createSpriteExample(selected));
  selectFile("main.stasis");
  status(`Loaded asset example for ${selected}. Run to see the uploaded image in the game.`, "success");
});

renderFileList();
renderAssetList();
api.ready = Promise.all([workerReady, fetchStaticDependencies()]).then(([workerInfo]) => {
  api.isReady = true;
  updateRunState();
  status(`Ready · ${stdlibFiles.length} local standard modules`, "ready");
  lastEditorQuery = editorQueryKey();
  scheduleEditorAnalysis({ immediate: true });
  return { abiVersion: workerInfo.abiVersion };
}).catch(error => {
  api.isReady = false;
  updateRunState();
  status(error?.message || String(error), "error");
  throw error;
});
api.ready.catch(() => {});
