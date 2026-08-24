(() => {
  "use strict";
  const canvas = document.getElementById("stasis-canvas");
  const context = canvas.getContext("2d", { alpha: false });
  const hud = document.getElementById("stasis-hud");
  const errorBox = document.getElementById("stasis-error");
  const loadingBox = document.getElementById("stasis-loading");
  const loadingStatus = document.getElementById("stasis-loading-status");
  const setLoading = (message, state = "loading") => {
    if (!loadingBox) return;
    if (loadingStatus) loadingStatus.textContent = message;
    else loadingBox.textContent = message;
    loadingBox.dataset.failed = state === "failed" ? "true" : "false";
    loadingBox.dataset.hidden = state === "ready" ? "true" : "false";
  };
  const keys = new Set();
  const pointer = { id: 0, x: 0, y: 0, dx: 0, dy: 0, hover: false, down: false, wentDown: false, wentUp: false };
  const commands = [];
  const game = window.STASIS_GAME || { strings: {}, memory: {}, assets: {} };
  const sprites = new Map();
  const fonts = new Map();
  const fontLoads = new Map();
  const cachedText = new Map();
  let nextHandle = 1;
  let instance;
  // @stasis-feature network begin
  const NETWORK_MAX_MESSAGE = 64 * 1024;
  const NETWORK_MAX_BUFFERED = 1024 * 1024;
  const networkClient = {
    socket: null, state: 0, error: 0, queue: [], queuedBytes: 0,
    outbound: [], outboundBytes: 0,
    resume: null, desiredSeat: -1, lastSequence: 0
  };
  const networkLocation = () => globalThis.location || null;
  const networkBytesFromHex = value => {
    if (typeof value !== "string" || value.length !== 32 || !/^[0-9a-f]{32}$/i.test(value)) return null;
    const bytes = new Uint8Array(16);
    for (let index = 0; index < bytes.length; index += 1) bytes[index] = parseInt(value.slice(index * 2, index * 2 + 2), 16);
    return bytes;
  };
  const networkResumeCredential = () => {
    const key = `stasis:resume:${networkLocation()?.origin || "unknown"}`;
    let value = null;
    try { value = localStorage.getItem(key); } catch (_) { value = null; }
    if (!networkBytesFromHex(value)) {
      const bytes = new Uint8Array(16);
      crypto.getRandomValues(bytes);
      value = Array.from(bytes, byte => byte.toString(16).padStart(2, "0")).join("");
      try { localStorage.setItem(key, value); } catch (_) { /* ephemeral storage is valid */ }
    }
    return value;
  };
  const networkCheckpointKey = credential => {
    // Keep the raw resume credential out of storage keys and all Wasm-visible
    // values while still separating metadata between adapter identities.
    let hash = 2166136261;
    const source = `${networkLocation()?.origin || "unknown"}:${credential}`;
    for (let index = 0; index < source.length; index += 1) {
      hash = Math.imul(hash ^ source.charCodeAt(index), 16777619) | 0;
    }
    return `stasis:checkpoint:${(hash >>> 0).toString(16).padStart(8, "0")}`;
  };
  const networkLoadCheckpoint = () => {
    // Non-network runtime tests and ordinary packages may not provide a
    // browser location/WebSocket.  Do not allocate adapter credentials there.
    if (!networkLocation() || typeof WebSocket !== "function") return;
    const credential = networkResumeCredential();
    let raw = null;
    try { raw = localStorage.getItem(networkCheckpointKey(credential)); } catch (_) { raw = null; }
    try {
      const checkpoint = JSON.parse(raw || "null");
      if (checkpoint && Number.isInteger(checkpoint.seat) && checkpoint.seat >= -1 && checkpoint.seat < 8
          && Number.isInteger(checkpoint.lastSequence) && checkpoint.lastSequence >= 0
          && checkpoint.lastSequence <= 0x7fffffff) {
        networkClient.desiredSeat = checkpoint.seat;
        networkClient.lastSequence = checkpoint.lastSequence;
      }
    } catch (_) {
      // Malformed adapter metadata falls back to a fresh seat/sequence.
    }
  };
  const networkPairingSecret = () => {
    const currentLocation = networkLocation();
    if (!currentLocation) return null;
    const fragment = new URLSearchParams(currentLocation.hash.startsWith("#") ? currentLocation.hash.slice(1) : currentLocation.hash);
    const secret = fragment.get("secret");
    return typeof secret === "string" && /^[0-9a-f]{32,128}$/i.test(secret) ? secret.toLowerCase() : null;
  };
  const networkEnqueue = bytes => {
    if (!(bytes instanceof Uint8Array) || bytes.byteLength > NETWORK_MAX_MESSAGE
      || bytes.byteLength + networkClient.queuedBytes > NETWORK_MAX_BUFFERED
      || networkClient.queue.length >= 256) { networkClient.error = -3; return false; }
    networkClient.queue.push(bytes.slice());
    networkClient.queuedBytes += bytes.byteLength;
    return true;
  };
  const networkConnect = () => {
    if (networkClient.socket && (networkClient.socket.readyState === WebSocket.OPEN || networkClient.socket.readyState === WebSocket.CONNECTING)) return 0;
    const secret = networkPairingSecret();
    const currentLocation = networkLocation();
    if (!secret || !currentLocation || typeof WebSocket !== "function") { networkClient.state = -4; networkClient.error = -4; return -4; }
    const credential = networkResumeCredential();
    const protocol = `stasis-resume-v1.${credential}`;
    const socketUrl = `${currentLocation.protocol === "https:" ? "wss:" : "ws:"}//${currentLocation.host}/session`;
    try {
      const socket = new WebSocket(socketUrl, ["stasis-v1", secret, protocol]);
      socket.binaryType = "arraybuffer";
      socket.onopen = () => {
        networkClient.state = 1;
        networkClient.error = 0;
        while (networkClient.outbound.length > 0) {
          const bytes = networkClient.outbound.shift();
          networkClient.outboundBytes -= bytes.byteLength;
          try { socket.send(bytes); } catch (_) { networkClient.error = -2; break; }
        }
      };
      socket.onmessage = event => {
        const bytes = event.data instanceof ArrayBuffer ? new Uint8Array(event.data) : null;
        if (bytes) networkEnqueue(bytes);
        else networkClient.error = -2;
      };
      socket.onerror = () => { networkClient.state = -2; networkClient.error = -2; };
      socket.onclose = () => { networkClient.state = 0; networkClient.socket = null; };
      networkClient.socket = socket;
      networkClient.state = 2;
      return 0;
    } catch (_) { networkClient.state = -2; networkClient.error = -2; return -2; }
  };
  const networkPoll = (outId, capacity) => {
    const output = resolveU8Memory(outId);
    if (!output || !Number.isInteger(capacity) || capacity < 0 || capacity > output.length) return -1;
    const next = networkClient.queue[0];
    if (!next) return networkClient.error || 0;
    if (next.byteLength > capacity) return -1;
    for (let index = 0; index < next.byteLength; index += 1) writeU8(output, index, next[index]);
    networkClient.queue.shift();
    networkClient.queuedBytes -= next.byteLength;
    return next.byteLength;
  };
  const networkSend = (payloadId, length) => {
    const source = resolveU8Memory(payloadId);
    if (!source || !Number.isInteger(length) || length < 0 || length > NETWORK_MAX_MESSAGE || length > source.length) return -1;
    const bytes = new Uint8Array(length);
    for (let index = 0; index < length; index += 1) bytes[index] = readU8(source, index);
    if (!networkClient.socket || networkClient.socket.readyState === WebSocket.CONNECTING) {
      if (networkClient.outboundBytes + length > NETWORK_MAX_BUFFERED || networkClient.outbound.length >= 256) return -3;
      networkClient.outbound.push(bytes);
      networkClient.outboundBytes += length;
      return 0;
    }
    if (networkClient.socket.readyState !== WebSocket.OPEN) return -2;
    try { networkClient.socket.send(bytes); return 0; } catch (_) { networkClient.error = -2; return -2; }
  };
  const networkCheckpoint = (seat, lastSequence) => {
    if (!Number.isInteger(seat) || seat < -1 || seat >= 8
        || !Number.isInteger(lastSequence) || lastSequence < 0
        || lastSequence > 0x7fffffff) {
      networkClient.error = -1;
      return -1;
    }
    networkClient.desiredSeat = seat;
    networkClient.lastSequence = lastSequence;
    try {
      localStorage.setItem(networkCheckpointKey(networkResumeCredential()), JSON.stringify({ seat, lastSequence }));
    } catch (_) {
      // Storage denial leaves the in-memory checkpoint valid for this page.
    }
    return 0;
  };
  networkLoadCheckpoint();
  // @stasis-feature network end
  // @stasis-feature audio begin
  let audioContext;
  let audioEnablePromise;
  let audioEvents = 0;
  let audioSampleRate = 48000;
  let audioChannels = 2;
  let audioNextStart = 0;
  let audioUnderruns = 0;
  let audioSuspendedByLifecycle = false;
  let pendingAudioFrames = 0;
  let nextAudioVoiceHandle = 1;
  let nextAudioVoiceGeneration = 1;
  const audioAssets = new Map();
  const audioVoices = new Map();
  const pendingAudio = [];
  // Keep pre-gesture PCM short enough to unlock without replaying stale gameplay.
  const PENDING_AUDIO_SECONDS = 0.1;
  const PENDING_AUDIO_ENTRY_LIMIT = 32;
  const MAX_AUDIO_VOICES = 32;
  const MAX_AUDIO_VOICE_HANDLE = 0x7fffffff;
  // @stasis-feature audio end
  const assetTasks = new Map();
  let nextAssetTask = 1;
  const volatileStorage = new Map();
  let clipboardText = "";
  let frames = 0;
  let tickIndex = 0;
  let resized = true;
  let displayGeneration = 1;
  let densityGeneration = 1;
  let lastWindowRequest = -1;
  let pendingFullscreen;
  let worstTick = 0;
  let worstRender = 0;
  let worstWasmRender = 0;
  let worstBrowserReplay = 0;
  let worstFrameWork = 0;
  const PERF_ROLLING_CAPACITY = 1200;
  const performanceWorstTimes = new Float64Array(PERF_ROLLING_CAPACITY);
  const performanceWorstValues = Array.from({ length: 5 }, () => new Float64Array(PERF_ROLLING_CAPACITY));
  let performanceWorstNext = 0;
  let performanceWorstCount = 0;
  const recordPerformanceWorst = (now, tick, render, wasm, replay, frameWork) => {
    performanceWorstTimes[performanceWorstNext] = now;
    performanceWorstValues[0][performanceWorstNext] = tick;
    performanceWorstValues[1][performanceWorstNext] = render;
    performanceWorstValues[2][performanceWorstNext] = wasm;
    performanceWorstValues[3][performanceWorstNext] = replay;
    performanceWorstValues[4][performanceWorstNext] = frameWork;
    performanceWorstNext = (performanceWorstNext + 1) % PERF_ROLLING_CAPACITY;
    if (performanceWorstCount < PERF_ROLLING_CAPACITY) performanceWorstCount += 1;
    const cutoff = now - 5000;
    for (let metric = 0; metric < 5; metric += 1) {
      let maximum = 0;
      for (let sample = 0; sample < performanceWorstCount; sample += 1) {
        if (performanceWorstTimes[sample] >= cutoff) maximum = Math.max(maximum, performanceWorstValues[metric][sample]);
      }
      if (metric === 0) worstTick = maximum;
      else if (metric === 1) worstRender = maximum;
      else if (metric === 2) worstWasmRender = maximum;
      else if (metric === 3) worstBrowserReplay = maximum;
      else worstFrameWork = maximum;
    }
  };
  const performanceWorkload = {
    commands: 0, lines: 0, rectangles: 0, sprites: 0, text: 0,
    instances: 0, batches: 0, drawCalls: 0, uploadedBytes: 0
  };
  let performanceBackend = "Canvas2D";
  let rectBatcher;
  const RECT_BATCH_MIN = 64;
  const RECT_CAP = 10000;
  const rectScratch = new Float32Array(RECT_CAP * 8);
  const startedAt = performance.now();

  const colorCache = new Map();
  const color = (r, g, b) => {
    const red = r & 255;
    const green = g & 255;
    const blue = b & 255;
    const key = (red << 16) | (green << 8) | blue;
    let value = colorCache.get(key);
    if (!value) {
      value = `rgb(${red} ${green} ${blue})`;
      colorCache.set(key, value);
    }
    return value;
  };
  const unitColor = (r, g, b) => color(
    Math.max(0, Math.min(255, Math.round(r * 255))),
    Math.max(0, Math.min(255, Math.round(g * 255))),
    Math.max(0, Math.min(255, Math.round(b * 255)))
  );
  const stringValue = id => game.strings[String(id)] || "";
  const assetKey = value => {
    if (value === "/assets") return "assets";
    if (value.startsWith("/assets/")) return value.slice(1);
    return value.replace(/^(?:\.\.\/|\.\/)+/, "");
  };
  const assetValue = id => {
    const value = stringValue(id);
    const key = assetKey(value);
    return Object.prototype.hasOwnProperty.call(game.assets || {}, key)
      ? game.assets[key]
      : key;
  };
  const storageKey = (scopeId, keyId) => `stasis:${stringValue(scopeId)}:${stringValue(keyId)}`;
  const storageGet = key => {
    try { return localStorage.getItem(key); } catch (_) { return volatileStorage.get(key) ?? null; }
  };
  const storageSet = (key, value) => {
    try { localStorage.setItem(key, value); } catch (_) { volatileStorage.set(key, value); }
    return 1;
  };
  const readAscii = (offset, length) => {
    if (!instance?.exports.memory || offset < 0 || length < 0) return "";
    const bytes = new Uint8Array(instance.exports.memory.buffer, offset, length);
    return String.fromCharCode(...bytes);
  };
  const writeAscii = (offset, capacity, value) => {
    if (!instance?.exports.memory || offset < 0 || capacity <= 0) return -1;
    const bytes = Array.from(value, character => character.codePointAt(0));
    if (bytes.some(value => value < 32 || value > 126) || bytes.length >= capacity) return -1;
    const target = new Uint8Array(instance.exports.memory.buffer, offset, capacity);
    target.fill(0);
    target.set(bytes);
    return bytes.length;
  };
  const memoryLayouts = typeId => Object.values(game.memory || {})
    .filter(layout => layout?.type_id === typeId
      && Number.isSafeInteger(layout.hash));
  const memoryLayoutsByHash = typeId => new Map(
    memoryLayouts(typeId).map(layout => [layout.hash | 0, layout])
  );
  const memoryLayoutsByOffset = typeId => new Map(
    Object.values(game.memory || {})
      .filter(layout => layout?.type_id === typeId
        && Number.isSafeInteger(layout.offset))
      .map(layout => [layout.offset | 0, layout])
  );
  const u8MemoryLayouts = new Map(
    Object.values(game.memory || {})
      .filter(layout => (layout?.byte_backed === true || layout?.type_id === 5)
        && Number.isSafeInteger(layout.hash))
      .map(layout => [layout.hash | 0, layout])
  );
  const u8MemoryLayoutsByOffset = new Map(
    Object.values(game.memory || {})
      .filter(layout => (layout?.byte_backed === true || layout?.type_id === 5)
        && Number.isSafeInteger(layout.offset))
      .map(layout => [layout.offset | 0, layout])
  );
  const hasU8MemoryReference = reference => u8MemoryLayouts.has(reference | 0)
    || u8MemoryLayoutsByOffset.has(reference | 0);
  const resolveU8Memory = hash => {
    const layout = u8MemoryLayouts.get(hash | 0) || u8MemoryLayoutsByOffset.get(hash | 0);
    const memory = instance?.exports?.memory;
    if (!layout || !(memory instanceof WebAssembly.Memory)) return null;
    const { offset, stride, length } = layout;
    if (![offset, stride, length].every(Number.isSafeInteger)
      || offset < 0 || stride <= 0 || length < 0) return null;
    const span = length === 0 ? 0 : (length - 1) * stride + 1;
    const end = offset + span;
    if (!Number.isSafeInteger(span) || !Number.isSafeInteger(end)
      || end > memory.buffer.byteLength) return null;
    return { bytes: new Uint8Array(memory.buffer), offset, stride, length };
  };
  const readU8 = (memory, index) => {
    if (!memory || !Number.isInteger(index) || index < 0 || index >= memory.length) return 0;
    return memory.bytes[memory.offset + index * memory.stride];
  };
  const writeU8 = (memory, index, value) => {
    if (!memory || !Number.isInteger(index) || index < 0 || index >= memory.length) return;
    memory.bytes[memory.offset + index * memory.stride] = value;
  };
  const sysMemcpyU8 = (dst, dstIndex, src, srcIndex, count) => {
    if (!Number.isInteger(count) || count <= 0) return;
    const source = resolveU8Memory(src);
    const sourceRegistered = hasU8MemoryReference(src);
    const literal = sourceRegistered
      ? null
      : new TextEncoder().encode(stringValue(src));
    const values = new Uint8Array(count);
    for (let offset = 0; offset < count; offset += 1) {
      const index = srcIndex + offset;
      values[offset] = sourceRegistered
        ? readU8(source, index)
        : literal?.[index] ?? 0;
    }
    const destination = resolveU8Memory(dst);
    for (let offset = 0; offset < count; offset += 1) {
      writeU8(destination, dstIndex + offset, values[offset]);
    }
  };
  const typedMemoryLayouts = new Map([
    [1, { byHash: memoryLayoutsByHash(1), byOffset: memoryLayoutsByOffset(1), width: 4 }],
    [2, { byHash: memoryLayoutsByHash(2), byOffset: memoryLayoutsByOffset(2), width: 4 }],
  ]);
  const resolveTypedMemory = (reference, typeId) => {
    const metadata = typedMemoryLayouts.get(typeId);
    const layout = metadata?.byHash.get(reference | 0)
      || metadata?.byOffset.get(reference | 0);
    const memory = instance?.exports?.memory;
    if (!layout || !(memory instanceof WebAssembly.Memory)) return null;
    const { offset, stride, length } = layout;
    if (![offset, stride, length].every(Number.isSafeInteger)
      || offset < 0 || stride <= 0 || length < 0) return null;
    const span = length === 0 ? 0 : (length - 1) * stride + metadata.width;
    const end = offset + span;
    if (!Number.isSafeInteger(span) || !Number.isSafeInteger(end)
      || end > memory.buffer.byteLength) return null;
    return { view: new DataView(memory.buffer), offset, stride, length };
  };
  const readTyped = (memory, index, typeId) => {
    if (!memory || !Number.isInteger(index) || index < 0 || index >= memory.length) return 0;
    const offset = memory.offset + index * memory.stride;
    return typeId === 1
      ? memory.view.getInt32(offset, true)
      : memory.view.getFloat32(offset, true);
  };
  const writeTyped = (memory, index, value, typeId) => {
    if (!memory || !Number.isInteger(index) || index < 0 || index >= memory.length) return;
    const offset = memory.offset + index * memory.stride;
    if (typeId === 1) memory.view.setInt32(offset, value, true);
    else memory.view.setFloat32(offset, value, true);
  };
  const sysMemcpyTyped = (dst, dstIndex, src, srcIndex, count, typeId) => {
    if (!Number.isInteger(count) || count <= 0) return;
    const source = resolveTypedMemory(src, typeId);
    const values = typeId === 1 ? new Int32Array(count) : new Float32Array(count);
    for (let offset = 0; offset < count; offset += 1) {
      values[offset] = readTyped(source, srcIndex + offset, typeId);
    }
    const destination = resolveTypedMemory(dst, typeId);
    for (let offset = 0; offset < count; offset += 1) {
      writeTyped(destination, dstIndex + offset, values[offset], typeId);
    }
  };
  const sysMemcpyI32 = (dst, dstIndex, src, srcIndex, count) => {
    sysMemcpyTyped(dst, dstIndex, src, srcIndex, count, 1);
  };
  const sysMemcpyF32 = (dst, dstIndex, src, srcIndex, count) => {
    sysMemcpyTyped(dst, dstIndex, src, srcIndex, count, 2);
  };
  const setViewField = (base, index, field, value) => {
    const path = game.views?.[String(base)]?.[field];
    if (!path) return false;
    if (index < 0) {
      const global = instance?.exports?.[path];
      if (global instanceof WebAssembly.Global) {
        global.value = value;
        return true;
      }
      const metadata = game.globals?.[path];
      if (!metadata) return false;
      const setter = metadata.type_id === 2
        ? instance?.exports?.__stasis_global_set_f32
        : instance?.exports?.__stasis_global_set_i32;
      return typeof setter === "function" && setter(metadata.hash, value) !== 0;
    }
    const layout = game.memory?.[path];
    if (!layout || index >= layout.length || !instance?.exports.memory) return false;
    const offset = layout.offset + index * layout.stride;
    const view = new DataView(instance.exports.memory.buffer);
    if (layout.type_id === 2) view.setFloat32(offset, value, true);
    else if (layout.type_id === 4) view.setFloat64(offset, value, true);
    else if (layout.type_id === 5 || layout.type_id === 3) view.setUint8(offset, value);
    else if (layout.type_id === 6) view.setUint16(offset, value, true);
    else view.setInt32(offset, value, true);
    return true;
  };
  const loadSprite = pathId => {
    const handle = nextHandle++;
    const image = new Image();
    image.src = assetValue(pathId);
    sprites.set(handle, image);
    return handle;
  };
  const setCanvasFont = (font, size = font.renderSize) => {
    context.font = `${size}px ${font.family}`;
    context.textBaseline = "alphabetic";
    if ("fontKerning" in context) context.fontKerning = "none";
  };
  const measureTextRun = (font, text) => {
    context.save();
    setCanvasFont(font);
    const metrics = context.measureText(text);
    context.restore();
    const descent = Number.isFinite(metrics.actualBoundingBoxDescent)
      ? Math.max(0, metrics.actualBoundingBoxDescent)
      : Math.max(0, font.size - font.baseline);
    return { width: metrics.width, height: font.baseline + descent };
  };
  const refreshTextRun = run => {
    const font = fonts.get(run.font);
    if (!font?.ready) return;
    const metrics = measureTextRun(font, run.text);
    setViewField(run.base, run.index, "width", metrics.width);
    setViewField(run.base, run.index, "height", metrics.height);
  };
  const calibrateFont = font => {
    context.save();
    setCanvasFont(font, 1000);
    const metrics = context.measureText("Mg");
    context.restore();
    const ascent = Number.isFinite(metrics.fontBoundingBoxAscent)
      ? metrics.fontBoundingBoxAscent
      : 1000;
    const descent = Number.isFinite(metrics.fontBoundingBoxDescent)
      ? metrics.fontBoundingBoxDescent
      : 0;
    const nativeHeight = ascent + descent;
    const scale = nativeHeight > 0 ? font.size / nativeHeight : font.size / 1000;
    font.renderSize = 1000 * scale;
    font.baseline = ascent * scale;
    font.ready = true;
    font.pendingRuns.forEach(refreshTextRun);
    font.pendingRuns.length = 0;
  };
  const loadFont = (pathId, size) => {
    const handle = nextHandle++;
    const family = `stasis-font-${handle}`;
    const font = new FontFace(family, `url(${assetValue(pathId)})`);
    const fontInfo = {
      family, size, renderSize: size, baseline: size, ready: false, pendingRuns: []
    };
    fonts.set(handle, fontInfo);
    const load = Promise.resolve()
      .then(() => font.load())
      .then(loaded => {
        document.fonts.add(loaded);
        return loaded;
      });
    fontLoads.set(handle, load);
    return handle;
  };
  const measureText = (fontHandle, textId) => {
    const font = fonts.get(fontHandle);
    if (!font) return 0;
    context.save();
    setCanvasFont(font);
    const width = context.measureText(stringValue(textId)).width;
    context.restore();
    return width;
  };
  const releaseSprite = handle => { sprites.delete(handle); };
  const requestSprite = pathId => {
    const task = nextAssetTask++;
    const handle = nextHandle++;
    const image = new Image();
    const entry = { state: 1, handle, kind: "sprite" };
    assetTasks.set(task, entry);
    image.addEventListener("load", () => { if (entry.state < 3) entry.state = 3; });
    image.addEventListener("error", () => { if (entry.state < 3) entry.state = 4; });
    entry.state = 2;
    image.src = assetValue(pathId);
    sprites.set(handle, image);
    return task;
  };
  // @stasis-feature audio begin
  const ensureAudio = () => {
    audioContext ||= new AudioContext();
    return audioContext;
  };
  const scheduledAudioFrames = () => audioContext
    ? Math.max(0, Math.round((audioNextStart - audioContext.currentTime) * audioSampleRate))
    : 0;
  const queuedAudioFrames = () => scheduledAudioFrames() + pendingAudioFrames;
  const pendingAudioFrameLimit = () => Math.max(1, Math.round(audioSampleRate * PENDING_AUDIO_SECONDS));
  const queuePendingAudio = (start, frames = 0) => {
    if (pendingAudio.length >= PENDING_AUDIO_ENTRY_LIMIT) return false;
    pendingAudio.push({ start, frames });
    pendingAudioFrames += frames;
    return true;
  };
  const flushPendingAudio = () => {
    if (!audioContext || audioContext.state !== "running") return;
    const ready = pendingAudio.splice(0);
    pendingAudioFrames = 0;
    for (const entry of ready) void entry.start();
  };
  const loadAudio = pathId => {
    const handle = nextHandle++;
    const uri = assetValue(pathId);
    const decoded = fetch(uri)
      .then(response => response.arrayBuffer())
      .then(bytes => ensureAudio().decodeAudioData(bytes));
    audioAssets.set(handle, decoded);
    return handle;
  };
  const requestAudio = pathId => {
    const task = nextAssetTask++;
    const handle = nextHandle++;
    const entry = { state: 2, handle, kind: "audio" };
    assetTasks.set(task, entry);
    fetch(assetValue(pathId))
      .then(response => response.arrayBuffer())
      .then(bytes => ensureAudio().decodeAudioData(bytes))
      .then(decoded => {
        if (entry.state >= 3) return;
        audioAssets.set(handle, Promise.resolve(decoded));
        entry.state = 3;
      })
      .catch(() => { if (entry.state < 3) entry.state = 4; });
    return task;
  };
  const clampAudioVolume = volume => Math.max(0, Math.min(1, volume));
  const clampAudioPan = pan => Math.max(-1, Math.min(1, pan));
  const allocateAudioVoiceHandle = () => {
    if (audioVoices.size >= MAX_AUDIO_VOICES) return 0;
    for (let attempt = 0; attempt <= MAX_AUDIO_VOICES; attempt += 1) {
      const handle = nextAudioVoiceHandle;
      nextAudioVoiceHandle = handle >= MAX_AUDIO_VOICE_HANDLE ? 1 : handle + 1;
      if (!audioVoices.has(handle)) return handle;
    }
    return 0;
  };
  const allocateAudioVoiceGeneration = () => {
    const generation = nextAudioVoiceGeneration;
    nextAudioVoiceGeneration = generation >= Number.MAX_SAFE_INTEGER ? 1 : generation + 1;
    return generation;
  };
  const setPanValue = (voice, pan) => {
    voice.pan = clampAudioPan(pan);
    if (voice.panner) {
      voice.panner.pan.value = voice.pan;
    } else if (voice.leftGain && voice.rightGain) {
      const angle = (voice.pan + 1) * Math.PI * 0.25;
      voice.leftGain.gain.value = Math.cos(angle);
      voice.rightGain.gain.value = Math.sin(angle);
    }
  };
  const connectVoiceOutput = (audio, voice) => {
    const gain = audio.createGain();
    gain.gain.value = voice.volume;
    voice.gain = gain;
    if (typeof audio.createStereoPanner === "function") {
      voice.panner = audio.createStereoPanner();
      gain.connect(voice.panner).connect(audio.destination);
    } else if (typeof audio.createChannelMerger === "function") {
      voice.leftGain = audio.createGain();
      voice.rightGain = audio.createGain();
      voice.merger = audio.createChannelMerger(2);
      gain.connect(voice.leftGain).connect(voice.merger, 0, 0);
      gain.connect(voice.rightGain).connect(voice.merger, 0, 1);
      voice.merger.connect(audio.destination);
    } else {
      gain.connect(audio.destination);
    }
    setPanValue(voice, voice.pan);
  };
  const forgetAudioVoice = (handle, voice) => {
    if (audioVoices.get(handle) !== voice) return;
    audioVoices.delete(handle);
  };
  const startAudio = (assetHandle, loop, volume, pan = 0) => {
    if (!audioAssets.has(assetHandle)) return 0;
    const handle = allocateAudioVoiceHandle();
    if (!handle) return 0;
    const voice = {
      assetHandle,
      generation: allocateAudioVoiceGeneration(),
      source: undefined,
      gain: undefined,
      paused: false,
      volume: clampAudioVolume(volume),
      pan: clampAudioPan(pan),
    };
    audioVoices.set(handle, voice);
    const start = async () => {
      const generation = voice.generation;
      try {
        const audio = ensureAudio();
        const buffer = await audioAssets.get(assetHandle);
        if (!buffer || audioVoices.get(handle) !== voice || voice.generation !== generation) return false;
        const source = audio.createBufferSource();
        source.buffer = buffer;
        source.loop = Boolean(loop);
        voice.source = source;
        connectVoiceOutput(audio, voice);
        source.connect(voice.gain);
        if (voice.paused && source.playbackRate) source.playbackRate.value = 0;
        source.addEventListener("ended", () => {
          if (voice.source === source) forgetAudioVoice(handle, voice);
        });
        source.start();
        audioEvents += 1;
        document.body.dataset.audioEvents = String(audioEvents);
        return true;
      } catch (_) {
        forgetAudioVoice(handle, voice);
        return false;
      }
    };
    if (!audioContext || audioContext.state !== "running") {
      if (!queuePendingAudio(start)) {
        forgetAudioVoice(handle, voice);
        return 0;
      }
      return handle;
    }
    void start();
    return handle;
  };
  const stopAudio = handle => {
    const voice = audioVoices.get(handle);
    if (!voice) return;
    forgetAudioVoice(handle, voice);
    if (voice.source) voice.source.stop();
  };
  const setAudioVoicePaused = (handle, paused) => {
    const voice = audioVoices.get(handle);
    if (!voice || voice.paused === Boolean(paused)) return;
    voice.paused = Boolean(paused);
    if (!voice.source) return;
    if (voice.source.playbackRate) {
      voice.source.playbackRate.value = voice.paused ? 0 : 1;
    } else if (voice.paused) {
      voice.source.disconnect();
    } else {
      voice.source.connect(voice.gain);
    }
  };
  const setAudioVoiceVolumePan = (handle, volume, pan) => {
    const voice = audioVoices.get(handle);
    if (!voice) return;
    voice.volume = clampAudioVolume(volume);
    if (voice.gain) voice.gain.gain.value = voice.volume;
    setPanValue(voice, pan);
  };
  const stopAudioAsset = assetHandle => {
    for (const [handle, voice] of Array.from(audioVoices.entries())) {
      if (voice.assetHandle === assetHandle) stopAudio(handle);
    }
  };
  const setAudioAssetPaused = (assetHandle, paused) => {
    for (const [handle, voice] of audioVoices.entries()) {
      if (voice.assetHandle === assetHandle) setAudioVoicePaused(handle, paused);
    }
  };
  const setAudioAssetVolume = (assetHandle, volume) => {
    for (const [handle, voice] of audioVoices.entries()) {
      if (voice.assetHandle === assetHandle) setAudioVoiceVolumePan(handle, volume, voice.pan);
    }
  };
  const updateAudioState = () => {
    document.body.dataset.audioState = audioContext?.state || "closed";
  };
  const enableWebAudio = () => {
    const audio = ensureAudio();
    if (audio.state === "running") {
      flushPendingAudio();
      updateAudioState();
      return Promise.resolve(true);
    }
    if (audioEnablePromise) return audioEnablePromise;
    const attempt = audio.resume().then(() => {
      if (audio === audioContext && audio.state === "running") {
        flushPendingAudio();
        updateAudioState();
        return true;
      }
      updateAudioState();
      return false;
    }).catch(() => {
      updateAudioState();
      return false;
    }).finally(() => {
      if (audioEnablePromise === attempt) audioEnablePromise = undefined;
    });
    audioEnablePromise = attempt;
    return attempt;
  };
  const suspendWebAudio = () => {
    if (!audioContext) return;
    audioSuspendedByLifecycle = true;
    if (audioContext.state === "running") {
      void audioContext.suspend().then(updateAudioState).catch(() => {});
    }
  };
  const resumeWebAudio = () => {
    if (!audioSuspendedByLifecycle || !audioContext || audioContext.state === "closed") return;
    audioSuspendedByLifecycle = false;
    const resumingContext = audioContext;
    void resumingContext.resume().then(() => {
      if (audioSuspendedByLifecycle && resumingContext === audioContext) {
        void resumingContext.suspend().then(updateAudioState).catch(() => {});
      } else {
        flushPendingAudio();
        updateAudioState();
      }
    }).catch(() => {
      if (resumingContext === audioContext) audioSuspendedByLifecycle = true;
    });
  };
  const shutdownWebAudio = () => {
    audioSuspendedByLifecycle = false;
    pendingAudio.length = 0;
    pendingAudioFrames = 0;
    for (const handle of Array.from(audioVoices.keys())) stopAudio(handle);
    const closingContext = audioContext;
    audioContext = undefined;
    audioEnablePromise = undefined;
    audioNextStart = 0;
    if (closingContext && closingContext.state !== "closed") void closingContext.close();
    updateAudioState();
  };
  const pushAudio = (byteOffset, frameCount) => {
    if (!instance?.exports.memory || frameCount <= 0) return 0;
    const suspended = !audioContext || audioContext.state !== "running";
    const acceptedFrames = suspended
      ? Math.min(frameCount, Math.max(0, pendingAudioFrameLimit() - queuedAudioFrames()))
      : frameCount;
    if (acceptedFrames <= 0 || (suspended && pendingAudio.length >= PENDING_AUDIO_ENTRY_LIMIT)) return 0;
    const sampleCount = acceptedFrames * audioChannels;
    if (byteOffset < 0 || byteOffset + sampleCount * 4 > instance.exports.memory.buffer.byteLength) return 0;
    const samples = new Float32Array(instance.exports.memory.buffer, byteOffset, sampleCount).slice();
    const start = async () => {
      const audio = ensureAudio();
      const buffer = audio.createBuffer(audioChannels, acceptedFrames, audioSampleRate);
      for (let channel = 0; channel < audioChannels; channel += 1) {
        const output = buffer.getChannelData(channel);
        for (let frame = 0; frame < acceptedFrames; frame += 1) output[frame] = samples[frame * audioChannels + channel];
      }
      const source = audio.createBufferSource();
      source.buffer = buffer;
      source.connect(audio.destination);
      const earliest = audio.currentTime + 0.005;
      if (audioNextStart > 0 && audioNextStart < audio.currentTime) audioUnderruns += 1;
      const startAt = Math.max(earliest, audioNextStart);
      source.start(startAt);
      audioNextStart = startAt + acceptedFrames / audioSampleRate;
      audioEvents += 1;
      document.body.dataset.audioEvents = String(audioEvents);
      document.body.dataset.audioMode = "stream";
    };
    if (suspended) queuePendingAudio(start, acceptedFrames);
    else void start();
    return acceptedFrames;
  };
  // @stasis-feature audio end
  const cancelAssetTask = task => {
    const entry = assetTasks.get(task);
    if (!entry) return;
    entry.state = 5;
    if (entry.kind === "sprite") releaseSprite(entry.handle);
    else audioAssets.delete(entry.handle);
    assetTasks.delete(task);
  };
  const imports = { env: {
    sin_fast: value => Math.sin(value),
    cos_fast: value => Math.cos(value),
    print_i32: value => console.log(value),
    print_int: value => console.log(value),
    print_char: value => console.log(String.fromCodePoint(value)),
    print_string: value => console.log(stringValue(value)),
    sys_memcpy_u8: sysMemcpyU8,
    sys_memcpy_i32: sysMemcpyI32,
    sys_memcpy_f32: sysMemcpyF32,
    // @stasis-feature network begin
    stasis_web_network_supported: () => typeof WebSocket === "function" ? 1 : 0,
    stasis_web_network_connect: networkConnect,
    stasis_web_network_status: () => networkClient.state,
    stasis_web_network_poll: networkPoll,
    stasis_web_network_send: networkSend,
    stasis_web_network_checkpoint: networkCheckpoint,
    stasis_web_network_resume_seat: () => networkClient.desiredSeat,
    stasis_web_network_last_sequence: () => networkClient.lastSequence,
    // @stasis-feature network end
    web_input_axis: () => (keys.has("ArrowRight") || keys.has("KeyD") ? 1 : 0) - (keys.has("ArrowLeft") || keys.has("KeyA") ? 1 : 0),
    web_input_fire: () => keys.has("Space") || pointer.down ? 1 : 0,
    web_pointer_x: () => pointer.x | 0,
    web_pointer_down: () => pointer.down ? 1 : 0,
    web_begin_frame: (r, g, b) => { commands.length = 0; commands.push([0, r, g, b]); },
    web_draw_rect: (x, y, width, height, r, g, b) => commands.push([1, x, y, width, height, r, g, b]),
    web_draw_text: (x, y, value) => commands.push([2, x, y, value]),
    // @stasis-feature audio begin
    web_play_tone: (frequency, durationMs) => {
      if (!audioContext || audioContext.state !== "running") return;
      const oscillator = audioContext.createOscillator();
      const gain = audioContext.createGain();
      const duration = Math.max(20, Math.min(durationMs, 1000)) / 1000;
      oscillator.frequency.value = Math.max(40, Math.min(frequency, 4000));
      gain.gain.setValueAtTime(0.08, audioContext.currentTime);
      gain.gain.exponentialRampToValueAtTime(0.0001, audioContext.currentTime + duration);
      oscillator.connect(gain).connect(audioContext.destination);
      oscillator.start();
      oscillator.stop(audioContext.currentTime + duration);
      audioEvents += 1;
      document.body.dataset.audioEvents = String(audioEvents);
    },
    // @stasis-feature audio end
    gfx_load_sprite: pathId => loadSprite(pathId),
    stasis_gfx_load_sprite: pathId => loadSprite(pathId),
    gfx_release_sprite: handle => releaseSprite(handle),
    stasis_gfx_release_sprite: handle => releaseSprite(handle),
    stasis_jit_gfx_release_sprite: handle => releaseSprite(handle),
    stasis_jit_asset_request_sprite: pathId => requestSprite(pathId),
    // @stasis-feature audio begin
    stasis_jit_asset_request_audio: pathId => requestAudio(pathId),
    // @stasis-feature audio end
    stasis_jit_asset_task_poll: task => assetTasks.get(task)?.state || 0,
    stasis_jit_asset_task_take_handle: task => {
      const entry = assetTasks.get(task);
      if (!entry || entry.state !== 3) return 0;
      assetTasks.delete(task);
      return entry.handle;
    },
    stasis_jit_asset_task_cancel: task => cancelAssetTask(task),
    load_font: (pathId, size) => loadFont(pathId, size),
    stasis_load_font: (pathId, size) => loadFont(pathId, size),
    measure_text: (font, textId) => measureText(font, textId),
    stasis_measure_text: (font, textId) => measureText(font, textId),
    stasis_jit_measure_text: (font, textId) => measureText(font, textId),
    stasis_jit_sprite_load_from: (base, index, _len, pathId, width, height) => {
      const handle = loadSprite(pathId);
      return setViewField(base, index, "handle", handle)
        && setViewField(base, index, "width", width)
        && setViewField(base, index, "height", height) ? 1 : 0;
    },
    stasis_jit_gfx_cache_text: (font, textId) => {
      const handle = nextHandle++;
      cachedText.set(handle, { font, text: stringValue(textId) });
      return handle;
    },
    stasis_jit_text_run_load_from: (base, index, _len, font, textId) => {
      const handle = nextHandle++;
      const text = stringValue(textId);
      cachedText.set(handle, { font, text });
      const fontInfo = fonts.get(font) || { size: 16 };
      const run = { base, index, font, text };
      const loaded = setViewField(base, index, "font", font)
        && setViewField(base, index, "handle", handle)
        && setViewField(base, index, "width", text.length * fontInfo.size * 0.6)
        && setViewField(base, index, "height", fontInfo.size);
      if (loaded && fontInfo.ready) refreshTextRun(run);
      else if (loaded && fontInfo.pendingRuns) fontInfo.pendingRuns.push(run);
      return loaded ? 1 : 0;
    },
    storage_load_i32: (scope, key, fallback) => {
      const value = storageGet(storageKey(scope, key));
      if (value === null || !/^-?\d+$/.test(value)) return fallback;
      const parsed = Number(value);
      return Number.isSafeInteger(parsed) ? parsed | 0 : fallback;
    },
    storage_save_i32: (scope, key, value) => storageSet(storageKey(scope, key), String(value | 0)),
    stasis_jit_storage_load_ascii: (scope, key, out, capacity) => {
      const value = storageGet(storageKey(scope, key));
      return value === null ? -1 : writeAscii(out, capacity, value);
    },
    stasis_jit_storage_save_ascii: (scope, key, value, length) =>
      storageSet(storageKey(scope, key), readAscii(value, length)),
    stasis_jit_clipboard_load_ascii: (out, capacity) =>
      clipboardText ? writeAscii(out, capacity, clipboardText) : -1,
    stasis_jit_clipboard_save_ascii: (value, length) => {
      clipboardText = readAscii(value, length);
      if (navigator.clipboard?.writeText) void navigator.clipboard.writeText(clipboardText).catch(() => {});
      return 1;
    },
    // @stasis-feature audio begin
    audio_init: (sampleRate, channels) => {
      audioSampleRate = Math.max(8000, Math.min(sampleRate || 48000, 192000));
      audioChannels = Math.max(1, Math.min(channels || 2, 2));
      ensureAudio();
      return 1;
    },
    audio_shutdown: () => {
      shutdownWebAudio();
    },
    audio_is_available: () => 1,
    audio_get_sample_rate: () => audioSampleRate,
    audio_get_channels: () => audioChannels,
    audio_get_queued_frames: () => queuedAudioFrames(),
    audio_get_underruns: () => audioUnderruns,
    audio_push_f32_interleaved: (byteOffset, frameCount) => pushAudio(byteOffset, frameCount),
    audio_load_wav: pathId => loadAudio(pathId),
    audio_release: handle => { audioAssets.delete(handle); stopAudioAsset(handle); },
    audio_play: (handle, loop, volume, pan) => startAudio(handle, loop, volume, pan),
    audio_stop: handle => stopAudio(handle),
    audio_voice_is_playing: handle => audioVoices.has(handle) ? 1 : 0,
    audio_voice_set_paused: (handle, paused) => setAudioVoicePaused(handle, paused),
    audio_voice_set_volume_pan: (handle, volume, pan) => setAudioVoiceVolumePan(handle, volume, pan),
    stasis_jit_audio_load_music: pathId => loadAudio(pathId),
    stasis_jit_audio_load_effect: pathId => loadAudio(pathId),
    stasis_jit_audio_play_music: (handle, loop, volume) => {
      stopAudioAsset(handle);
      const voice = startAudio(handle, loop, volume, 0);
      return voice ? 1 : 0;
    },
    stasis_jit_audio_play_effect: (handle, volume) => startAudio(handle, false, volume, 0) ? 1 : 0,
    stasis_jit_audio_stop_music: handle => stopAudioAsset(handle),
    stasis_jit_audio_pause_music: (handle, paused) => setAudioAssetPaused(handle, paused),
    stasis_jit_audio_set_music_volume: (handle, volume) => setAudioAssetVolume(handle, volume),
    // @stasis-feature audio end
  }};

  document.addEventListener("paste", event => {
    clipboardText = event.clipboardData?.getData("text/plain") || clipboardText;
  });
  // @stasis-feature audio begin
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) suspendWebAudio();
    else resumeWebAudio();
  });
  addEventListener("pagehide", event => {
    if (event.persisted) suspendWebAudio();
    else shutdownWebAudio();
  });
  addEventListener("pageshow", event => {
    if (event.persisted && !document.hidden) resumeWebAudio();
  });
  // @stasis-feature audio end

  function getRectBatcher() {
    if (rectBatcher !== undefined) return rectBatcher;
    try {
      if (!document.createElement) return (rectBatcher = null);
      const target = document.createElement("canvas");
      const gl = target.getContext("webgl2", { alpha: true, premultipliedAlpha: true });
      if (!gl) return (rectBatcher = null);
      const vertex = gl.createShader(gl.VERTEX_SHADER);
      const fragment = gl.createShader(gl.FRAGMENT_SHADER);
      const program = gl.createProgram();
      if (!vertex || !fragment || !program) throw new Error("rectangle WebGL allocation failed");
      gl.shaderSource(vertex, `#version 300 es
        layout(location = 0) in vec2 p;
        layout(location = 1) in vec4 r;
        layout(location = 2) in vec4 c;
        uniform vec2 size;
        out vec4 color;
        void main() {
          vec2 q = r.xy + p * r.zw;
          gl_Position = vec4(q.x / size.x * 2.0 - 1.0,
            1.0 - q.y / size.y * 2.0, 0.0, 1.0);
          color = c;
        }`);
      gl.compileShader(vertex);
      if (!gl.getShaderParameter(vertex, gl.COMPILE_STATUS)) throw new Error("rectangle vertex shader failed");
      gl.shaderSource(fragment, `#version 300 es
        precision mediump float;
        in vec4 color;
        out vec4 outputColor;
        void main() { outputColor = color; }`);
      gl.compileShader(fragment);
      if (!gl.getShaderParameter(fragment, gl.COMPILE_STATUS)) throw new Error("rectangle fragment shader failed");
      gl.attachShader(program, vertex);
      gl.attachShader(program, fragment);
      gl.linkProgram(program);
      if (!gl.getProgramParameter(program, gl.LINK_STATUS)) throw new Error("rectangle program failed");
      const vao = gl.createVertexArray();
      const unitBuffer = gl.createBuffer();
      const instanceBuffer = gl.createBuffer();
      if (!vao || !unitBuffer || !instanceBuffer) throw new Error("rectangle buffers failed");
      gl.bindVertexArray(vao);
      gl.bindBuffer(gl.ARRAY_BUFFER, unitBuffer);
      gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([0, 0, 1, 0, 0, 1, 1, 1]), gl.STATIC_DRAW);
      gl.enableVertexAttribArray(0);
      gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
      gl.bindBuffer(gl.ARRAY_BUFFER, instanceBuffer);
      gl.bufferData(gl.ARRAY_BUFFER, rectScratch.byteLength, gl.DYNAMIC_DRAW);
      for (const [attribute, offset] of [[1, 0], [2, 16]]) {
        gl.enableVertexAttribArray(attribute);
        gl.vertexAttribPointer(attribute, 4, gl.FLOAT, false, 32, offset);
        gl.vertexAttribDivisor(attribute, 1);
      }
      gl.bindVertexArray(null);
      const size = gl.getUniformLocation(program, "size");
      return (rectBatcher = {
        draw(values, count) {
          const width = Math.max(1, canvas.width | 0);
          const height = Math.max(1, canvas.height | 0);
          if (target.width !== width || target.height !== height) {
            target.width = width;
            target.height = height;
          }
          gl.viewport(0, 0, width, height);
          gl.clearColor(0, 0, 0, 0);
          gl.clear(gl.COLOR_BUFFER_BIT);
          gl.useProgram(program);
          gl.uniform2f(size, width, height);
          gl.bindVertexArray(vao);
          gl.bindBuffer(gl.ARRAY_BUFFER, instanceBuffer);
          gl.bufferSubData(gl.ARRAY_BUFFER, 0, values, 0, count * 8);
          gl.enable(gl.BLEND);
          gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
          gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, count);
          gl.bindVertexArray(null);
          context.globalAlpha = 1;
          context.globalCompositeOperation = "source-over";
          context.drawImage(target, 0, 0);
        }
      });
    } catch (_) {
      return (rectBatcher = null);
    }
  }
  function executeCommands() {
    performanceWorkload.commands += commands.length;
    context.globalAlpha = 1;
    context.globalCompositeOperation = "source-over";
    for (const command of commands) {
      if (command[0] === 0) {
        context.fillStyle = color(command[1], command[2], command[3]);
        context.fillRect(0, 0, canvas.width, canvas.height);
      } else if (command[0] === 1) {
        context.globalAlpha = 1;
        context.fillStyle = color(command[5], command[6], command[7]);
        context.fillRect(command[1], command[2], command[3], command[4]);
      } else if (command[0] === 2) {
        context.fillStyle = "#dff6ff";
        context.font = "18px ui-monospace, Consolas, monospace";
        context.fillText(`score ${command[3]}`, command[1], command[2]);
      }
    }
    executeStasisBuffer();
  }

  function executeStasisBuffer() {
    const iLayout = game.memory.gfx_cmd_i32;
    const fLayout = game.memory.gfx_cmd_f32;
    if (!iLayout || !fLayout || !instance.exports.memory) return;
    const i32 = new Int32Array(instance.exports.memory.buffer, iLayout.offset, iLayout.length);
    const f32 = new Float32Array(instance.exports.memory.buffer, fLayout.offset, fLayout.length);
    if (i32[0] !== 1196967473) return;
    const version = i32[1];
    if (version < 2 || version > 5) return;
    const spriteStride = version >= 5 ? 8 : 4;
    const textBase = version >= 5 ? 112772 : 96388;
    const flags = i32[2];
    if (flags & 1) {
      context.save();
      context.globalAlpha = Math.max(0, Math.min(1, f32[3]));
      context.fillStyle = `rgb(${Math.round(f32[0] * 255)} ${Math.round(f32[1] * 255)} ${Math.round(f32[2] * 255)})`;
      context.fillRect(0, 0, canvas.width, canvas.height);
      context.restore();
    }
    const drawLine = index => {
      performanceWorkload.lines += 1;
      performanceWorkload.drawCalls += 1;
      const base = 4 + index * 8;
      context.save();
      context.globalAlpha = f32[base + 7];
      context.strokeStyle = `rgb(${Math.round(f32[base + 4] * 255)} ${Math.round(f32[base + 5] * 255)} ${Math.round(f32[base + 6] * 255)})`;
      context.beginPath();
      context.moveTo(f32[base], f32[base + 1]);
      context.lineTo(f32[base + 2], f32[base + 3]);
      context.stroke();
      context.restore();
    };
    const drawRect = index => {
      const base = 79996 - index * 8;
      context.globalAlpha = f32[base + 7];
      context.fillStyle = unitColor(f32[base + 4], f32[base + 5], f32[base + 6]);
      context.fillRect(f32[base], f32[base + 1], f32[base + 2], f32[base + 3]);
    };
    const drawRectRun = (start, count, ordered) => {
      performanceWorkload.rectangles += count;
      if (count < RECT_BATCH_MIN) {
        performanceWorkload.drawCalls += count;
        for (let offset = 0; offset < count; offset += 1) {
          const index = ordered ? i32[18464 + start + offset] % 16384 : start + offset;
          drawRect(index);
        }
        return;
      }
      const batcher = getRectBatcher();
      if (!batcher) {
        performanceWorkload.drawCalls += count;
        for (let offset = 0; offset < count; offset += 1) {
          const index = ordered ? i32[18464 + start + offset] % 16384 : start + offset;
          drawRect(index);
        }
        return;
      }
      for (let offset = 0; offset < count; offset += 1) {
        const index = ordered ? i32[18464 + start + offset] % 16384 : start + offset;
        const source = 79996 - index * 8;
        const target = offset * 8;
        for (let field = 0; field < 8; field += 1) rectScratch[target + field] = f32[source + field];
      }
      try {
        batcher.draw(rectScratch, count);
        performanceWorkload.instances += count;
        performanceWorkload.batches += 1;
        performanceWorkload.drawCalls += 1;
        performanceWorkload.uploadedBytes += count * 8 * Float32Array.BYTES_PER_ELEMENT;
        performanceBackend = "Canvas2D + WebGL2";
      } catch (_) {
        rectBatcher = null;
        performanceWorkload.drawCalls += count;
        for (let offset = 0; offset < count; offset += 1) {
          const index = ordered ? i32[18464 + start + offset] % 16384 : start + offset;
          drawRect(index);
        }
      }
    };
    const drawSprite = index => {
      performanceWorkload.sprites += 1;
      performanceWorkload.drawCalls += 1;
      const baseI = 32 + index * 3;
      const baseF = 80004 + index * spriteStride;
      const image = sprites.get(i32[baseI]);
      if (!image || !image.complete || !image.naturalWidth) return;
      const x = f32[baseF];
      const y = f32[baseF + 1];
      const width = f32[baseF + 2];
      const height = f32[baseF + 3];
      const u0 = version >= 5 ? f32[baseF + 4] : 0;
      const v0 = version >= 5 ? f32[baseF + 5] : 0;
      const u1 = version >= 5 ? f32[baseF + 6] : 1;
      const v1 = version >= 5 ? f32[baseF + 7] : 1;
      if (u0 < 0 || v0 < 0 || u1 > 1 || v1 > 1 || u0 >= u1 || v0 >= v1) return;
      const sourceX = u0 * image.naturalWidth;
      const sourceY = v0 * image.naturalHeight;
      const sourceWidth = (u1 - u0) * image.naturalWidth;
      const sourceHeight = (v1 - v0) * image.naturalHeight;
      context.save();
      context.globalAlpha = Math.max(0, Math.min(1, i32[baseI + 2] / 255));
      context.translate(x + width / 2, y + height / 2);
      context.rotate(i32[baseI + 1] * Math.PI / 180);
      if (u0 === 0 && v0 === 0 && u1 === 1 && v1 === 1) {
        context.drawImage(image, -width / 2, -height / 2, width, height);
      } else {
        context.drawImage(image, sourceX, sourceY, sourceWidth, sourceHeight, -width / 2, -height / 2, width, height);
      }
      context.restore();
    };
    const drawText = index => {
      performanceWorkload.text += 1;
      performanceWorkload.drawCalls += 1;
      const baseI = 12320 + index * 3;
      const baseF = textBase + index * 6;
      const offset = i32[baseI + 1];
      const cached = offset < 0 ? cachedText.get(-offset) : null;
      const fontHandle = cached ? cached.font : i32[baseI];
      const font = fonts.get(fontHandle) || {
        family: "ui-monospace", size: 18, renderSize: 18, baseline: 18
      };
      let text = cached ? cached.text : "";
      if (!cached && game.memory.gfx_cmd_u8) {
        const bytesLayout = game.memory.gfx_cmd_u8;
        const bytes = new Uint8Array(instance.exports.memory.buffer, bytesLayout.offset + offset, i32[baseI + 2]);
        text = new TextDecoder().decode(bytes);
      }
      context.save();
      context.globalAlpha = f32[baseF + 5];
      context.fillStyle = `rgb(${Math.round(f32[baseF + 2] * 255)} ${Math.round(f32[baseF + 3] * 255)} ${Math.round(f32[baseF + 4] * 255)})`;
      setCanvasFont(font);
      context.fillText(text, f32[baseF], f32[baseF + 1] + font.baseline);
      context.restore();
    };
    const lineCount = Math.max(0, Math.min(i32[3], 10000));
    const spriteCount = Math.max(0, Math.min(i32[4], 4096));
    const textCount = Math.max(0, Math.min(i32[7], 2048));
    const rectCount = version >= 4 ? Math.max(0, Math.min(i32[24], 10000 - lineCount)) : 0;
    const orderCount = version >= 3 ? Math.max(0, Math.min(i32[22], 16144)) : 0;
    performanceWorkload.commands += lineCount + rectCount + spriteCount + textCount;
    if (orderCount > 0) {
      for (let order = 0; order < orderCount; order += 1) {
        const encoded = i32[18464 + order];
        const kind = Math.floor(encoded / 16384);
        const index = encoded % 16384;
        if (kind === 1 && index < lineCount) drawLine(index);
        else if (kind === 2 && index < spriteCount) drawSprite(index);
        else if (kind === 3 && index < textCount) drawText(index);
        else if (kind === 4 && index < rectCount) {
          let runCount = 1;
          while (order + runCount < orderCount) {
            const next = i32[18464 + order + runCount];
            if (Math.floor(next / 16384) !== 4 || next % 16384 >= rectCount) break;
            runCount += 1;
          }
          drawRectRun(order, runCount, true);
          order += runCount - 1;
        }
      }
    } else {
      for (let index = 0; index < lineCount; index += 1) drawLine(index);
      drawRectRun(0, rectCount, false);
      for (let index = 0; index < spriteCount; index += 1) drawSprite(index);
      for (let index = 0; index < textCount; index += 1) drawText(index);
    }
  }

  function sdlScancode(code) {
    if (/^Key[A-Z]$/.test(code)) return code.charCodeAt(3) - 65 + 4;
    if (/^Digit[1-9]$/.test(code)) return Number(code[5]) + 29;
    const values = {
      Digit0: 39, Enter: 40, Escape: 41, Backspace: 42, Tab: 43, Space: 44,
      Minus: 45, Equal: 46, BracketLeft: 47, BracketRight: 48, Backslash: 49,
      Semicolon: 51, Quote: 52, Backquote: 53, Comma: 54, Period: 55, Slash: 56,
      CapsLock: 57, PrintScreen: 70, ScrollLock: 71, Pause: 72, Insert: 73,
      Home: 74, PageUp: 75, Delete: 76, End: 77, PageDown: 78,
      ArrowRight: 79, ArrowLeft: 80, ArrowDown: 81, ArrowUp: 82,
      NumLock: 83, NumpadDivide: 84, NumpadMultiply: 85, NumpadSubtract: 86,
      NumpadAdd: 87, NumpadEnter: 88, NumpadDecimal: 99, IntlBackslash: 100,
      ContextMenu: 101, ControlLeft: 224, ShiftLeft: 225, AltLeft: 226,
      MetaLeft: 227, ControlRight: 228, ShiftRight: 229, AltRight: 230, MetaRight: 231
    };
    if (/^F(?:[1-9]|1[0-2])$/.test(code)) return Number(code.slice(1)) + 57;
    if (/^Numpad[1-9]$/.test(code)) return Number(code[6]) + 88;
    if (code === "Numpad0") return 98;
    return values[code];
  }

  function writeHostFrame(timestamp) {
    const iLayout = game.memory.host_i32;
    const fLayout = game.memory.host_f32;
    if (!iLayout || !fLayout || !instance.exports.memory) return;
    const i32 = new Int32Array(instance.exports.memory.buffer, iLayout.offset, iLayout.length);
    const f32 = new Float32Array(instance.exports.memory.buffer, fLayout.offset, fLayout.length);
    i32.fill(0);
    f32.fill(0);
    const elapsedMs = timestamp - startedAt;
    const bounds = canvas.getBoundingClientRect();
    const ratio = devicePixelRatio || 1;
    const focused = document.hasFocus() ? 1 : 0;
    const pointerCount = pointer.hover || pointer.down || pointer.wentDown || pointer.wentUp ? 1 : 0;
    i32[0] = Math.floor(elapsedMs) | 0;
    i32[7] = pointerCount;
    i32[8] = 0;
    i32[9] = 0;
    i32[10] = tickIndex++;
    i32[11] = resized ? 1 : 0;
    i32[12] = Math.round(screen.width * ratio);
    i32[13] = Math.round(screen.height * ratio);
    i32[14] = 3;
    i32[15] = (focused ? 2 : 0) | (document.hidden ? 4 : 0) | (resized ? 8 : 0);
    i32[16] = 0;
    i32[17] = focused;
    i32[18] = document.hidden ? 1 : 0;
    i32[19] = Math.floor(elapsedMs * 1000) | 0;
    i32[22] = Math.round(screen.width * ratio);
    i32[23] = Math.round(screen.height * ratio);
    i32[24] = Math.round(bounds.width * ratio);
    i32[25] = Math.round(bounds.height * ratio);
    i32[30] = displayGeneration;
    i32[31] = densityGeneration;
    for (const code of keys) {
      const scancode = sdlScancode(code);
      if (scancode !== undefined && scancode < 512) i32[32 + scancode] = 1;
    }
    if (pointerCount) {
      i32[544] = pointer.id;
      i32[545] = pointer.down ? 1 : 0;
      i32[546] = pointer.wentDown ? 1 : 0;
      i32[547] = pointer.wentUp ? 1 : 0;
      f32[0] = pointer.x;
      f32[1] = pointer.y;
      f32[2] = pointer.dx;
      f32[3] = pointer.dy;
      f32[4] = canvas.width ? pointer.x / canvas.width : 0;
      f32[5] = canvas.height ? pointer.y / canvas.height : 0;
    }
    f32[48] = ratio;
    f32[49] = ratio;
    f32[50] = canvas.width;
    f32[51] = canvas.height;
    f32[52] = 0;
    f32[53] = 0;
    f32[54] = canvas.width;
    f32[55] = canvas.height;
    document.body.dataset.hostTick = String(i32[10]);
    document.body.dataset.hostTimeMs = String(i32[0]);
    if (resized) document.body.dataset.resizeTick = String(i32[10]);
    resized = false;
  }

  function finishHostFrame() {
    pointer.wentDown = false;
    pointer.wentUp = false;
    pointer.dx = 0;
    pointer.dy = 0;
  }

  function exportedI32(name) {
    const value = instance?.exports[name];
    if (value && typeof value.value === "number") return value.value;
    const metadata = game.globals?.[name];
    const getter = instance?.exports?.__stasis_global_get_i32;
    return metadata && typeof getter === "function" ? getter(metadata.hash) : undefined;
  }

  function setCanvasSize(width, height) {
    width = Math.max(1, Math.min(width | 0, 8192));
    height = Math.max(1, Math.min(height | 0, 8192));
    if (canvas.width === width && canvas.height === height) return;
    canvas.width = width;
    canvas.height = height;
    canvas.style.aspectRatio = `${width} / ${height}`;
    canvas.parentElement.style.width = `min(100vw, calc(100vh * ${width} / ${height}))`;
    resized = true;
    displayGeneration += 1;
  }

  function applyWindowRequest() {
    const sequence = exportedI32("host_req_seq");
    if (sequence === undefined || sequence === lastWindowRequest) return;
    lastWindowRequest = sequence;
    const flags = exportedI32("host_req_flags") || 0;
    const width = exportedI32("host_req_window_w_px") || canvas.width;
    const height = exportedI32("host_req_window_h_px") || canvas.height;
    if (flags & 1) {
      canvas.style.width = "";
      canvas.style.height = "";
      setCanvasSize(width, height);
      pendingFullscreen = false;
      document.body.dataset.windowMode = "windowed";
    } else if (flags & 2) {
      pendingFullscreen = true;
      document.body.dataset.windowMode = "fullscreen-pending";
    } else if (flags & 4) {
      setCanvasSize(width, height);
      document.body.dataset.windowMode = "maximized";
      resized = true;
    }
    document.body.dataset.windowRequestSeq = String(sequence);
  }

  async function applyFullscreenGesture() {
    if (pendingFullscreen === undefined) return;
    try {
      if (pendingFullscreen && !document.fullscreenElement) await canvas.requestFullscreen();
      if (!pendingFullscreen && document.fullscreenElement) await document.exitFullscreen();
      document.body.dataset.windowMode = pendingFullscreen ? "fullscreen" : "windowed";
      pendingFullscreen = undefined;
    } catch (error) {
      document.body.dataset.fullscreenError = String(error);
    }
  }

  function frame(timestamp) {
    applyWindowRequest();
    writeHostFrame(timestamp);
    const tickStart = performance.now();
    instance.exports.tick();
    const tickMs = performance.now() - tickStart;
    const wasmRenderStart = performance.now();
    instance.exports.render();
    const wasmRenderMs = performance.now() - wasmRenderStart;
    performanceWorkload.commands = 0;
    performanceWorkload.lines = 0;
    performanceWorkload.rectangles = 0;
    performanceWorkload.sprites = 0;
    performanceWorkload.text = 0;
    performanceWorkload.instances = 0;
    performanceWorkload.batches = 0;
    performanceWorkload.drawCalls = 0;
    performanceWorkload.uploadedBytes = 0;
    performanceBackend = rectBatcher ? "Canvas2D + WebGL2" : "Canvas2D";
    const replayStart = performance.now();
    executeCommands();
    const browserReplayMs = performance.now() - replayStart;
    const renderMs = wasmRenderMs + browserReplayMs;
    const frameWorkMs = tickMs + renderMs;
    const renderPrepMs = -1;
    const gpuSubmitMs = -1;
    const gpuExecutionMs = -1;
    const presentWaitMs = -1;
    frames += 1;
    if (hud) {
      recordPerformanceWorst(timestamp, tickMs, renderMs, wasmRenderMs, browserReplayMs, frameWorkMs);
    }
    const underBudget = frameWorkMs <= 16.67;
    if (hud) {
      const uploadText = performanceWorkload.uploadedBytes > 0 ? ` · uploaded ${performanceWorkload.uploadedBytes} B` : "";
      const instanceText = performanceBackend.includes("WebGL2")
        ? ` · instances ${performanceWorkload.instances} · batches ${performanceWorkload.batches}` : "";
      hud.textContent = `${performanceBackend} · frame ${frames}\ntick ${tickMs.toFixed(3)} ms (worst ${worstTick.toFixed(3)}) · guest render ${wasmRenderMs.toFixed(3)} ms (worst ${worstWasmRender.toFixed(3)})\nhost replay ${browserReplayMs.toFixed(3)} ms (worst ${worstBrowserReplay.toFixed(3)})\nframe work ${frameWorkMs.toFixed(3)} ms (worst ${worstFrameWork.toFixed(3)}) · ${underBudget ? "UNDER 16.67 ms" : "OVER 16.67 ms"}\ncommands ${performanceWorkload.commands} · lines ${performanceWorkload.lines} · rects ${performanceWorkload.rectangles} · sprites ${performanceWorkload.sprites} · text ${performanceWorkload.text}\ndraws ${performanceWorkload.drawCalls}${instanceText}${uploadText}`;
    }
    document.body.dataset.frames = String(frames);
    document.body.dataset.tickMs = tickMs.toFixed(3);
    document.body.dataset.renderMs = renderMs.toFixed(3);
    document.body.dataset.wasmRenderMs = wasmRenderMs.toFixed(3);
    document.body.dataset.browserReplayMs = browserReplayMs.toFixed(3);
    document.body.dataset.frameWorkMs = frameWorkMs.toFixed(3);
    document.body.dataset.backend = performanceBackend;
    document.body.dataset.hostReplayMs = browserReplayMs.toFixed(3);
    document.body.dataset.renderPrepMs = String(renderPrepMs);
    document.body.dataset.gpuSubmitMs = String(gpuSubmitMs);
    document.body.dataset.gpuExecutionMs = String(gpuExecutionMs);
    document.body.dataset.presentWaitMs = String(presentWaitMs);
    document.body.dataset.commands = String(performanceWorkload.commands);
    document.body.dataset.lines = String(performanceWorkload.lines);
    document.body.dataset.rectangles = String(performanceWorkload.rectangles);
    document.body.dataset.sprites = String(performanceWorkload.sprites);
    document.body.dataset.text = String(performanceWorkload.text);
    document.body.dataset.instances = performanceBackend.includes("WebGL2") ? String(performanceWorkload.instances) : "-1";
    document.body.dataset.batches = performanceBackend.includes("WebGL2") ? String(performanceWorkload.batches) : "-1";
    document.body.dataset.drawCalls = String(performanceWorkload.drawCalls);
    document.body.dataset.uploadedBytes = String(performanceWorkload.uploadedBytes);
    document.body.dataset.worstTickMs = worstTick.toFixed(3);
    document.body.dataset.worstRenderMs = worstRender.toFixed(3);
    document.body.dataset.worstWasmRenderMs = worstWasmRender.toFixed(3);
    document.body.dataset.worstBrowserReplayMs = worstBrowserReplay.toFixed(3);
    document.body.dataset.worstFrameWorkMs = worstFrameWork.toFixed(3);
    document.body.dataset.underBudget = String(underBudget);
    if (instance.exports.player_x) document.body.dataset.playerX = String(instance.exports.player_x.value);
    finishHostFrame();
    requestAnimationFrame(frame);
  }

  function updatePointer(event) {
    const bounds = canvas.getBoundingClientRect();
    const x = Math.round((event.clientX - bounds.left) * canvas.width / bounds.width);
    const y = Math.round((event.clientY - bounds.top) * canvas.height / bounds.height);
    const inside = event.clientX >= bounds.left && event.clientX <= bounds.right
      && event.clientY >= bounds.top && event.clientY <= bounds.bottom;
    pointer.dx += x - pointer.x;
    pointer.dy += y - pointer.y;
    pointer.x = x;
    pointer.y = y;
    pointer.id = event.pointerId | 0;
    pointer.hover = event.pointerType !== "touch" && inside;
  }
  addEventListener("keydown", event => {
    keys.add(event.code);
    // @stasis-feature audio begin
    void enableWebAudio();
    // @stasis-feature audio end
    void applyFullscreenGesture();
    if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Space"].includes(event.code)) event.preventDefault();
  });
  addEventListener("keyup", event => { keys.delete(event.code); void applyFullscreenGesture(); });
  canvas.addEventListener("pointermove", updatePointer);
  // @stasis-feature audio begin
  addEventListener("pointerdown", () => { void enableWebAudio(); }, { passive: true });
  // @stasis-feature audio end
  canvas.addEventListener("pointerdown", event => {
    updatePointer(event);
    pointer.down = true;
    pointer.wentDown = true;
    canvas.setPointerCapture(event.pointerId);
    canvas.focus();
    void applyFullscreenGesture();
  });
  canvas.addEventListener("pointerleave", () => { pointer.hover = false; });
  canvas.addEventListener("pointerup", event => {
    updatePointer(event);
    pointer.down = false;
    pointer.wentUp = true;
  });
  canvas.addEventListener("pointercancel", () => { pointer.hover = false; pointer.down = false; pointer.wentUp = true; });
  addEventListener("resize", () => { resized = true; displayGeneration += 1; });
  document.addEventListener("fullscreenchange", () => { resized = true; displayGeneration += 1; });
  // @stasis-feature audio begin
  void enableWebAudio();
  // @stasis-feature audio end

  async function wasmBytes() {
    const response = await fetch("game.wasm");
    if (!response.ok) throw new Error(`failed to load game.wasm: ${response.status}`);
    return response.arrayBuffer();
  }

  window.STASIS_RUNTIME_PROMISE = (async () => {
    try {
      setLoading("Preparing…", "loading");
      const result = await WebAssembly.instantiate(await wasmBytes(), imports);
      instance = result.instance;
      writeHostFrame(performance.now());
      const mainResult = instance.exports.main();
      finishHostFrame();
      applyWindowRequest();
      await Promise.all([
        ...Array.from(sprites.values(), image => image.decode().catch(() => undefined)),
        ...fontLoads.values()
      ]);
      await document.fonts.ready;
      setLoading("", "ready");
      fonts.forEach(calibrateFont);
      document.body.dataset.ready = "true";
      document.body.dataset.runtime = "wasm";
      document.body.dataset.mainResult = String(mainResult);
      requestAnimationFrame(frame);
    } catch (error) {
      document.body.dataset.ready = "false";
      setLoading(`Unable to start this game. ${String(error && error.message || error)}`, "failed");
      if (instance) {
        for (const [label, name] of [
          ["debugPhaseCount", "state.active_run_definition.phase_count"],
          ["debugSimulationTicks", "state.game.simulation_ticks"],
          ["debugRunEnabled", "state.active_run_definition.enabled"]
        ]) {
          const value = instance.exports[name];
          if (value instanceof WebAssembly.Global) document.body.dataset[label] = String(value.value);
        }
      }
      errorBox.textContent = String(error && error.stack || error);
      throw error;
    }
  })();
})();
