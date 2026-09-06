(() => {
  "use strict";
  const canvas = document.getElementById("stasis-canvas");
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
  let externalActionGeneration = 0;
  let pendingExternalActionGeneration = 0;
  const commands = [];
  const game = window.STASIS_GAME || { strings: {}, memory: {}, assets: {} };
  const sprites = new Map();
  const fonts = new Map();
  const fontLoads = new Map();
  const cachedText = new Map();
  const immutableTextHandles = new Map();
  const TEXT_RUN_MAX_ENTRIES = 4096;
  const TEXT_RUN_MAX_BYTES = 262144;
  const DYNAMIC_TEXT_MAX_BYTES = 4096;
  let cachedTextBytes = 0;
  const preparedText = new Map();
  const PREPARED_TEXT_MAX_ENTRIES = 256;
  const PREPARED_TEXT_MAX_BYTES = 8 * 1024 * 1024;
  let preparedTextBytes = 0;
  const DISPLAY_MAX_DPR = 4;
  const DISPLAY_MIN_DPR = 0.5;
  const DISPLAY_MAX_BACKING_WIDTH = 8192;
  const DISPLAY_MAX_BACKING_HEIGHT = 8192;
  const DISPLAY_MAX_LOGICAL_DIMENSION = 8192;
  const DISPLAY_MAX_BACKING_BYTES = 64 * 1024 * 1024;
  const DISPLAY_MAX_RASTER_BYTES = 64 * 1024 * 1024;
  const DISPLAY_DENSITY_TIERS = Object.freeze([1, 1.25, 1.5, 2, 3, 4, 6, 8]);
  const RASTER_OPTIONS = "contain-center-high-smoothing-v1";
  const SPRITE_CACHE_MAX_BYTES = 64 * 1024 * 1024;
  const initialBackingDimension = (value, fallback) => {
    const number = Number(value);
    return Number.isFinite(number) && number >= 1 ? Math.round(number) : fallback;
  };
  const initialLogicalDimension = (name, backing) => {
    const attribute = `data-logical-${name.toLowerCase()}`;
    const value = Number(canvas.dataset?.[`logical${name}`] ?? canvas.getAttribute?.(attribute));
    return Number.isInteger(value) && value >= 1 && value <= DISPLAY_MAX_LOGICAL_DIMENSION
      ? value
      : Math.min(DISPLAY_MAX_LOGICAL_DIMENSION, backing);
  };
  const initialBackingWidth = initialBackingDimension(canvas.width, 640);
  const initialBackingHeight = initialBackingDimension(canvas.height, 360);
  const initialLogicalWidth = initialLogicalDimension("Width", initialBackingWidth);
  const initialLogicalHeight = initialLogicalDimension("Height", initialBackingHeight);
  const display = {
    logicalWidth: initialLogicalWidth,
    logicalHeight: initialLogicalHeight,
    availableWidth: 1,
    availableHeight: 1,
    cssWidth: 0,
    cssHeight: 0,
    backingWidth: initialBackingWidth,
    backingHeight: initialBackingHeight,
    rawDpr: 1,
    effectiveDpr: 1,
    scaleX: 1,
    scaleY: 1,
    contentScale: 1,
    rasterScale: 1,
    densityTier: 1,
    displayGeneration: 1,
    densityGeneration: 1,
    backingBytes: 0,
    fallback: "none",
    densityKey: "1",
    rasterScaleKey: "1",
  };
  let resizeGenerationPending = true;
  let logicalExtentPending = false;
  let onDensityChange = () => {};
  let spriteTierCache = new Map();
  let spriteCacheHits = 0;
  let spriteRasterCount = 0;
  let spriteDecodedCount = 0;
  let spriteStaleCount = 0;
  const spriteDecodedSources = new Set();
  let spriteCacheBytes = 0;
  let latestAssetResource;
  const MAX_SPRITE_CACHE_ENTRIES = 128;
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
    instances: 0, batches: 0, drawCalls: 0, composites: 0,
    renderSubmissions: 0, uploadedBytes: 0,
    textureBinds: 0, atlasTransitions: 0, pipelineBoundaries: 0,
    atlasPages: -1, atlasLiveEntries: -1, atlasAllocatedBytes: -1,
    atlasUploadCount: -1, atlasUploadBytes: -1
  };
  let performanceBackend = "WebGL2";
  let gpuBatcher;
  let webglLifecycleInstalled = false;
  let loseWebGlRenderer = () => {};
  // Keep the production gfx_cmd decoder values named and mechanically checked
  // against runtime/stasis_render_contract.h by the ABI gate.
  const GFX_CMD_MAGIC = 0x47584631;
  const GFX_CMD_VERSION = 7;
  const GFX_FLAG_CLEAR = 1;
  const GFX_FLAG_PRESENT = 2;
  const GFX_I_MAGIC = 0;
  const GFX_I_VERSION = 1;
  const GFX_I_FLAGS = 2;
  const GFX_I_LINE_COUNT = 3;
  const GFX_I_SPRITE_COUNT = 4;
  const GFX_I_TEXT_COUNT = 7;
  const GFX_I_TEXT_BYTES_USED = 9;
  const GFX_I_ORDER_COUNT = 22;
  const GFX_I_RECT_COUNT = 24;
  const GFX_I_CLIP_COUNT = 27;
  const GFX_I_SPRITE_RUN_COUNT = 29;
  const GFX_I_SPRITE_BASE = 32;
  const GFX_I_TEXT_BASE = 12320;
  const GFX_I_SPRITE_RUN_BASE = 18464;
  const GFX_I_ORDER_BASE = 51232;
  const GFX_F_CLEAR_BASE = 0;
  const GFX_F_LINE_BASE = 4;
  const GFX_F_SPRITE_BASE = 80004;
  const GFX_F_RECT_REVERSE_BASE = 79996;
  const GFX_F_TEXT_BASE = 133252;
  const GFX_F_CLIP_BASE = 145540;
  const GFX_MAX_GEOMETRY = 10000;
  const GFX_GEOMETRY_STRIDE_F32 = 8;
  const GFX_MAX_LINES = GFX_MAX_GEOMETRY;
  const GFX_LINE_STRIDE_F32 = GFX_GEOMETRY_STRIDE_F32;
  const GFX_MAX_SPRITES = 4096;
  const GFX_SPRITE_STRIDE_I32 = 3;
  const GFX_SPRITE_STRIDE_F32 = 13;
  const GFX_MAX_SPRITE_RUNS = 4096;
  const GFX_SPRITE_RUN_STRIDE_I32 = 8;
  const GFX_MAX_TEXT = 2048;
  const GFX_TEXT_STRIDE_I32 = 3;
  const GFX_TEXT_STRIDE_F32 = 6;
  const GFX_TEXT_MAX_BYTES = 65536;
  const GFX_MAX_CLIPS = 256;
  const GFX_CLIP_STRIDE_F32 = 4;
  const GFX_MAX_ORDER = GFX_MAX_LINES + GFX_MAX_SPRITES + GFX_MAX_TEXT + GFX_MAX_CLIPS * 2;
  const GFX_ORDER_KIND_SCALE = 16384;
  const GFX_ORDER_LINE = 1;
  const GFX_ORDER_SPRITE = 2;
  const GFX_ORDER_TEXT = 3;
  const GFX_ORDER_RECT = 4;
  const GFX_ORDER_CLIP_PUSH = 5;
  const GFX_ORDER_CLIP_POP = 6;
  const SPRITE_CAP = GFX_MAX_SPRITES;
  const spriteScratch = new Float32Array(SPRITE_CAP * 16);
  const ATLAS_PAGE_SIZE = 512;
  const ATLAS_PAGE_MAX = 2048;
  const ATLAS_MAX_PAGES = 8;
  // A dedicated 4096² page plus the ordinary shared page must coexist for an
  // oversize resource without selecting another renderer.
  const ATLAS_MAX_BYTES = 128 * 1024 * 1024;
  const ATLAS_PADDING = 2;
  const startedAt = performance.now();

  let preparationCanvas;
  let preparationContext;
  let missingSpriteResource;
  const resourcePreparationContext = () => {
    preparationCanvas ||= document.createElement?.("canvas");
    preparationContext ||= preparationCanvas?.getContext?.("2d", { alpha: true });
    if (!preparationCanvas || !preparationContext) throw new Error("Canvas2D resource preparation unavailable");
    return { canvas: preparationCanvas, context: preparationContext };
  };
  const deterministicMissingSprite = () => {
    if (missingSpriteResource) return missingSpriteResource;
    const surface = document.createElement?.("canvas");
    const preparation = surface?.getContext?.("2d", { alpha: true });
    if (!surface || !preparation) throw new Error("Canvas2D placeholder resource preparation unavailable");
    surface.width = 2;
    surface.height = 2;
    preparation.fillStyle = "#ff00ff";
    preparation.fillRect(0, 0, 2, 2);
    preparation.fillStyle = "#111111";
    preparation.fillRect(1, 0, 1, 1);
    preparation.fillRect(0, 1, 1, 1);
    missingSpriteResource = {
      ready: true, drawable: surface, width: 2, height: 2, generation: 1,
      missing: true
    };
    return missingSpriteResource;
  };
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
  const assetMetadata = id => {
    const key = assetKey(stringValue(id));
    return game.asset_metadata?.[key] || game.assetMetadata?.[key] || null;
  };
  const finitePositive = (value, fallback = 0) =>
    Number.isFinite(Number(value)) && Number(value) > 0 ? Number(value) : fallback;
  const boundedInteger = (value, minimum, maximum, fallback, floorValue = false) => {
    const number = Number(value);
    return Number.isFinite(number)
      ? Math.max(minimum, Math.min(maximum, floorValue ? Math.floor(number) : Math.round(number)))
      : fallback;
  };
  const displayNumber = value => Number.isFinite(value) ? String(Number(value.toFixed(6))) : "0";
  const setCanvasMetadata = (name, value) => {
    const text = String(value);
    if (canvas.dataset && canvas.dataset[name] !== text) canvas.dataset[name] = text;
    else if (!canvas.dataset && typeof canvas.setAttribute === "function") {
      const attribute = `data-${name.replace(/[A-Z]/g, letter => `-${letter.toLowerCase()}`)}`;
      if (canvas.getAttribute?.(attribute) !== text) canvas.setAttribute(attribute, text);
    }
  };
  const publishDisplayReceipt = () => {
    if (!document.body?.dataset) return;
    const data = document.body.dataset;
    data.logicalWidth = displayNumber(display.logicalWidth);
    data.logicalHeight = displayNumber(display.logicalHeight);
    data.availableWidth = displayNumber(display.availableWidth);
    data.availableHeight = displayNumber(display.availableHeight);
    data.cssWidth = displayNumber(display.cssWidth);
    data.cssHeight = displayNumber(display.cssHeight);
    data.backingWidth = String(display.backingWidth);
    data.backingHeight = String(display.backingHeight);
    data.backingBytes = String(display.backingBytes);
    data.devicePixelRatio = displayNumber(display.rawDpr);
    data.effectiveDpr = displayNumber(display.effectiveDpr);
    data.contentScale = displayNumber(display.contentScale);
    data.rasterScale = displayNumber(display.rasterScale);
    data.densityTier = displayNumber(display.densityTier);
    data.displayGeneration = String(display.displayGeneration);
    data.densityGeneration = String(display.densityGeneration);
    data.backingFallback = display.fallback;
    data.backingCap = display.fallback === "none" ? "none" : "capped";
    data.spriteCacheHits = String(spriteCacheHits);
    data.spriteRasterCount = String(spriteRasterCount);
    data.spriteDecodedCount = String(spriteDecodedCount);
    data.spriteStaleCount = String(spriteStaleCount);
    data.assetCacheBytes = String(spriteCacheBytes);
    data.assetCacheFallback = spriteCacheBytes > SPRITE_CACHE_MAX_BYTES ? "memory-cap" : "none";
    if (latestAssetResource) {
      const resource = latestAssetResource;
      const metadata = resource.metadata || {};
      data.assetSource = resource.source;
      data.assetSourceIdentity = resource.sourceIdentity || "";
      data.assetSourceWidth = String(resource.sourceWidth || metadata.source_width || 0);
      data.assetSourceHeight = String(resource.sourceHeight || metadata.source_height || 0);
      data.assetSourceBytes = String(resource.sourceBytes || metadata.source_bytes || 0);
      data.assetPreparedWidth = String(resource.width || 0);
      data.assetPreparedHeight = String(resource.height || 0);
      data.assetPreparedBytes = String((resource.width || 0) * (resource.height || 0) * 4);
      data.assetPreparedFileBytes = String(metadata.prepared_bytes || 0);
      data.assetPreparedTier = displayNumber(resource.tier || 0);
      data.assetLogicalWidth = String(resource.logicalWidth || 0);
      data.assetLogicalHeight = String(resource.logicalHeight || 0);
      data.assetTierKey = resource.tierKey || "";
      data.assetFallback = resource.fallback || "none";
      data.assetDecodedWidth = String(resource.decodedWidth || 0);
      data.assetDecodedHeight = String(resource.decodedHeight || 0);
      data.assetDecodedBytes = String(resource.decodedBytes || 0);
      data.assetGeneration = String(resource.generation || 0);
      data.assetReady = resource.ready ? "true" : "false";
      data.assetRefreshState = resource.refreshing
        ? "pending" : resource.refreshError ? "failed" : "none";
      data.assetRefresh = data.assetRefreshState;
      data.assetRefreshError = resource.refreshError
        ? String(resource.refreshError?.message || resource.refreshError) : "";
      data.assetRefreshFallback = resource.refreshFallback || "none";
    }
    const atlas = gpuBatcher?.metrics?.();
    data.assetAtlasWidth = String(atlas?.width || 0);
    data.assetAtlasHeight = String(atlas?.height || 0);
    data.assetAtlasBytes = String(atlas?.allocatedBytes || 0);
    data.assetAtlasGeneration = String(atlas?.generation || 0);
    data.assetAtlasFallback = document.body.dataset.atlasFallback || "none";
  };
  const displayCssExtent = () => {
    const bounds = canvas.getBoundingClientRect?.() || {};
    const width = finitePositive(bounds.width)
      || finitePositive(canvas.clientWidth)
      || display.cssWidth
      || display.logicalWidth;
    const height = finitePositive(bounds.height)
      || finitePositive(canvas.clientHeight)
      || display.cssHeight
      || display.logicalHeight;
    return { width, height };
  };
  const availablePresentationExtent = () => {
    const published = window.STASIS_AVAILABLE_VIEWPORT;
    const visual = window.visualViewport;
    const styles = typeof getComputedStyle === "function" ? getComputedStyle(document.body) : null;
    const inset = side => {
      const value = styles && parseFloat(styles.getPropertyValue(`padding-${side}`));
      return Number.isFinite(value) && value > 0 ? value : 0;
    };
    const bounded = value => Math.max(1, Math.min(0x7fffffff, finitePositive(value, 1)));
    const viewportWidth = finitePositive(published?.width)
      || finitePositive(visual?.width)
      || finitePositive(document.documentElement?.clientWidth)
      || finitePositive(window.innerWidth)
      || finitePositive(globalThis.screen?.width) / finitePositive(globalThis.devicePixelRatio, 1);
    const viewportHeight = finitePositive(published?.height)
      || finitePositive(visual?.height)
      || finitePositive(document.documentElement?.clientHeight)
      || finitePositive(window.innerHeight)
      || finitePositive(globalThis.screen?.height) / finitePositive(globalThis.devicePixelRatio, 1);
    if (finitePositive(published?.width) && finitePositive(published?.height)) {
      return { width: bounded(published.width), height: bounded(published.height) };
    }
    return {
      width: bounded(viewportWidth - inset("left") - inset("right")),
      height: bounded(viewportHeight - inset("top") - inset("bottom"))
    };
  };
  const densityTierFor = scale =>
    DISPLAY_DENSITY_TIERS.find(tier => tier + 1e-9 >= scale) || DISPLAY_DENSITY_TIERS.at(-1);
  const requestDisplaySync = () => {
    resized = true;
  };
  const syncDisplayState = () => {
    // Older test hosts without a dataset had no separate logical metadata.
    // Preserve their explicit intrinsic resize behavior while real browsers
    // always use the data-logical-* contract above the physical backing.
    if (!canvas.dataset && !logicalExtentPending
      && (canvas.width !== display.backingWidth || canvas.height !== display.backingHeight)) {
      display.logicalWidth = boundedInteger(canvas.width, 1, DISPLAY_MAX_BACKING_WIDTH, display.logicalWidth);
      display.logicalHeight = boundedInteger(canvas.height, 1, DISPLAY_MAX_BACKING_HEIGHT, display.logicalHeight);
      logicalExtentPending = true;
    }
    const extent = displayCssExtent();
    const available = availablePresentationExtent();
    const requestedDpr = finitePositive(globalThis.devicePixelRatio, 1);
    // Keep the browser's requested DPR observable for host metrics and
    // receipts. Backing allocation has its own bounded DPR so an unusually
    // dense display cannot bypass the physical-size and byte caps below.
    const backingDpr = Math.max(DISPLAY_MIN_DPR, Math.min(DISPLAY_MAX_DPR, requestedDpr));
    const requestedWidth = Math.max(1, Math.round(extent.width * backingDpr));
    const requestedHeight = Math.max(1, Math.round(extent.height * backingDpr));
    const requestedBytes = requestedWidth * requestedHeight * 4;
    const dimensionScale = Math.min(
      1,
      DISPLAY_MAX_BACKING_WIDTH / requestedWidth,
      DISPLAY_MAX_BACKING_HEIGHT / requestedHeight
    );
    const byteScale = requestedBytes > DISPLAY_MAX_BACKING_BYTES
      ? Math.sqrt(DISPLAY_MAX_BACKING_BYTES / requestedBytes)
      : 1;
    const capScale = Math.min(1, dimensionScale, byteScale);
    const effectiveDpr = backingDpr * capScale;
    let backingWidth = boundedInteger(
      extent.width * effectiveDpr, 1, DISPLAY_MAX_BACKING_WIDTH, 1
    );
    let backingHeight = boundedInteger(
      extent.height * effectiveDpr, 1, DISPLAY_MAX_BACKING_HEIGHT, 1
    );
    while (backingWidth * backingHeight * 4 > DISPLAY_MAX_BACKING_BYTES
      && (backingWidth > 1 || backingHeight > 1)) {
      const correction = Math.sqrt(DISPLAY_MAX_BACKING_BYTES / (backingWidth * backingHeight * 4));
      const nextWidth = Math.max(1, Math.floor(backingWidth * correction));
      const nextHeight = Math.max(1, Math.floor(backingHeight * correction));
      if (nextWidth === backingWidth && nextHeight === backingHeight) {
        if (backingWidth >= backingHeight) backingWidth -= 1;
        else backingHeight -= 1;
      } else {
        backingWidth = nextWidth;
        backingHeight = nextHeight;
      }
    }
    const scaleX = backingWidth / Math.max(1, display.logicalWidth);
    const scaleY = backingHeight / Math.max(1, display.logicalHeight);
    const contentScale = Math.min(scaleX, scaleY);
    const rasterScale = Math.max(1, Math.min(8, contentScale));
    const densityTier = densityTierFor(rasterScale);
    const fallback = [
      requestedDpr !== backingDpr ? "dpr" : "",
      dimensionScale < 1 ? "dimension" : "",
      byteScale < 1 ? "bytes" : ""
    ].filter(Boolean).join(",") || "none";
    const extentChanged = logicalExtentPending
      || display.cssWidth !== extent.width
      || display.cssHeight !== extent.height
      || display.availableWidth !== available.width
      || display.availableHeight !== available.height
      || display.backingWidth !== backingWidth
      || display.backingHeight !== backingHeight
      || display.logicalWidth <= 0 || display.logicalHeight <= 0;
    const densityTierChanged = display.densityKey !== String(densityTier);
    const rasterScaleKey = String(rasterScale);
    const rasterScaleChanged = display.rasterScaleKey !== rasterScaleKey;
    display.cssWidth = extent.width;
    display.cssHeight = extent.height;
    display.availableWidth = available.width;
    display.availableHeight = available.height;
    display.rawDpr = requestedDpr;
    display.effectiveDpr = effectiveDpr;
    display.backingWidth = backingWidth;
    display.backingHeight = backingHeight;
    display.scaleX = scaleX;
    display.scaleY = scaleY;
    display.contentScale = contentScale;
    display.rasterScale = rasterScale;
    display.densityTier = densityTier;
    display.backingBytes = backingWidth * backingHeight * 4;
    display.fallback = fallback || "none";
    if (extentChanged && !resizeGenerationPending) {
      display.displayGeneration += 1;
      resizeGenerationPending = true;
    } else if (extentChanged) {
      resized = true;
    }
    if (rasterScaleChanged) {
      display.rasterScaleKey = rasterScaleKey;
      display.densityGeneration += 1;
      resized = true;
    }
    if (densityTierChanged) {
      display.densityKey = String(densityTier);
      onDensityChange();
    }
    if (canvas.width !== backingWidth) canvas.width = backingWidth;
    if (canvas.height !== backingHeight) canvas.height = backingHeight;
    logicalExtentPending = false;
    setCanvasMetadata("logicalWidth", display.logicalWidth);
    setCanvasMetadata("logicalHeight", display.logicalHeight);
    publishDisplayReceipt();
    return display;
  };
  const setLogicalCanvas = (width, height) => {
    const nextWidth = boundedInteger(width, 1, DISPLAY_MAX_BACKING_WIDTH, display.logicalWidth);
    const nextHeight = boundedInteger(height, 1, DISPLAY_MAX_BACKING_HEIGHT, display.logicalHeight);
    if (display.logicalWidth === nextWidth && display.logicalHeight === nextHeight) {
      syncDisplayState();
      return;
    }
    display.logicalWidth = nextWidth;
    display.logicalHeight = nextHeight;
    logicalExtentPending = true;
    setCanvasMetadata("logicalWidth", nextWidth);
    setCanvasMetadata("logicalHeight", nextHeight);
    window.STASIS_REFIT_VIEWPORT?.();
    syncDisplayState();
  };
  const displaySnapshot = () => ({
    logicalWidth: display.logicalWidth,
    logicalHeight: display.logicalHeight,
    availableWidth: display.availableWidth,
    availableHeight: display.availableHeight,
    cssWidth: display.cssWidth,
    cssHeight: display.cssHeight,
    backingWidth: display.backingWidth,
    backingHeight: display.backingHeight,
    rawDpr: display.rawDpr,
    effectiveDpr: display.effectiveDpr,
    scaleX: display.scaleX,
    scaleY: display.scaleY,
    contentScale: display.contentScale,
    rasterScale: display.rasterScale,
    densityTier: display.densityTier,
    displayGeneration: display.displayGeneration,
    densityGeneration: display.densityGeneration,
    backingBytes: display.backingBytes,
    fallback: display.fallback,
  });
  const hostDesktopDimension = (screenExtent, cssExtent) => {
    const fallback = finitePositive(cssExtent, 1);
    const extent = finitePositive(screenExtent);
    const pixels = extent > 0 ? extent * display.rawDpr : fallback;
    return boundedInteger(pixels, 1, 0x7fffffff, Math.round(fallback));
  };
  setCanvasMetadata("logicalWidth", display.logicalWidth);
  setCanvasMetadata("logicalHeight", display.logicalHeight);
  window.STASIS_DISPLAY_RECEIPT = displaySnapshot;
  if (globalThis.STASIS_CHARACTERIZATION_TEST === true) {
    window.__STASIS_DISPLAY__ = displaySnapshot;
  }
  const storageKey = (scopeId, keyId) => `stasis:${stringValue(scopeId)}:${stringValue(keyId)}`;
  const storageGet = key => {
    try { return localStorage.getItem(key); } catch (_) { return volatileStorage.get(key) ?? null; }
  };
  const storageSet = (key, value) => {
    try { localStorage.setItem(key, value); } catch (_) { volatileStorage.set(key, value); }
    return 1;
  };
  // Keep the browser runtime as the source of truth for characterization.  The
  // hook is opt-in and only exists in deterministic test VMs; published pages
  // do not expose internal storage or network state.
  if (globalThis.STASIS_CHARACTERIZATION_TEST === true) {
    window.__STASIS_CHARACTERIZATION__ = {
      storageKey,
      storageGet,
      storageSet,
      networkCheckpoint,
      networkCheckpointKey,
      networkLoadCheckpoint,
      networkConnect,
      networkPoll,
      networkSend,
      networkClient,
    };
  }
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
    Object.entries(game.memory || {})
      .filter(([, layout]) => (layout?.byte_backed === true || layout?.type_id === 5)
        && Number.isSafeInteger(layout.hash))
      .map(([path, layout]) => [layout.hash | 0, { ...layout, path }])
  );
  const u8MemoryLayoutsByOffset = new Map(
    Object.entries(game.memory || {})
      .filter(([, layout]) => (layout?.byte_backed === true || layout?.type_id === 5)
        && Number.isSafeInteger(layout.offset))
      .map(([path, layout]) => [layout.offset | 0, { ...layout, path }])
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
    return { bytes: new Uint8Array(memory.buffer), offset, stride, length, path: layout.path };
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
  const canWriteViewField = (base, index, field) => {
    const path = game.views?.[String(base)]?.[field];
    if (!path) return false;
    if (index < 0) {
      const global = instance?.exports?.[path];
      if (global instanceof WebAssembly.Global) return true;
      const metadata = game.globals?.[path];
      const setter = metadata?.type_id === 2
        ? instance?.exports?.__stasis_global_set_f32
        : instance?.exports?.__stasis_global_set_i32;
      return Boolean(metadata) && typeof setter === "function";
    }
    const layout = game.memory?.[path];
    return Boolean(layout && index < layout.length && instance?.exports.memory);
  };
  const canWriteTextRun = (base, index) => ["font", "handle", "width", "height"]
    .every(field => canWriteViewField(base, index, field));
  const runtimeCollectionLength = memory => {
    const path = memory?.path;
    if (!path) return null;
    const lengthPath = `${path}.length`;
    const exported = instance?.exports?.[lengthPath];
    let length;
    if (exported instanceof WebAssembly.Global) {
      length = Number(exported.value);
    } else {
      const metadata = game.globals?.[lengthPath];
      const getter = instance?.exports?.__stasis_global_get_i32;
      if (!metadata || typeof getter !== "function") return null;
      length = Number(getter(metadata.hash));
    }
    return Number.isInteger(length) && length >= 0 && length <= memory.length ? length : null;
  };
  const runtimeTextValue = (reference, maxBytes = Number.POSITIVE_INFINITY) => {
    const memory = resolveU8Memory(reference);
    if (memory) {
      const length = runtimeCollectionLength(memory);
      if (length === null || length > maxBytes) return null;
      const bytes = Array.from({ length }, (_, index) => readU8(memory, index));
      try {
        return { text: new TextDecoder("utf-8", { fatal: true }).decode(new Uint8Array(bytes)), bytes: bytes.length };
      } catch {
        return null;
      }
    }
    if (Object.prototype.hasOwnProperty.call(game.strings || {}, String(reference))) {
      const text = String(game.strings[String(reference)]);
      if (text.length > maxBytes) return null;
      const bytes = new TextEncoder().encode(text).length;
      return bytes <= maxBytes ? { text, bytes } : null;
    }
    return null;
  };
  const EXTERNAL_URL_MAX_BYTES = 2048;
  const publishExternalUrlResult = result => {
    if (document.body?.dataset) document.body.dataset.externalUrlResult = result;
  };
  const markExternalActionGesture = () => {
    externalActionGeneration += 1;
    pendingExternalActionGeneration = externalActionGeneration;
  };
  const clearExternalActionGesture = () => { pendingExternalActionGeneration = 0; };
  const validExternalUrlPort = value => /^\d{1,5}$/.test(value)
    && Number(value) >= 1 && Number(value) <= 65535;
  const validExternalUrlDnsHost = value => {
    if (value.length < 1 || value.length > 253 || value.startsWith(".") || value.endsWith(".")) return false;
    const labels = value.split(".");
    if (labels.some(label => label.length < 1 || label.length > 63
        || label.startsWith("-") || label.endsWith("-") || !/^[a-zA-Z0-9-]+$/.test(label))) return false;
    if (!labels.every(label => /^\d+$/.test(label))) return true;
    return labels.length === 4 && labels.every(label => label.length <= 3
      && (label.length === 1 || !label.startsWith("0")) && Number(label) <= 255);
  };
  const validExternalUrlAuthority = value => {
    if (value.length < 1 || value.includes("@") || /[^\x00-\x7f]/.test(value)) return false;
    if (value.startsWith("[")) {
      const close = value.indexOf("]");
      if (close <= 1 || !/^[0-9a-fA-F:]+$/.test(value.slice(1, close))) return false;
      return close === value.length - 1
        || (value[close + 1] === ":" && validExternalUrlPort(value.slice(close + 2)));
    }
    const colon = value.indexOf(":");
    if (colon !== value.lastIndexOf(":")) return false;
    const host = colon < 0 ? value : value.slice(0, colon);
    return validExternalUrlDnsHost(host)
      && (colon < 0 || validExternalUrlPort(value.slice(colon + 1)));
  };
  const validatedExternalUrl = reference => {
    const value = runtimeTextValue(reference, EXTERNAL_URL_MAX_BYTES);
    if (!value || value.bytes < 1 || value.bytes > EXTERNAL_URL_MAX_BYTES
        || /[\u0000-\u0020\u007f-\u009f]/u.test(value.text)
        || value.text.includes("\\") || /%(?![0-9a-fA-F]{2})/.test(value.text)
        || !/^(?:http|https):\/\//.test(value.text)) return null;
    try {
      const bytes = new TextEncoder().encode(value.text);
      if (bytes.length !== value.bytes
          || new TextDecoder("utf-8", { fatal: true }).decode(bytes) !== value.text) return null;
      const authority = value.text.slice(value.text.indexOf("//") + 2).split(/[/?#]/, 1)[0];
      if (!validExternalUrlAuthority(authority)) return null;
      const parsed = new URL(value.text);
      if ((parsed.protocol !== "http:" && parsed.protocol !== "https:")
          || parsed.username !== "" || parsed.password !== "" || parsed.hostname === "") return null;
      return value.text;
    } catch (_) {
      return null;
    }
  };
  const openExternalUrl = reference => {
    const url = validatedExternalUrl(reference);
    if (url === null) { publishExternalUrlResult("invalid"); return -1; }
    if (pendingExternalActionGeneration === 0) {
      publishExternalUrlResult("ignored");
      return 0;
    }
    clearExternalActionGesture();
    const userActivation = globalThis.navigator?.userActivation;
    if (globalThis.STASIS_HEADLESS === true || globalThis.STASIS_RECORDING === true
        || game.headless === true || game.recording === true
        || userActivation?.isActive !== true || typeof window.open !== "function") {
      publishExternalUrlResult("unavailable");
      return 0;
    }
    let opened;
    try {
      opened = window.open("about:blank", "_blank");
      if (!opened) { publishExternalUrlResult("blocked"); return 0; }
      opened.opener = null;
      const link = opened.document?.createElement?.("a");
      if (!link) {
        opened.close?.();
        publishExternalUrlResult("blocked");
        return 0;
      }
      link.href = url;
      link.target = "_self";
      link.rel = "noopener noreferrer";
      link.referrerPolicy = "no-referrer";
      opened.document.body?.append?.(link);
      link.click();
      link.remove?.();
      publishExternalUrlResult("opened");
      return 1;
    } catch (_) {
      opened?.close?.();
      publishExternalUrlResult("blocked");
      return 0;
    }
  };
  if (globalThis.STASIS_CHARACTERIZATION_TEST === true) {
    Object.assign(window.__STASIS_CHARACTERIZATION__, {
      openExternalUrl,
      validatedExternalUrl,
      markExternalActionGesture,
      clearExternalActionGesture,
    });
  }
  const getViewField = (base, index, field) => {
    const path = game.views?.[String(base)]?.[field];
    if (!path) return 0;
    if (index < 0) {
      const global = instance?.exports?.[path];
      if (global instanceof WebAssembly.Global) return Number(global.value);
      const metadata = game.globals?.[path];
      if (!metadata) return 0;
      const getter = metadata.type_id === 2
        ? instance?.exports?.__stasis_global_get_f32
        : instance?.exports?.__stasis_global_get_i32;
      return typeof getter === "function" ? Number(getter(metadata.hash)) : 0;
    }
    const layout = game.memory?.[path];
    if (!layout || index >= layout.length || !instance?.exports.memory) return 0;
    const offset = layout.offset + index * layout.stride;
    const view = new DataView(instance.exports.memory.buffer);
    if (layout.type_id === 2) return view.getFloat32(offset, true);
    if (layout.type_id === 4) return view.getFloat64(offset, true);
    if (layout.type_id === 5 || layout.type_id === 3) return view.getUint8(offset);
    if (layout.type_id === 6) return view.getUint16(offset, true);
    return view.getInt32(offset, true);
  };
  const spriteDimensions = (width, height) => {
    const valid = value => Number.isFinite(value) && value > 0 && value <= DISPLAY_MAX_BACKING_WIDTH;
    if (!valid(width) || !valid(height)) return null;
    return { width: Math.max(1, Math.round(width)), height: Math.max(1, Math.round(height)) };
  };
  const spriteEncoding = request => String(request.metadata?.encoding || "").toLowerCase();
  const spriteMetadataDimensions = metadata => {
    const width = finitePositive(
      metadata?.prepared_width ?? metadata?.source_width ?? metadata?.width
    );
    const height = finitePositive(
      metadata?.prepared_height ?? metadata?.source_height ?? metadata?.height
    );
    return width && height ? {
      width: Math.max(1, Math.round(width)), height: Math.max(1, Math.round(height))
    } : null;
  };
  const spriteSourceIdentity = value => [
    String(value.source ?? ""),
    value.metadata?.source_sha256 || "",
    value.metadata?.prepared_sha256 || "",
  ].join("|");
  const spriteTierKey = request => [
    request.sourceIdentity,
    request.logicalWidth || 0,
    request.logicalHeight || 0,
    request.width || 0,
    request.height || 0,
    request.tier || 0,
    request.fallback || "none",
    RASTER_OPTIONS,
  ].join(":");
  const publishAssetReceipt = resource => {
    if (!document.body?.dataset || !resource) return;
    latestAssetResource = resource;
    publishDisplayReceipt();
  };
  const spriteRasterRequest = resource => {
    const metadata = resource.metadata || {};
    const metadataDimensions = spriteMetadataDimensions(metadata);
    const logical = resource.requested || (
      finitePositive(metadata.logical_width) && finitePositive(metadata.logical_height)
        ? {
            width: Number(metadata.logical_width), height: Number(metadata.logical_height)
          }
        : metadataDimensions
    );
    const tier = densityTierFor(display.rasterScale);
    const requestedWidth = logical ? logical.width * tier : metadataDimensions?.width || 0;
    const requestedHeight = logical ? logical.height * tier : metadataDimensions?.height || 0;
    const uncappedWidth = finitePositive(requestedWidth, metadataDimensions?.width || 1);
    const uncappedHeight = finitePositive(requestedHeight, metadataDimensions?.height || 1);
    const dimensionScale = Math.min(
      1,
      DISPLAY_MAX_BACKING_WIDTH / uncappedWidth,
      DISPLAY_MAX_BACKING_HEIGHT / uncappedHeight
    );
    const dimensionWidth = uncappedWidth * dimensionScale;
    const dimensionHeight = uncappedHeight * dimensionScale;
    const dimensionBytes = dimensionWidth * dimensionHeight * 4;
    const byteScale = dimensionBytes > DISPLAY_MAX_RASTER_BYTES
      ? Math.sqrt(DISPLAY_MAX_RASTER_BYTES / dimensionBytes)
      : 1;
    const capped = dimensionScale < 1 || byteScale < 1;
    const finalScale = dimensionScale * byteScale;
    const quantize = value => capped ? Math.floor(value) : Math.ceil(value);
    const width = boundedInteger(
      quantize(uncappedWidth * finalScale), 1, DISPLAY_MAX_BACKING_WIDTH,
      metadataDimensions?.width || 1,
      capped
    );
    const height = boundedInteger(
      quantize(uncappedHeight * finalScale), 1, DISPLAY_MAX_BACKING_HEIGHT,
      metadataDimensions?.height || 1,
      capped
    );
    const fallback = byteScale < 1
      ? "raster-bytes"
      : dimensionScale < 1 ? "raster-dimension" : "none";
    const request = {
      source: String(resource.source ?? ""),
      metadata: Object.freeze({ ...metadata }),
      sourceIdentity: spriteSourceIdentity(resource),
      sourceDimensions: metadataDimensions,
      sourceBytes: finitePositive(metadata.source_bytes),
      encoding: spriteEncoding({ metadata }),
      logicalWidth: logical?.width ? Math.max(1, Math.round(logical.width)) : 0,
      logicalHeight: logical?.height ? Math.max(1, Math.round(logical.height)) : 0,
      width, height, tier, fallback,
      hasDimensions: Boolean(logical || metadataDimensions),
      loadImage: () => ensureSpriteImage(resource),
    };
    request.key = spriteTierKey(request);
    return Object.freeze(request);
  };
  const resultForSprite = (request, drawable, width, height, fallback,
    sourceWidth, sourceHeight, decodedWidth = sourceWidth, decodedHeight = sourceHeight,
    sourceDrawable = null, sourceDrawableWidth = 0, sourceDrawableHeight = 0,
    sourceDrawableOwned = false) => ({
    drawable, width, height, fallback: fallback || "none",
    sourceDrawable, sourceDrawableWidth, sourceDrawableHeight,
    sourceDrawableOwned,
    sourceWidth, sourceHeight,
    sourceBytes: request.sourceBytes || 0,
    decodedWidth: decodedWidth || 0,
    decodedHeight: decodedHeight || 0,
    decodedBytes: (decodedWidth || 0) * (decodedHeight || 0) * 4,
  });
  const spriteVariantFor = (resource, partial) =>
    partial && resource.sourceDrawable && resource.sourceDrawableWidth && resource.sourceDrawableHeight
      ? {
          key: "source", drawable: resource.sourceDrawable,
          width: resource.sourceDrawableWidth, height: resource.sourceDrawableHeight
        }
      : { key: "full", drawable: resource.drawable, width: resource.width, height: resource.height };
  const rasterSprite = async request => {
    const bitmapFactory = globalThis.createImageBitmap;
    const sourceDimensions = request.sourceDimensions;
    const targetWidth = Math.max(1, Math.min(DISPLAY_MAX_BACKING_WIDTH, request.width || sourceDimensions?.width || 1));
    const targetHeight = Math.max(1, Math.min(DISPLAY_MAX_BACKING_HEIGHT, request.height || sourceDimensions?.height || 1));
    const fitScale = sourceDimensions
      ? Math.min(targetWidth / sourceDimensions.width, targetHeight / sourceDimensions.height)
      : 1;
    const fitWidth = sourceDimensions
      ? Math.max(1, Math.min(targetWidth, Math.round(sourceDimensions.width * fitScale)))
      : targetWidth;
    const fitHeight = sourceDimensions
      ? Math.max(1, Math.min(targetHeight, Math.round(sourceDimensions.height * fitScale)))
      : targetHeight;
    const rasterFallback = request.fallback !== "none" ? request.fallback : "bitmap-resize-unavailable";
    const rasterOptions = {
      resizeWidth: fitWidth,
      resizeHeight: fitHeight,
      resizeQuality: "high",
    };
    // A prepared physical source is sufficient to use the direct Blob path. It
    // avoids constructing a full Image and lets the browser decode only the
    // requested tier. Raster sources are never enlarged; SVG is vector-backed.
    const canResizeSource = sourceDimensions && (
      request.encoding === "svg"
      || (fitWidth <= sourceDimensions.width && fitHeight <= sourceDimensions.height)
    );
    if (typeof bitmapFactory === "function" && typeof fetch === "function"
        && request.source && canResizeSource) {
      try {
        const response = await fetch(request.source);
        if (response?.ok !== false && typeof response.blob === "function") {
          const blob = await response.blob();
          const bitmap = await bitmapFactory(blob, rasterOptions);
          if (bitmap) {
            const decodedWidth = boundedInteger(bitmap.width, 1, targetWidth, fitWidth);
            const decodedHeight = boundedInteger(bitmap.height, 1, targetHeight, fitHeight);
            if (decodedWidth === targetWidth && decodedHeight === targetHeight) {
              return resultForSprite(request, bitmap, targetWidth, targetHeight,
                request.fallback, sourceDimensions.width, sourceDimensions.height,
                decodedWidth, decodedHeight);
            }
            const surface = document.createElement?.("canvas");
            const surfaceContext = surface?.getContext?.("2d");
            if (!surface || !surfaceContext) {
              closeSpriteDrawable(bitmap);
              throw new Error("sprite contain surface unavailable");
            }
            surface.width = targetWidth;
            surface.height = targetHeight;
            try {
              surfaceContext.save?.();
              surfaceContext.clearRect?.(0, 0, targetWidth, targetHeight);
              surfaceContext.imageSmoothingEnabled = true;
              if ("imageSmoothingQuality" in surfaceContext) {
                surfaceContext.imageSmoothingQuality = "high";
              }
              surfaceContext.drawImage(bitmap,
                (targetWidth - decodedWidth) / 2,
                (targetHeight - decodedHeight) / 2,
                decodedWidth, decodedHeight);
              surfaceContext.restore?.();
            } catch (error) {
              closeSpriteDrawable(bitmap);
              throw error;
            }
            return resultForSprite(request, surface, targetWidth, targetHeight,
              request.fallback, sourceDimensions.width, sourceDimensions.height,
              decodedWidth, decodedHeight, bitmap, decodedWidth, decodedHeight, true);
          }
        }
      } catch (_) {
        // Fall through to the genuine Image.decode/canvas fallback and expose
        // the reason on the resulting receipt.
      }
    }
    const image = await request.loadImage();
    const sourceWidth = finitePositive(image.naturalWidth, sourceDimensions?.width || 0);
    const sourceHeight = finitePositive(image.naturalHeight, sourceDimensions?.height || 0);
    if (!sourceWidth || !sourceHeight) throw new Error("sprite image is unavailable");
    const underprovisioned = ["png", "jpeg", "jpg", "webp"].includes(request.encoding) &&
      (targetWidth > sourceWidth || targetHeight > sourceHeight);
    if (underprovisioned) {
      const raster = document.createElement?.("canvas");
      const rasterContext = raster?.getContext?.("2d");
      if (!raster || !rasterContext) throw new Error("sprite contain surface unavailable");
      raster.width = targetWidth;
      raster.height = targetHeight;
      rasterContext.save?.();
      rasterContext.clearRect?.(0, 0, targetWidth, targetHeight);
      rasterContext.imageSmoothingEnabled = true;
      if ("imageSmoothingQuality" in rasterContext) rasterContext.imageSmoothingQuality = "high";
      const logicalWidth = finitePositive(request.logicalWidth, targetWidth);
      const logicalHeight = finitePositive(request.logicalHeight, targetHeight);
      const physicalPerLogical = Math.min(
        targetWidth / logicalWidth,
        targetHeight / logicalHeight
      );
      const scale = Math.min(
        physicalPerLogical,
        targetWidth / sourceWidth,
        targetHeight / sourceHeight
      );
      const drawWidth = Math.max(1, Math.min(targetWidth, Math.round(sourceWidth * scale)));
      const drawHeight = Math.max(1, Math.min(targetHeight, Math.round(sourceHeight * scale)));
      rasterContext.drawImage(image, (targetWidth - drawWidth) / 2, (targetHeight - drawHeight) / 2,
        drawWidth, drawHeight);
      rasterContext.restore?.();
      return resultForSprite(request, raster, targetWidth, targetHeight,
        "source-underprovisioned", sourceWidth, sourceHeight,
        sourceWidth, sourceHeight, image, sourceWidth, sourceHeight);
    }
    if (!request.hasDimensions) {
      return resultForSprite(request, image, sourceWidth, sourceHeight,
        request.fallback, sourceWidth, sourceHeight);
    }
    const raster = document.createElement?.("canvas");
    const rasterContext = raster?.getContext?.("2d");
    if (!raster || !rasterContext) throw new Error("sprite raster canvas unavailable");
    raster.width = targetWidth;
    raster.height = targetHeight;
    rasterContext.save?.();
    rasterContext.clearRect?.(0, 0, targetWidth, targetHeight);
    rasterContext.imageSmoothingEnabled = true;
    if ("imageSmoothingQuality" in rasterContext) rasterContext.imageSmoothingQuality = "high";
    const scale = Math.min(targetWidth / sourceWidth, targetHeight / sourceHeight);
    const drawWidth = sourceWidth * scale;
    const drawHeight = sourceHeight * scale;
    rasterContext.drawImage(image, (targetWidth - drawWidth) / 2, (targetHeight - drawHeight) / 2,
      drawWidth, drawHeight);
    rasterContext.restore?.();
    return resultForSprite(request, raster, targetWidth, targetHeight,
      rasterFallback, sourceWidth, sourceHeight);
  };
  const closeSpriteDrawable = drawable => drawable?.close?.();
  const disposeSpriteCacheEntry = entry => {
    if (!entry || entry.closed || entry.pending > 0 || entry.users > 0 || !entry.evicted) return;
    if (spriteTierCache.get(entry.key) === entry) spriteTierCache.delete(entry.key);
    if (!entry.result) {
      if (entry.error) entry.closed = true;
      return;
    }
    entry.closed = true;
    closeSpriteDrawable(entry.result.drawable);
    if (entry.result.sourceDrawableOwned
        && entry.result.sourceDrawable
        && entry.result.sourceDrawable !== entry.result.drawable) {
      closeSpriteDrawable(entry.result.sourceDrawable);
    }
    spriteCacheBytes = Math.max(0, spriteCacheBytes - (entry.byteLength || 0));
    entry.byteLength = 0;
  };
  const evictSpriteCacheEntry = entry => {
    if (!entry) return;
    entry.evicted = true;
    if (spriteTierCache.get(entry.key) === entry) spriteTierCache.delete(entry.key);
    disposeSpriteCacheEntry(entry);
  };
  const releaseSpriteCacheEntry = entry => {
    if (!entry) return;
    entry.users = Math.max(0, entry.users - 1);
    if (entry.pending === 0 && entry.users === 0) entry.evicted = true;
    disposeSpriteCacheEntry(entry);
  };
  const trimSpriteCache = protectedEntry => {
    while (spriteCacheBytes > SPRITE_CACHE_MAX_BYTES) {
      let candidate;
      for (const entry of spriteTierCache.values()) {
        if (entry !== protectedEntry && entry.pending === 0 && entry.users === 0) {
          candidate = entry;
          break;
        }
      }
      if (!candidate) break;
      evictSpriteCacheEntry(candidate);
    }
  };
  const spritePreparationEntry = request => {
    let cached = spriteTierCache.get(request.key);
    if (cached?.evicted || cached?.closed) {
      if (spriteTierCache.get(request.key) === cached) spriteTierCache.delete(request.key);
      cached = null;
    }
    if (cached) {
      spriteCacheHits += 1;
      return cached;
    }
    cached = {
      key: request.key, promise: null, result: null, pending: 0, users: 0,
      evicted: false, closed: false
    };
    cached.promise = rasterSprite(request).then(result => {
      cached.result = result;
      cached.byteLength = (result.width || 0) * (result.height || 0) * 4
        + (result.sourceDrawableOwned
          && result.sourceDrawable
          && result.sourceDrawable !== result.drawable
          ? (result.sourceDrawableWidth || 0) * (result.sourceDrawableHeight || 0) * 4
          : 0);
      spriteCacheBytes += cached.byteLength;
      disposeSpriteCacheEntry(cached);
      return result;
    }).catch(error => {
      cached.error = error;
      if (cached.evicted && spriteTierCache.get(cached.key) === cached) spriteTierCache.delete(cached.key);
      throw error;
    });
    spriteTierCache.set(request.key, cached);
    while (spriteTierCache.size > MAX_SPRITE_CACHE_ENTRIES) {
      const oldest = spriteTierCache.keys().next().value;
      if (oldest === request.key) break;
      const evicted = spriteTierCache.get(oldest);
      evictSpriteCacheEntry(evicted);
    }
    spriteRasterCount += request.hasDimensions ? 1 : 0;
    return cached;
  };
  const noteSpriteDecoded = (request, result) => {
    if (!result?.decodedWidth || spriteDecodedSources.has(request.sourceIdentity)) return;
    spriteDecodedSources.add(request.sourceIdentity);
    spriteDecodedCount += 1;
  };
  // Return a preparation promise plus its cache entry without touching the
  // live resource. The caller performs the generation/key-checked commit.
  const prepareSprite = request => {
    const entry = spritePreparationEntry(request);
    entry.pending += 1;
    let activeLease = true;
    const lease = {
      commit: () => {
        if (!activeLease) return false;
        activeLease = false;
        entry.pending = Math.max(0, entry.pending - 1);
        entry.users += 1;
        disposeSpriteCacheEntry(entry);
        return true;
      },
      cancel: () => {
        if (!activeLease) return false;
        activeLease = false;
        entry.pending = Math.max(0, entry.pending - 1);
        if (entry.pending === 0 && entry.users === 0) entry.evicted = true;
        disposeSpriteCacheEntry(entry);
        return true;
      },
    };
    return {
      entry,
      lease,
      promise: entry.promise.then(result => {
        noteSpriteDecoded(request, result);
        return { request, result, entry, lease };
      }),
    };
  };
  const ensureSpriteImage = resource => {
    if (resource.imagePromise) return resource.imagePromise;
    if (typeof Image !== "function") return Promise.reject(new Error("Image API unavailable"));
    const image = new Image();
    resource.image = image;
    const decoded = typeof image.decode === "function"
      ? () => image.decode()
      : () => new Promise((resolve, reject) => {
        image.addEventListener?.("load", resolve, { once: true });
        image.addEventListener?.("error", () => reject(new Error("sprite image failed to load")), { once: true });
        if (image.complete && image.naturalWidth) resolve();
      });
    resource.imagePromise = Promise.resolve().then(decoded).then(() => {
      if (!image.naturalWidth || !image.naturalHeight) throw new Error("sprite image is unavailable");
      return image;
    });
    image.src = resource.source;
    return resource.imagePromise;
  };
  const commitSpritePreparation = (resource, prepared) => {
    const { request, result, entry, lease } = prepared;
    if (!lease.commit()) return false;
    const oldCacheEntry = resource.cacheEntry;
    const retained = Boolean(resource.ready && resource.drawable && oldCacheEntry);
    gpuBatcher?.releaseResource(resource);
    resource.drawable = result.drawable;
    resource.width = result.width;
    resource.height = result.height;
    resource.fallback = result.fallback || "none";
    resource.sourceIdentity = request.sourceIdentity;
    resource.sourceDrawable = result.sourceDrawable || null;
    resource.sourceDrawableWidth = result.sourceDrawableWidth || 0;
    resource.sourceDrawableHeight = result.sourceDrawableHeight || 0;
    resource.sourceDrawableOwned = Boolean(result.sourceDrawableOwned);
    resource.sourceWidth = result.sourceWidth || 0;
    resource.sourceHeight = result.sourceHeight || 0;
    resource.sourceBytes = result.sourceBytes || 0;
    resource.decodedWidth = result.decodedWidth || 0;
    resource.decodedHeight = result.decodedHeight || 0;
    resource.decodedBytes = result.decodedBytes || 0;
    resource.logicalWidth = request.logicalWidth;
    resource.logicalHeight = request.logicalHeight;
    resource.tier = request.tier;
    resource.tierKey = request.key;
    resource.cacheEntry = entry;
    resource.refreshing = false;
    resource.refreshError = null;
    resource.refreshFallback = "none";
    trimSpriteCache(entry);
    resource.pendingLease = null;
    resource.pendingEntry = null;
    resource.ready = true;
    const renderer = getGpuBatcher();
    if (!renderer) throw new Error("WebGL2 renderer unavailable during sprite publication");
    renderer.atlasFor(resource, spriteVariantFor(resource, false));
    if (resource.sourceDrawable && resource.sourceDrawableWidth && resource.sourceDrawableHeight) {
      renderer.atlasFor(resource, spriteVariantFor(resource, true));
    }
    if (oldCacheEntry) releaseSpriteCacheEntry(oldCacheEntry);
    publishAssetReceipt(resource);
    return { retained };
  };
  const startSpritePreparation = (resource, onReady) => {
    const retained = Boolean(resource.ready && resource.drawable && resource.cacheEntry);
    resource.pendingLease?.cancel();
    resource.pendingLease = null;
    resource.pendingEntry = null;
    const generation = resource.generation;
    const request = spriteRasterRequest(resource);
    resource.pendingTierKey = request.key;
    resource.refreshing = retained;
    resource.refreshError = retained ? null : resource.refreshError;
    resource.refreshFallback = retained ? "pending" : resource.refreshFallback;
    if (!retained) {
      resource.ready = false;
      resource.error = null;
    }
    const preparation = prepareSprite(request);
    resource.pendingLease = preparation.lease;
    resource.pendingEntry = preparation.entry;
    resource.readyPromise = preparation.promise
      .then(prepared => {
        const current = resource.generation === generation
          && resource.pendingTierKey === request.key;
        if (!current) {
          prepared.lease.cancel();
          spriteStaleCount += 1;
          publishDisplayReceipt();
          return resource;
        }
        const committed = commitSpritePreparation(resource, prepared);
        if (committed && !committed.retained) onReady?.(resource, true);
        return resource;
      })
      .catch(error => {
        const current = resource.generation === generation
          && resource.pendingTierKey === request.key;
        if (!current) {
          preparation.lease.cancel();
          spriteStaleCount += 1;
          publishDisplayReceipt();
          return resource;
        }
        preparation.lease.cancel();
        resource.pendingLease = null;
        if (resource.pendingEntry === preparation.entry) resource.pendingEntry = null;
        if (retained && resource.ready && resource.drawable && resource.cacheEntry) {
          resource.refreshing = false;
          resource.refreshError = error;
          resource.refreshFallback = request.fallback === "none"
            ? "refresh-error" : request.fallback;
          publishAssetReceipt(resource);
          return resource;
        }
        resource.error = error;
        resource.ready = false;
        resource.fallback = request.fallback || "bitmap-resize-unavailable";
        publishAssetReceipt(resource);
        onReady?.(resource, false);
        return resource;
      });
    return resource.readyPromise;
  };
  const createSpriteResource = (pathId, width, height, onReady) => {
    const source = assetValue(pathId);
    const resource = {
      image: null, imagePromise: null, source, metadata: assetMetadata(pathId), sourceIdentity: source,
      drawable: null, sourceDrawable: null, sourceDrawableWidth: 0, sourceDrawableHeight: 0,
      sourceDrawableOwned: false,
      requested: spriteDimensions(width, height),
      logicalWidth: 0, logicalHeight: 0, tier: 1, tierKey: "", pendingTierKey: "",
      width: 0, height: 0, sourceWidth: 0, sourceHeight: 0, sourceBytes: 0,
      decodedWidth: 0, decodedHeight: 0, decodedBytes: 0,
      ready: false, error: null, generation: 1, cacheEntry: null, pendingEntry: null,
      pendingLease: null,
      refreshing: false, refreshError: null, refreshFallback: "none",
      fallback: "none", readyPromise: null, onReady
    };
    startSpritePreparation(resource, onReady);
    return resource;
  };
  const loadSprite = (pathId, width, height) => {
    const handle = nextHandle++;
    sprites.set(handle, createSpriteResource(pathId, width, height));
    return handle;
  };
  const invalidateDensityResources = () => {
    let invalidated = 0;
    for (const resource of sprites.values()) {
      resource.generation += 1;
      resource.pendingLease?.cancel();
      resource.pendingLease = null;
      resource.pendingEntry = null;
      startSpritePreparation(resource, resource.onReady);
      invalidated += 1;
    }
    for (const font of fonts.values()) {
      font.densityTier = display.densityTier;
      font.densityGeneration = display.densityGeneration;
      font.cacheKey = [font.source, font.size, display.densityTier, RASTER_OPTIONS].join(":");
    }
    if (document.body?.dataset) {
      document.body.dataset.assetDensityInvalidations = String(invalidated);
      document.body.dataset.assetDensityGeneration = String(display.densityGeneration);
    }
  };
  onDensityChange = invalidateDensityResources;
  const setPreparationFont = (context, font, size = font.renderSize) => {
    context.font = `${size}px ${font.family}`;
    context.textBaseline = "alphabetic";
    if ("fontKerning" in context) context.fontKerning = "none";
  };
  const measureTextRun = (font, text) => {
    const { context } = resourcePreparationContext();
    context.save();
    setPreparationFont(context, font);
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
    const entry = cachedText.get(run.handle);
    if (!entry || entry.generation !== run.generation) return;
    if (getViewField(run.base, run.index, "font") !== run.font
      || getViewField(run.base, run.index, "handle") !== run.handle) return;
    const metrics = measureTextRun(font, run.text);
    entry.width = metrics.width;
    entry.height = metrics.height;
    setViewField(run.base, run.index, "width", metrics.width);
    setViewField(run.base, run.index, "height", metrics.height);
  };
  const queuePendingTextRun = (font, run) => {
    const prior = font.pendingRuns.findIndex(
      pending => pending.base === run.base && pending.index === run.index,
    );
    if (prior >= 0) font.pendingRuns[prior] = run;
    else font.pendingRuns.push(run);
  };
  const calibrateFont = font => {
    const { context } = resourcePreparationContext();
    context.save();
    setPreparationFont(context, font, 1000);
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
      family, size, renderSize: size, baseline: size, ready: false, pendingRuns: [],
      source: assetValue(pathId), metadata: assetMetadata(pathId),
      densityTier: display.densityTier, densityGeneration: display.densityGeneration,
      cacheKey: [assetValue(pathId), size, display.densityTier, RASTER_OPTIONS].join(":")
    };
    fonts.set(handle, fontInfo);
    const load = Promise.resolve()
      .then(() => font.load())
      .then(loaded => {
        document.fonts.add(loaded);
        if (document.body?.dataset) {
          document.body.dataset.fontSource = fontInfo.source;
          document.body.dataset.fontRasterTier = displayNumber(fontInfo.densityTier);
          document.body.dataset.fontDensityGeneration = String(fontInfo.densityGeneration);
        }
        return loaded;
      });
    fontLoads.set(handle, load);
    return handle;
  };
  const measureText = (fontHandle, textId) => {
    const font = fonts.get(fontHandle);
    if (!font) return 0;
    const { context } = resourcePreparationContext();
    context.save();
    setPreparationFont(context, font);
    const width = context.measureText(stringValue(textId)).width;
    context.restore();
    return width;
  };
  const releaseSprite = handle => {
    const resource = sprites.get(handle);
    if (resource) {
      resource.generation += 1;
      resource.pendingLease?.cancel();
      resource.pendingLease = null;
      resource.pendingEntry = null;
      releaseSpriteCacheEntry(resource.cacheEntry);
      resource.cacheEntry = null;
      trimSpriteCache(null);
      resource.ready = false;
      resource.drawable = null;
      resource.sourceDrawable = null;
      resource.sourceDrawableWidth = 0;
      resource.sourceDrawableHeight = 0;
      resource.sourceDrawableOwned = false;
      resource.refreshing = false;
      resource.refreshError = null;
      resource.refreshFallback = "none";
      sprites.delete(handle);
      gpuBatcher?.releaseResource(resource);
      resource.image = null;
      resource.imagePromise = null;
      if (latestAssetResource === resource) {
        // Publish the terminal state while the receipt still points at this
        // resource, then drop the diagnostic reference so its decoded Image
        // cannot be retained after release. The already-published asset
        // fields remain in the body dataset for later display receipts.
        publishDisplayReceipt();
        latestAssetResource = undefined;
      }
    }
  };
  const requestSprite = (pathId, width, height) => {
    const task = nextAssetTask++;
    const handle = nextHandle++;
    const entry = { state: 1, handle, kind: "sprite" };
    assetTasks.set(task, entry);
    entry.state = 2;
    sprites.set(handle, createSpriteResource(pathId, width, height,
      (_resource, ready) => { if (entry.state < 3) entry.state = ready ? 3 : 4; }));
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
    // @stasis-import web_input_axis begin
    web_input_axis: () => (keys.has("ArrowRight") || keys.has("KeyD") ? 1 : 0) - (keys.has("ArrowLeft") || keys.has("KeyA") ? 1 : 0),
    // @stasis-import web_input_axis end
    // @stasis-import web_input_fire begin
    web_input_fire: () => keys.has("Space") || pointer.down ? 1 : 0,
    // @stasis-import web_input_fire end
    // @stasis-import web_pointer_x begin
    web_pointer_x: () => pointer.x | 0,
    // @stasis-import web_pointer_x end
    // @stasis-import web_pointer_down begin
    web_pointer_down: () => pointer.down ? 1 : 0,
    // @stasis-import web_pointer_down end
    // @stasis-import stasis_jit_open_external_url begin
    stasis_jit_open_external_url: openExternalUrl,
    // @stasis-import stasis_jit_open_external_url end
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
    gfx_load_sprite: (pathId, width, height) => loadSprite(pathId, width, height),
    stasis_gfx_load_sprite: (pathId, width, height) => loadSprite(pathId, width, height),
    gfx_release_sprite: handle => releaseSprite(handle),
    stasis_gfx_release_sprite: handle => releaseSprite(handle),
    stasis_jit_gfx_release_sprite: handle => releaseSprite(handle),
    stasis_jit_asset_request_sprite: (pathId, width, height) => requestSprite(pathId, width, height),
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
      const handle = loadSprite(pathId, width, height);
      return setViewField(base, index, "sprite_ref", handle)
        && setViewField(base, index, "width", width)
        && setViewField(base, index, "height", height) ? 1 : 0;
    },
    stasis_jit_gfx_cache_text: (font, textId) => {
      const value = runtimeTextValue(textId);
      if (!value) return 0;
      const { text, bytes } = value;
      const key = `${font}:${text}`;
      const prior = immutableTextHandles.get(key);
      if (prior) return prior;
      if (cachedText.size >= TEXT_RUN_MAX_ENTRIES || cachedTextBytes + bytes > TEXT_RUN_MAX_BYTES) return 0;
      const handle = nextHandle++;
      cachedText.set(handle, { font, text, bytes, replaceable: false, generation: 0 });
      immutableTextHandles.set(key, handle);
      cachedTextBytes += bytes;
      return handle;
    },
    stasis_jit_text_run_load_from: (base, index, _len, font, textId) => {
      if (!canWriteTextRun(base, index)) return 0;
      const value = runtimeTextValue(textId);
      if (!value) return 0;
      const { text } = value;
      const key = `${font}:${text}`;
      let handle = immutableTextHandles.get(key);
      if (!handle) {
        const bytes = value.bytes;
        if (cachedText.size >= TEXT_RUN_MAX_ENTRIES || cachedTextBytes + bytes > TEXT_RUN_MAX_BYTES) return 0;
        handle = nextHandle++;
        cachedText.set(handle, { font, text, bytes, replaceable: false, generation: 0 });
        immutableTextHandles.set(key, handle);
        cachedTextBytes += bytes;
      }
      const fontInfo = fonts.get(font) || { size: 16 };
      const run = { base, index, font, text, handle, generation: 0 };
      const loaded = setViewField(base, index, "font", font)
        && setViewField(base, index, "handle", handle)
        && setViewField(base, index, "width", text.length * fontInfo.size * 0.6)
        && setViewField(base, index, "height", fontInfo.size);
      if (loaded && fontInfo.ready) refreshTextRun(run);
      else if (loaded && fontInfo.pendingRuns) queuePendingTextRun(fontInfo, run);
      return loaded ? 1 : 0;
    },
    stasis_jit_text_run_replace_from: (base, index, _len, font, textId) => {
      if (font <= 0 || !canWriteTextRun(base, index)) return 0;
      const value = runtimeTextValue(textId);
      if (!value) return 0;
      const { text, bytes } = value;
      if (bytes === 0 || bytes > DYNAMIC_TEXT_MAX_BYTES) return 0;
      const fontInfo = fonts.get(font);
      if (!fontInfo) return 0;
      const oldHandle = getViewField(base, index, "handle");
      const old = cachedText.get(oldHandle);
      const reuse = old?.replaceable === true;
      if (!reuse && cachedText.size >= TEXT_RUN_MAX_ENTRIES) return 0;
      const priorBytes = reuse ? old.bytes : 0;
      if (cachedTextBytes - priorBytes + bytes > TEXT_RUN_MAX_BYTES) return 0;
      const handle = reuse ? oldHandle : nextHandle++;
      const generation = reuse ? old.generation + 1 : 1;
      const entry = { font, text, bytes, replaceable: true, generation,
        width: text.length * fontInfo.size * 0.6, height: fontInfo.size };
      cachedText.set(handle, entry);
      cachedTextBytes += bytes - priorBytes;
      const loaded = setViewField(base, index, "font", font)
        && setViewField(base, index, "handle", handle)
        && setViewField(base, index, "width", entry.width)
        && setViewField(base, index, "height", entry.height);
      if (!loaded) {
        if (reuse) cachedText.set(handle, old);
        else cachedText.delete(handle);
        cachedTextBytes -= bytes - priorBytes;
        return 0;
      }
      const run = { base, index, font, text, handle, generation };
      if (fontInfo.ready) refreshTextRun(run);
      else if (fontInfo.pendingRuns) queuePendingTextRun(fontInfo, run);
      return 1;
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

  // The visible canvas has exactly one renderer. Guest memory is copied into
  // reusable host staging arrays before upload; Canvas2D is resource prep only.
  function getGpuBatcher() {
    if (gpuBatcher !== undefined) return gpuBatcher;
    try {
      const target = canvas;
      const gl = target.getContext("webgl2", {
        alpha: false, premultipliedAlpha: true, antialias: false,
        preserveDrawingBuffer: false
      });
      if (!gl) throw new Error("WebGL2 is required by the Stasis Web renderer");
      const makeProgram = (name, vertexSource, fragmentSource) => {
        const vertex = gl.createShader(gl.VERTEX_SHADER);
        const fragment = gl.createShader(gl.FRAGMENT_SHADER);
        const program = gl.createProgram();
        if (!vertex || !fragment || !program) throw new Error(`${name} WebGL allocation failed`);
        gl.shaderSource(vertex, vertexSource);
        gl.compileShader(vertex);
        if (!gl.getShaderParameter(vertex, gl.COMPILE_STATUS)) {
          throw new Error(`${name} vertex shader failed: ${gl.getShaderInfoLog?.(vertex) || "unknown compile error"}`);
        }
        gl.shaderSource(fragment, fragmentSource);
        gl.compileShader(fragment);
        if (!gl.getShaderParameter(fragment, gl.COMPILE_STATUS)) {
          throw new Error(`${name} fragment shader failed: ${gl.getShaderInfoLog?.(fragment) || "unknown compile error"}`);
        }
        gl.attachShader(program, vertex);
        gl.attachShader(program, fragment);
        gl.linkProgram(program);
        if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
          throw new Error(`${name} program failed: ${gl.getProgramInfoLog?.(program) || "unknown link error"}`);
        }
        return { program, vertex, fragment };
      };
      const spriteProgram = makeProgram("sprite", `#version 300 es
        layout(location = 0) in vec2 p;
        layout(location = 1) in vec4 rect;
        layout(location = 2) in vec4 uv;
        layout(location = 3) in vec4 color;
        layout(location = 4) in vec4 rotation;
        uniform vec2 size;
        out vec2 textureUv;
        out vec4 vertexColor;
        void main() {
          vec2 local = p * rect.zw - rotation.zw;
          vec2 rotated = vec2(local.x * rotation.y - local.y * rotation.x,
            local.x * rotation.x + local.y * rotation.y);
          vec2 q = rect.xy + rotation.zw + rotated;
          gl_Position = vec4(q.x / size.x * 2.0 - 1.0,
            1.0 - q.y / size.y * 2.0, 0.0, 1.0);
          textureUv = mix(uv.xy, uv.zw, p);
          vertexColor = color;
        }`, `#version 300 es
        precision mediump float;
        uniform sampler2D sprite;
        in vec2 textureUv;
        in vec4 vertexColor;
        out vec4 outputColor;
        void main() { outputColor = texture(sprite, textureUv) * vertexColor; }`);
      const spriteVao = gl.createVertexArray();
      const unitBuffer = gl.createBuffer();
      const instanceBuffer = gl.createBuffer();
      if (!spriteVao || !unitBuffer || !instanceBuffer) throw new Error("WebGL buffers failed");
      gl.bindBuffer(gl.ARRAY_BUFFER, unitBuffer);
      gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([0, 0, 1, 0, 0, 1, 1, 1]), gl.STATIC_DRAW);
      gl.bindVertexArray(spriteVao);
      gl.bindBuffer(gl.ARRAY_BUFFER, unitBuffer);
      gl.enableVertexAttribArray(0);
      gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
      gl.bindBuffer(gl.ARRAY_BUFFER, instanceBuffer);
      gl.bufferData(gl.ARRAY_BUFFER, spriteScratch.byteLength, gl.DYNAMIC_DRAW);
      for (const [attribute, offset] of [[1, 0], [2, 16], [3, 32], [4, 48]]) {
        gl.enableVertexAttribArray(attribute);
        gl.vertexAttribPointer(attribute, 4, gl.FLOAT, false, 64, offset);
        gl.vertexAttribDivisor(attribute, 1);
      }
      gl.bindVertexArray(null);
      const spriteSize = gl.getUniformLocation(spriteProgram.program, "size");
      const spriteSampler = gl.getUniformLocation(spriteProgram.program, "sprite");
      const atlasPages = [];
      const atlasByResource = new WeakMap();
      const maxTextureSize = Math.max(1, Number(gl.getParameter?.(gl.MAX_TEXTURE_SIZE)) || ATLAS_PAGE_MAX);
      // Upload counters are cumulative for this helper/context lifetime;
      // page and live-entry counts describe the current bounded atlas.
      let atlasUploadCount = 0;
      let atlasUploadBytes = 0;
      let frameTextureBinds = 0;
      let frameAtlasTransitions = 0;
      let lastTexture = null;
      let stagingCanvas;
      let stagingContext;
      let lost = false;
      const failIfLost = () => {
        if (lost || (typeof gl.isContextLost === "function" && gl.isContextLost())) throw new Error("WebGL context lost");
      };
      const failIfBad = () => {
        failIfLost();
        if (typeof gl.getError === "function" && gl.getError() !== (gl.NO_ERROR ?? 0)) throw new Error("WebGL error");
      };
      const dispose = () => {
        for (const page of atlasPages) gl.deleteTexture?.(page.texture);
        atlasPages.length = 0;
        gl.deleteBuffer?.(instanceBuffer);
        gl.deleteBuffer?.(unitBuffer);
        gl.deleteVertexArray?.(spriteVao);
        gl.deleteProgram?.(spriteProgram.program);
      };
      const fail = () => {
        lost = true;
        dispose();
        if (gpuBatcher?.target === target) gpuBatcher = null;
      };
      loseWebGlRenderer = fail;
      if (!webglLifecycleInstalled) {
        webglLifecycleInstalled = true;
        target.addEventListener?.("webglcontextlost", event => {
          event.preventDefault?.();
          loseWebGlRenderer();
        });
        target.addEventListener?.("webglcontextrestored", () => {
          gpuBatcher = undefined;
          const restored = getGpuBatcher();
          if (!restored) return;
          try {
            for (const resource of sprites.values()) {
              if (!resource.ready) continue;
              restored.atlasFor(resource, spriteVariantFor(resource, false));
              if (resource.sourceDrawable && resource.sourceDrawableWidth && resource.sourceDrawableHeight) {
                restored.atlasFor(resource, spriteVariantFor(resource, true));
              }
            }
            for (const resource of preparedText.values()) restored.atlasFor(resource, null);
          } catch (error) {
            document.body.dataset.gpuError = String(error);
            gpuBatcher = null;
          }
        });
      }
      const createAtlasPage = size => {
        if (atlasPages.length >= ATLAS_MAX_PAGES) throw new Error("WebGL2 atlas page capacity exhausted");
        if (size > maxTextureSize) throw new Error("WebGL2 atlas page exceeds MAX_TEXTURE_SIZE");
        const allocatedBytes = atlasPages.reduce((total, page) => total + page.size * page.size * 4, 0);
        if (allocatedBytes + size * size * 4 > ATLAS_MAX_BYTES) {
          throw new Error("WebGL2 atlas memory capacity exhausted");
        }
        const texture = gl.createTexture();
        if (!texture) throw new Error("WebGL atlas texture allocation failed");
        gl.bindTexture(gl.TEXTURE_2D, texture);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
        gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, size, size, 0,
          gl.RGBA, gl.UNSIGNED_BYTE, null);
        const solidPixels = new Uint8Array([
          255, 255, 255, 255, 255, 255, 255, 255,
          255, 255, 255, 255, 255, 255, 255, 255
        ]);
        gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, 2, 2, gl.RGBA, gl.UNSIGNED_BYTE, solidPixels);
        failIfBad();
        const page = {
          texture, size, cursorX: 2, cursorY: 0, rowHeight: 2,
          solidUv: 0.5 / size,
          entries: new Set(), freeRects: []
        };
        atlasUploadCount += 1;
        atlasUploadBytes += solidPixels.byteLength;
        atlasPages.push(page);
        return page;
      };
      const deleteAtlasPage = page => {
        const index = atlasPages.indexOf(page);
        if (index >= 0) atlasPages.splice(index, 1);
        gl.deleteTexture?.(page.texture);
        page.deleted = true;
      };
      const coalesceFreeRects = page => {
        let merged = true;
        while (merged) {
          merged = false;
          outer: for (let left = 0; left < page.freeRects.length; left += 1) {
            for (let right = left + 1; right < page.freeRects.length; right += 1) {
              const first = page.freeRects[left];
              const second = page.freeRects[right];
              if (first.y === second.y && first.height === second.height
                  && (first.x + first.width === second.x || second.x + second.width === first.x)) {
                const x = Math.min(first.x, second.x);
                page.freeRects[left] = {
                  x, y: first.y, width: Math.max(first.x + first.width, second.x + second.width) - x,
                  height: first.height
                };
                page.freeRects.splice(right, 1);
                merged = true;
                break outer;
              }
              if (first.x === second.x && first.width === second.width
                  && (first.y + first.height === second.y || second.y + second.height === first.y)) {
                const y = Math.min(first.y, second.y);
                page.freeRects[left] = {
                  x: first.x, y, width: first.width,
                  height: Math.max(first.y + first.height, second.y + second.height) - y
                };
                page.freeRects.splice(right, 1);
                merged = true;
                break outer;
              }
            }
          }
        }
      };
      const releaseAtlasEntry = entry => {
        if (!entry || entry.released) return;
        entry.released = true;
        const page = entry.page;
        page.entries.delete(entry);
        if (entry.allocation && !page.deleted) {
          page.freeRects.push({ ...entry.allocation });
          coalesceFreeRects(page);
        }
        if (page.entries.size === 0 && !page.deleted) deleteAtlasPage(page);
      };
      const allocateAtlasRect = (page, width, height) => {
        const previous = {
          cursorX: page.cursorX, cursorY: page.cursorY, rowHeight: page.rowHeight,
          freeRects: page.freeRects.map(rect => ({ ...rect }))
        };
        const freeIndex = page.freeRects.findIndex(rect =>
          rect.width >= width && rect.height >= height);
        if (freeIndex >= 0) {
          const rect = page.freeRects.splice(freeIndex, 1)[0];
          if (rect.width > width) {
            page.freeRects.push({
              x: rect.x + width, y: rect.y, width: rect.width - width, height: rect.height
            });
          }
          if (rect.height > height) {
            page.freeRects.push({
              x: rect.x, y: rect.y + height, width, height: rect.height - height
            });
          }
          return { x: rect.x, y: rect.y, width, height, previous, source: "free" };
        }
        let x = page.cursorX;
        let y = page.cursorY;
        if (x + width > page.size) {
          x = 0;
          y += page.rowHeight;
        }
        if (y + height > page.size) return null;
        page.cursorX = x + width;
        page.cursorY = y;
        page.rowHeight = y === previous.cursorY
          ? Math.max(previous.rowHeight, height) : height;
        return { x, y, width, height, previous, source: "shelf" };
      };
      const rollbackAtlasAllocation = (page, allocation) => {
        page.cursorX = allocation.previous.cursorX;
        page.cursorY = allocation.previous.cursorY;
        page.rowHeight = allocation.previous.rowHeight;
        page.freeRects = allocation.previous.freeRects.map(rect => ({ ...rect }));
      };
      const uploadAtlasEntry = (page, variant, entry) => {
        stagingCanvas ||= document.createElement?.("canvas");
        stagingContext ||= stagingCanvas?.getContext?.("2d");
        if (!stagingCanvas || !stagingContext) throw new Error("sprite atlas staging unavailable");
        const width = variant.width;
        const height = variant.height;
        const paddedWidth = width + ATLAS_PADDING * 2;
        const paddedHeight = height + ATLAS_PADDING * 2;
        stagingCanvas.width = paddedWidth;
        stagingCanvas.height = paddedHeight;
        stagingContext.clearRect?.(0, 0, paddedWidth, paddedHeight);
        stagingContext.imageSmoothingEnabled = false;
        stagingContext.drawImage(variant.drawable, ATLAS_PADDING, ATLAS_PADDING, width, height);
        // Extrude each edge into the padding to keep LINEAR samples away from
        // neighboring material, including allocations recycled after refresh.
        stagingContext.drawImage(variant.drawable, 0, 0, 1, height, 0, ATLAS_PADDING, ATLAS_PADDING, height);
        stagingContext.drawImage(variant.drawable, width - 1, 0, 1, height,
          ATLAS_PADDING + width, ATLAS_PADDING, ATLAS_PADDING, height);
        stagingContext.drawImage(variant.drawable, 0, 0, width, 1, ATLAS_PADDING, 0, width, ATLAS_PADDING);
        stagingContext.drawImage(variant.drawable, 0, height - 1, width, 1,
          ATLAS_PADDING, ATLAS_PADDING + height, width, ATLAS_PADDING);
        stagingContext.drawImage(variant.drawable, 0, 0, 1, 1,
          0, 0, ATLAS_PADDING, ATLAS_PADDING);
        stagingContext.drawImage(variant.drawable, width - 1, 0, 1, 1,
          ATLAS_PADDING + width, 0, ATLAS_PADDING, ATLAS_PADDING);
        stagingContext.drawImage(variant.drawable, 0, height - 1, 1, 1,
          0, ATLAS_PADDING + height, ATLAS_PADDING, ATLAS_PADDING);
        stagingContext.drawImage(variant.drawable, width - 1, height - 1, 1, 1,
          ATLAS_PADDING + width, ATLAS_PADDING + height, ATLAS_PADDING, ATLAS_PADDING);
        gl.bindTexture(gl.TEXTURE_2D, page.texture);
        gl.texSubImage2D(gl.TEXTURE_2D, 0, entry.x - ATLAS_PADDING, entry.y - ATLAS_PADDING,
          gl.RGBA, gl.UNSIGNED_BYTE, stagingCanvas);
        failIfBad();
        atlasUploadCount += 1;
        atlasUploadBytes += paddedWidth * paddedHeight * 4;
      };
      const atlasFor = (resource, variant) => {
        const selected = variant?.drawable ? variant : {
          key: "full", drawable: resource.drawable, width: resource.width, height: resource.height
        };
        const variantKey = selected.key || "full";
        let variants = atlasByResource.get(resource);
        const old = variants?.get(variantKey);
        if (old && (old.generation === resource.generation
          || resource.refreshing || resource.refreshError)) return old;
        if (old) {
          releaseAtlasEntry(old);
          variants.delete(variantKey);
        }
        if (!resource.ready || !selected.drawable || !selected.width || !selected.height) return null;
        const paddedWidth = selected.width + ATLAS_PADDING * 2;
        const paddedHeight = selected.height + ATLAS_PADDING * 2;
        if (paddedWidth > maxTextureSize || paddedHeight > maxTextureSize) {
          throw new Error("Sprite exceeds WebGL2 MAX_TEXTURE_SIZE");
        }
        let pageSize = ATLAS_PAGE_SIZE;
        while (pageSize < paddedWidth || pageSize < paddedHeight) pageSize *= 2;
        pageSize = Math.min(pageSize, maxTextureSize);
        let page = null;
        let allocation = null;
        for (const candidate of atlasPages) {
          if (paddedWidth > candidate.size || paddedHeight > candidate.size) continue;
          const candidateAllocation = allocateAtlasRect(candidate, paddedWidth, paddedHeight);
          if (candidateAllocation) {
            page = candidate;
            allocation = candidateAllocation;
            break;
          }
        }
        let createdPage = false;
        if (!page) {
          page = createAtlasPage(pageSize);
          if (!page) return null;
          createdPage = true;
          allocation = allocateAtlasRect(page, paddedWidth, paddedHeight);
          if (!allocation) {
            deleteAtlasPage(page);
            return null;
          }
        }
        const entry = {
          page, x: allocation.x + ATLAS_PADDING, y: allocation.y + ATLAS_PADDING,
          width: selected.width, height: selected.height,
          generation: resource.generation, variantKey,
          allocation: {
            x: allocation.x, y: allocation.y, width: allocation.width, height: allocation.height
          }
        };
        try {
          uploadAtlasEntry(page, selected, entry);
        } catch (error) {
          rollbackAtlasAllocation(page, allocation);
          if (createdPage) {
            deleteAtlasPage(page);
          }
          throw error;
        }
        page.entries.add(entry);
        if (!variants) {
          variants = new Map();
          atlasByResource.set(resource, variants);
        }
        variants.set(variantKey, entry);
        return entry;
      };
      const draw = (values, count, texture) => {
        const width = display.backingWidth;
        const height = display.backingHeight;
        const logicalWidth = Math.max(1, display.logicalWidth);
        const logicalHeight = Math.max(1, display.logicalHeight);
        failIfBad();
        gl.viewport(0, 0, width, height);
        gl.useProgram(spriteProgram.program);
        gl.uniform2f(spriteSize, logicalWidth, logicalHeight);
        gl.uniform1i?.(spriteSampler, 0);
        gl.bindVertexArray(spriteVao);
        gl.bindBuffer(gl.ARRAY_BUFFER, instanceBuffer);
        gl.bufferSubData(gl.ARRAY_BUFFER, 0, values, 0, count * 16);
        gl.enable(gl.BLEND);
        gl.blendFuncSeparate(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA, gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
        gl.activeTexture?.(gl.TEXTURE0);
        gl.bindTexture(gl.TEXTURE_2D, texture);
        frameTextureBinds += 1;
        if (lastTexture !== null && lastTexture !== texture) frameAtlasTransitions += 1;
        lastTexture = texture;
        gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, count);
        gl.bindVertexArray(null);
        failIfBad();
      };
      return (gpuBatcher = {
        target,
        flush: () => {},
        resetFrameMetrics: () => {
          frameTextureBinds = 0;
          frameAtlasTransitions = 0;
          lastTexture = null;
        },
        beginFrame: (red, green, blue, alpha = 1) => {
          failIfLost();
          gl.disable(gl.SCISSOR_TEST);
          gl.viewport(0, 0, display.backingWidth, display.backingHeight);
          gl.clearColor(red, green, blue, alpha);
          gl.clear(gl.COLOR_BUFFER_BIT);
        },
        setClip: clip => {
          if (!clip) { gl.disable(gl.SCISSOR_TEST); return; }
          const sx = display.backingWidth / Math.max(1, display.logicalWidth);
          const sy = display.backingHeight / Math.max(1, display.logicalHeight);
          const x = Math.max(0, Math.floor(clip.x * sx));
          const top = Math.max(0, Math.floor(clip.y * sy));
          const right = Math.min(display.backingWidth, Math.ceil((clip.x + clip.width) * sx));
          const bottom = Math.min(display.backingHeight, Math.ceil((clip.y + clip.height) * sy));
          gl.enable(gl.SCISSOR_TEST);
          gl.scissor(x, display.backingHeight - bottom, Math.max(0, right - x), Math.max(0, bottom - top));
        },
        atlasFor,
        solidFor: preferredPage => {
          const page = (!preferredPage?.deleted && preferredPage)
            || atlasPages.find(candidate => !candidate.deleted) || createAtlasPage(ATLAS_PAGE_SIZE);
          return page ? { page, uv: page.solidUv } : null;
        },
        drawSprites: (values, count, page) => draw(values, count, page.texture),
        releaseResource: resource => {
          const variants = atlasByResource.get(resource);
          if (!variants) return;
          atlasByResource.delete(resource);
          for (const entry of variants.values()) releaseAtlasEntry(entry);
        },
        metrics: () => ({
          pages: atlasPages.length,
          liveEntries: atlasPages.reduce((total, page) => total + page.entries.size, 0),
          allocatedBytes: atlasPages.reduce((total, page) => total + page.size * page.size * 4, 0),
          width: atlasPages.reduce((total, page) => total + page.size, 0),
          height: atlasPages.reduce((maximum, page) => Math.max(maximum, page.size), 0),
          generation: atlasPages.reduce((maximum, page) => Math.max(
            maximum, ...Array.from(page.entries, entry => entry.generation || 0)
          ), 0),
          uploadCount: atlasUploadCount,
          uploadBytes: atlasUploadBytes,
          frameTextureBinds,
          frameAtlasTransitions
        })
      });
    } catch (error) {
      document.body.dataset.gpuError = String(error);
      document.body.dataset.backend = "unsupported";
      setLoading("This game requires WebGL2.", "failed");
      if (errorBox) errorBox.textContent = String(error?.message || error);
      return (gpuBatcher = null);
    }
  }
  const writeQuad = (target, x, y, width, height, atlas, red, green, blue, alpha,
    radians = 0, pivotX = width * 0.5, pivotY = height * 0.5) => {
    spriteScratch[target] = x;
    spriteScratch[target + 1] = y;
    spriteScratch[target + 2] = width;
    spriteScratch[target + 3] = height;
    if (Object.prototype.hasOwnProperty.call(atlas, "uv")) {
      for (let field = 4; field < 8; field += 1) spriteScratch[target + field] = atlas.uv;
    } else {
      spriteScratch[target + 4] = atlas.u0;
      spriteScratch[target + 5] = atlas.v0;
      spriteScratch[target + 6] = atlas.u1;
      spriteScratch[target + 7] = atlas.v1;
    }
    spriteScratch[target + 8] = red;
    spriteScratch[target + 9] = green;
    spriteScratch[target + 10] = blue;
    spriteScratch[target + 11] = alpha;
    spriteScratch[target + 12] = Math.sin(radians);
    spriteScratch[target + 13] = Math.cos(radians);
    spriteScratch[target + 14] = pivotX;
    spriteScratch[target + 15] = pivotY;
  };
  const drawImmediateSolid = (x, y, width, height, red, green, blue, alpha = 1,
    radians = 0, pivotX = width * 0.5, pivotY = height * 0.5) => {
    const renderer = getGpuBatcher();
    if (!renderer) return false;
    const atlas = renderer.solidFor();
    if (!atlas) throw new Error("WebGL2 solid atlas allocation failed");
    writeQuad(0, x, y, width, height, atlas, red, green, blue, alpha, radians, pivotX, pivotY);
    renderer.drawSprites(spriteScratch, 1, atlas.page);
    performanceWorkload.instances += 1;
    performanceWorkload.batches += 1;
    performanceWorkload.drawCalls += 1;
    performanceWorkload.uploadedBytes += 16 * Float32Array.BYTES_PER_ELEMENT;
    return true;
  };
  const preparedTextResource = (fontHandle, text) => {
    const font = fonts.get(fontHandle) || {
      family: "ui-monospace, Consolas, monospace", size: 18, renderSize: 18, baseline: 18,
      densityGeneration: display.densityGeneration
    };
    const key = `${fontHandle}|${font.densityGeneration || 0}|${text}`;
    const existing = preparedText.get(key);
    if (existing) {
      // Map iteration order is the LRU order used by the bounded cache.
      preparedText.delete(key);
      preparedText.set(key, existing);
      return existing;
    }
    const surface = document.createElement?.("canvas");
    const preparation = surface?.getContext?.("2d", { alpha: true });
    if (!surface || !preparation) throw new Error("Canvas2D text resource preparation unavailable");
    setPreparationFont(preparation, font);
    const metrics = preparation.measureText(text);
    const descent = Number.isFinite(metrics.actualBoundingBoxDescent)
      ? Math.max(0, metrics.actualBoundingBoxDescent) : Math.max(0, font.size - font.baseline);
    const width = Math.max(1, Math.ceil(metrics.width));
    const height = Math.max(1, Math.ceil(font.baseline + descent));
    surface.width = width;
    surface.height = height;
    setPreparationFont(preparation, font);
    preparation.clearRect(0, 0, width, height);
    preparation.fillStyle = "white";
    preparation.fillText(text, 0, font.baseline);
    const resource = {
      ready: true, drawable: surface, width, height, generation: 1,
      baseline: font.baseline, text, fontHandle, byteLength: width * height * 4,
      transient: false
    };
    if (resource.byteLength > PREPARED_TEXT_MAX_BYTES) {
      resource.transient = true;
      return resource;
    }
    preparedText.set(key, resource);
    preparedTextBytes += resource.byteLength;
    for (const [candidateKey, candidate] of preparedText) {
      if (preparedText.size <= PREPARED_TEXT_MAX_ENTRIES
          && preparedTextBytes <= PREPARED_TEXT_MAX_BYTES) break;
      // The new resource is prepared and drawn by the caller immediately.
      if (candidate === resource) continue;
      preparedText.delete(candidateKey);
      preparedTextBytes = Math.max(0, preparedTextBytes - candidate.byteLength);
      gpuBatcher?.releaseResource(candidate);
    }
    return resource;
  };
  const drawPreparedText = (fontHandle, text, x, y, red, green, blue, alpha) => {
    if (!text) return;
    const renderer = getGpuBatcher();
    if (!renderer) return;
    const resource = preparedTextResource(fontHandle, text);
    try {
      const entry = renderer.atlasFor(resource, null);
      if (!entry) throw new Error("WebGL2 text atlas allocation failed");
      writeQuad(0, x, y, resource.width, resource.height, {
        u0: entry.x / entry.page.size, v0: entry.y / entry.page.size,
        u1: (entry.x + entry.width) / entry.page.size,
        v1: (entry.y + entry.height) / entry.page.size
      }, red, green, blue, alpha);
      renderer.drawSprites(spriteScratch, 1, entry.page);
      performanceWorkload.instances += 1;
      performanceWorkload.batches += 1;
      performanceWorkload.drawCalls += 1;
      performanceWorkload.uploadedBytes += 16 * Float32Array.BYTES_PER_ELEMENT;
    } finally {
      if (resource.transient) renderer.releaseResource(resource);
    }
  };
  function executeCommands() {
    performanceWorkload.commands += commands.length;
    for (const command of commands) {
      if (command[0] === 0) {
        getGpuBatcher()?.beginFrame((command[1] & 255) / 255,
          (command[2] & 255) / 255, (command[3] & 255) / 255, 1);
      } else if (command[0] === 1) {
        drawImmediateSolid(command[1], command[2], command[3], command[4],
          (command[5] & 255) / 255, (command[6] & 255) / 255, (command[7] & 255) / 255);
      } else if (command[0] === 2) {
        // Legacy direct text commands are prepared once per distinct value and
        // submitted through the same texture path as canonical text below.
        drawPreparedText(0, `score ${command[3]}`, command[1], command[2], 0.875, 0.965, 1, 1);
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
    if (i32[GFX_I_MAGIC] !== GFX_CMD_MAGIC) return;
    const version = i32[GFX_I_VERSION];
    if (version !== GFX_CMD_VERSION) return;
    const publishedSprites = i32[GFX_I_SPRITE_COUNT];
    const publishedRuns = i32[GFX_I_SPRITE_RUN_COUNT];
    if (publishedSprites < 0 || publishedSprites > GFX_MAX_SPRITES
        || publishedRuns < 0 || publishedRuns > GFX_MAX_SPRITE_RUNS) return;
    for (let run = 0; run < publishedRuns; run += 1) {
      const base = GFX_I_SPRITE_RUN_BASE + run * GFX_SPRITE_RUN_STRIDE_I32;
      const first = i32[base];
      const count = i32[base + 1];
      if (first < 0 || count <= 0 || first + count > publishedSprites
          || i32[base + 2] < -1 || i32[base + 3] !== 0 || i32[base + 4] !== 0
          || i32[base + 5] !== 0 || i32[base + 6] !== 0 || i32[base + 7] !== 0) return;
    }
    for (let sprite = 0; sprite < publishedSprites; sprite += 1) {
      const baseI = GFX_I_SPRITE_BASE + sprite * GFX_SPRITE_STRIDE_I32;
      const baseF = GFX_F_SPRITE_BASE + sprite * GFX_SPRITE_STRIDE_F32;
      if (i32[baseI] === 0 || i32[baseI + 2] !== 0) return;
      for (let field = 0; field < GFX_SPRITE_STRIDE_F32; field += 1) {
        if (!Number.isFinite(f32[baseF + field])) return;
      }
      if (f32[baseF + 2] <= 0 || f32[baseF + 3] <= 0 || f32[baseF + 4] < 0
          || f32[baseF + 5] < 0 || f32[baseF + 6] < 0 || f32[baseF + 7] < 0
          || ((f32[baseF + 6] === 0) !== (f32[baseF + 7] === 0))
          || f32[baseF + 10] === 0 || f32[baseF + 11] === 0) return;
    }
    const spriteStride = GFX_SPRITE_STRIDE_F32;
    const textBase = GFX_F_TEXT_BASE;
    const batcher = getGpuBatcher();
    if (!batcher) return;
    const flags = i32[GFX_I_FLAGS];
    if (flags & GFX_FLAG_CLEAR) {
      batcher.beginFrame(f32[GFX_F_CLEAR_BASE], f32[GFX_F_CLEAR_BASE + 1],
        f32[GFX_F_CLEAR_BASE + 2], Math.max(0, Math.min(1, f32[GFX_F_CLEAR_BASE + 3])));
    }
    const drawLine = index => {
      performanceWorkload.lines += 1;
      const base = GFX_F_LINE_BASE + index * GFX_LINE_STRIDE_F32;
      const dx = f32[base + 2] - f32[base];
      const dy = f32[base + 3] - f32[base + 1];
      const length = Math.hypot(dx, dy);
      if (length <= 0) return;
      drawImmediateSolid(f32[base], f32[base + 1] - 0.5, length, 1,
        f32[base + 4], f32[base + 5], f32[base + 6], f32[base + 7],
        Math.atan2(dy, dx), 0, 0.5);
    };
    const drawRectRun = (start, count, ordered) => {
      performanceWorkload.rectangles += count;
      const solid = batcher.solidFor();
      if (!solid) throw new Error("WebGL2 solid atlas allocation failed");
      for (let first = 0; first < count; first += SPRITE_CAP) {
        const chunk = Math.min(SPRITE_CAP, count - first);
        for (let offset = 0; offset < chunk; offset += 1) {
          const index = ordered
            ? i32[GFX_I_ORDER_BASE + start + first + offset] % GFX_ORDER_KIND_SCALE
            : start + first + offset;
          const source = GFX_F_RECT_REVERSE_BASE - index * GFX_GEOMETRY_STRIDE_F32;
          writeQuad(offset * 16, f32[source], f32[source + 1], f32[source + 2], f32[source + 3],
            solid, f32[source + 4], f32[source + 5], f32[source + 6], f32[source + 7]);
        }
        batcher.drawSprites(spriteScratch, chunk, solid.page);
        performanceWorkload.batches += 1;
        performanceWorkload.drawCalls += 1;
        performanceWorkload.uploadedBytes += chunk * 16 * Float32Array.BYTES_PER_ELEMENT;
      }
      performanceWorkload.instances += count;
    };
    const spriteInfo = index => {
      const baseI = GFX_I_SPRITE_BASE + index * GFX_SPRITE_STRIDE_I32;
      const baseF = GFX_F_SPRITE_BASE + index * spriteStride;
      let resource = sprites.get(i32[baseI]);
      if (!resource?.ready || !resource.drawable || !resource.width || !resource.height) {
        resource = deterministicMissingSprite();
      }
      const x = f32[baseF];
      const y = f32[baseF + 1];
      const width = f32[baseF + 2];
      const height = f32[baseF + 3];
      const cropRequested = !resource.missing && (f32[baseF + 6] !== 0 || f32[baseF + 7] !== 0);
      const variant = resource.missing
        ? { key: "missing", drawable: resource.drawable, width: 2, height: 2 }
        : spriteVariantFor(resource, cropRequested);
      const logicalWidth = resource.width;
      const logicalHeight = resource.height;
      const logicalX = cropRequested ? f32[baseF + 4] : 0;
      const logicalY = cropRequested ? f32[baseF + 5] : 0;
      const logicalCropWidth = cropRequested ? f32[baseF + 6] : logicalWidth;
      const logicalCropHeight = cropRequested ? f32[baseF + 7] : logicalHeight;
      if (logicalX < 0 || logicalY < 0 || logicalCropWidth <= 0 || logicalCropHeight <= 0
          || logicalX + logicalCropWidth > logicalWidth
          || logicalY + logicalCropHeight > logicalHeight) return null;
      const u0 = logicalX / logicalWidth;
      const v0 = logicalY / logicalHeight;
      const u1 = (logicalX + logicalCropWidth) / logicalWidth;
      const v1 = (logicalY + logicalCropHeight) / logicalHeight;
      const tint = i32[baseI + 1] >>> 0;
      return { handle: i32[baseI], resource, variant, x, y, width, height,
        u0, v0, u1, v1,
        pivotX: f32[baseF + 8], pivotY: f32[baseF + 9],
        scaleX: f32[baseF + 10], scaleY: f32[baseF + 11],
        red: ((tint >>> 24) & 255) / 255, green: ((tint >>> 16) & 255) / 255,
        blue: ((tint >>> 8) & 255) / 255, alpha: (tint & 255) / 255,
        radians: f32[baseF + 12] * Math.PI / 180 };
    };
    const drawSprite = index => {
      performanceWorkload.sprites += 1;
      const info = spriteInfo(index);
      if (!info) return;
      const atlas = batcher.atlasFor(info.resource, info.variant);
      if (!atlas) throw new Error("WebGL2 sprite atlas allocation failed");
      writeQuad(0,
        info.x + info.pivotX - info.pivotX * info.scaleX,
        info.y + info.pivotY - info.pivotY * info.scaleY,
        info.width * info.scaleX, info.height * info.scaleY, {
          u0: (atlas.x + info.u0 * atlas.width) / atlas.page.size,
          v0: (atlas.y + info.v0 * atlas.height) / atlas.page.size,
          u1: (atlas.x + info.u1 * atlas.width) / atlas.page.size,
          v1: (atlas.y + info.v1 * atlas.height) / atlas.page.size
        }, info.red, info.green, info.blue, info.alpha, info.radians,
        info.pivotX * info.scaleX, info.pivotY * info.scaleY);
      batcher.drawSprites(spriteScratch, 1, atlas.page);
      performanceWorkload.instances += 1;
      performanceWorkload.batches += 1;
      performanceWorkload.drawCalls += 1;
      performanceWorkload.uploadedBytes += 16 * Float32Array.BYTES_PER_ELEMENT;
    };
    const drawSpriteRun = (start, count) => {
      let offset = 0;
      while (offset < count) {
        let batchCount = 0;
        let page = null;
        while (offset + batchCount < count && batchCount < SPRITE_CAP) {
          const value = spriteInfo(start + offset + batchCount);
          if (!value) { if (batchCount === 0) offset += 1; break; }
          const atlas = batcher.atlasFor(value.resource, value.variant);
          if (!atlas) throw new Error("WebGL2 sprite atlas allocation failed");
          if (!atlas || (page && atlas.page !== page)) break;
          page ||= atlas.page;
          writeQuad(batchCount * 16,
            value.x + value.pivotX - value.pivotX * value.scaleX,
            value.y + value.pivotY - value.pivotY * value.scaleY,
            value.width * value.scaleX, value.height * value.scaleY, {
              u0: (atlas.x + value.u0 * atlas.width) / atlas.page.size,
              v0: (atlas.y + value.v0 * atlas.height) / atlas.page.size,
              u1: (atlas.x + value.u1 * atlas.width) / atlas.page.size,
              v1: (atlas.y + value.v1 * atlas.height) / atlas.page.size
            }, value.red, value.green, value.blue, value.alpha, value.radians,
            value.pivotX * value.scaleX, value.pivotY * value.scaleY);
          batchCount += 1;
        }
        if (batchCount > 0) {
          batcher.drawSprites(spriteScratch, batchCount, page);
          performanceWorkload.instances += batchCount;
          performanceWorkload.batches += 1;
          performanceWorkload.drawCalls += 1;
          performanceWorkload.sprites += batchCount;
          performanceWorkload.uploadedBytes += batchCount * 16 * Float32Array.BYTES_PER_ELEMENT;
        }
        offset += batchCount;
      }
    };
    // Decode adjacent semantic rectangles and sprite runs into one private
    // ordered 64-byte quad stream. Rectangles sample a host-owned white atlas
    // texel; no synthetic handle or physical page enters guest-visible data.
    const drawMixedOrderRun = (firstOrder, orderLength) => {
      let itemCount = 0;
      for (let offset = 0; offset < orderLength; offset += 1) {
        const entry = i32[GFX_I_ORDER_BASE + firstOrder + offset];
        const kind = Math.floor(entry / GFX_ORDER_KIND_SCALE);
        const index = entry % GFX_ORDER_KIND_SCALE;
        if (kind === GFX_ORDER_RECT) itemCount += 1;
        else {
          const runBase = GFX_I_SPRITE_RUN_BASE + index * GFX_SPRITE_RUN_STRIDE_I32;
          itemCount += i32[runBase + 1];
        }
      }
      if (itemCount < 2) return false;
      let batchCount = 0;
      let batchRects = 0;
      let batchSprites = 0;
      let page = null;
      let executionDomain = null;
      const flush = () => {
        if (batchCount === 0) return;
        batcher.drawSprites(spriteScratch, batchCount, page);
        performanceWorkload.instances += batchCount;
        performanceWorkload.rectangles += batchRects;
        performanceWorkload.sprites += batchSprites;
        performanceWorkload.batches += 1;
        performanceWorkload.drawCalls += 1;
        performanceWorkload.uploadedBytes += batchCount * 16 * Float32Array.BYTES_PER_ELEMENT;
        batchCount = 0;
        batchRects = 0;
        batchSprites = 0;
        page = null;
        executionDomain = null;
      };
      const emit = (kind, index, domain, preferredPage = null) => {
        if (kind !== GFX_ORDER_RECT && executionDomain !== null && executionDomain !== domain) flush();
        if (kind !== GFX_ORDER_RECT) executionDomain = domain;
        let atlas;
        let value;
        if (kind === GFX_ORDER_RECT) {
          atlas = batcher.solidFor(page || preferredPage);
        } else {
          value = spriteInfo(index);
          if (!value) {
            flush();
            return;
          }
          atlas = batcher.atlasFor(value.resource, value.variant);
        }
        if (!atlas) {
          throw new Error("WebGL2 atlas allocation failed");
        }
        if (page && atlas.page !== page) flush();
        if (batchCount >= SPRITE_CAP) flush();
        executionDomain = domain;
        page = atlas.page;
        const target = batchCount * 16;
        if (kind === GFX_ORDER_RECT) {
          const source = GFX_F_RECT_REVERSE_BASE - index * GFX_GEOMETRY_STRIDE_F32;
          spriteScratch[target] = f32[source];
          spriteScratch[target + 1] = f32[source + 1];
          spriteScratch[target + 2] = f32[source + 2];
          spriteScratch[target + 3] = f32[source + 3];
          for (let uv = 4; uv < 8; uv += 1) spriteScratch[target + uv] = atlas.uv;
          spriteScratch[target + 8] = f32[source + 4];
          spriteScratch[target + 9] = f32[source + 5];
          spriteScratch[target + 10] = f32[source + 6];
          spriteScratch[target + 11] = f32[source + 7];
          spriteScratch[target + 12] = 0;
          spriteScratch[target + 13] = 1;
          spriteScratch[target + 14] = f32[source + 2] * 0.5;
          spriteScratch[target + 15] = f32[source + 3] * 0.5;
          batchRects += 1;
        } else {
          spriteScratch[target] = value.x + value.pivotX - value.pivotX * value.scaleX;
          spriteScratch[target + 1] = value.y + value.pivotY - value.pivotY * value.scaleY;
          spriteScratch[target + 2] = value.width * value.scaleX;
          spriteScratch[target + 3] = value.height * value.scaleY;
          spriteScratch[target + 4] = (atlas.x + value.u0 * atlas.width) / atlas.page.size;
          spriteScratch[target + 5] = (atlas.y + value.v0 * atlas.height) / atlas.page.size;
          spriteScratch[target + 6] = (atlas.x + value.u1 * atlas.width) / atlas.page.size;
          spriteScratch[target + 7] = (atlas.y + value.v1 * atlas.height) / atlas.page.size;
          spriteScratch[target + 8] = value.red;
          spriteScratch[target + 9] = value.green;
          spriteScratch[target + 10] = value.blue;
          spriteScratch[target + 11] = value.alpha;
          spriteScratch[target + 12] = Math.sin(value.radians);
          spriteScratch[target + 13] = Math.cos(value.radians);
          spriteScratch[target + 14] = value.pivotX * value.scaleX;
          spriteScratch[target + 15] = value.pivotY * value.scaleY;
          batchSprites += 1;
        }
        batchCount += 1;
      };
      try {
        for (let offset = 0; offset < orderLength; offset += 1) {
          const entry = i32[GFX_I_ORDER_BASE + firstOrder + offset];
          const kind = Math.floor(entry / GFX_ORDER_KIND_SCALE);
          const index = entry % GFX_ORDER_KIND_SCALE;
          if (kind === GFX_ORDER_RECT) {
            let preferredPage = null;
            if (!page) {
              for (let lookahead = offset + 1; lookahead < orderLength; lookahead += 1) {
                const future = i32[GFX_I_ORDER_BASE + firstOrder + lookahead];
                if (Math.floor(future / GFX_ORDER_KIND_SCALE) !== GFX_ORDER_SPRITE) continue;
                const futureRun = GFX_I_SPRITE_RUN_BASE
                  + (future % GFX_ORDER_KIND_SCALE) * GFX_SPRITE_RUN_STRIDE_I32;
                const futureValue = spriteInfo(i32[futureRun]);
                if (futureValue) preferredPage = batcher.atlasFor(futureValue.resource, futureValue.variant)?.page;
                break;
              }
            }
            emit(kind, index, executionDomain, preferredPage);
          } else {
            const runBase = GFX_I_SPRITE_RUN_BASE + index * GFX_SPRITE_RUN_STRIDE_I32;
            const first = i32[runBase];
            const count = i32[runBase + 1];
            const domain = `${i32[runBase + 2]}|${i32[runBase + 3]}|${i32[runBase + 4]}|${i32[runBase + 5]}|${i32[runBase + 6]}|${i32[runBase + 7]}`;
            for (let item = 0; item < count; item += 1) emit(kind, first + item, domain);
          }
        }
        flush();
        return true;
      } catch (error) {
        document.body.dataset.gpuError = String(error);
        throw error;
      }
    };
    const drawText = index => {
      performanceWorkload.text += 1;
      const baseI = GFX_I_TEXT_BASE + index * GFX_TEXT_STRIDE_I32;
      const baseF = textBase + index * GFX_TEXT_STRIDE_F32;
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
      drawPreparedText(fontHandle, text, f32[baseF], f32[baseF + 1],
        f32[baseF + 2], f32[baseF + 3], f32[baseF + 4], f32[baseF + 5]);
    };
    const lineCount = Math.max(0, Math.min(i32[GFX_I_LINE_COUNT], GFX_MAX_LINES));
    const spriteCount = Math.max(0, Math.min(i32[GFX_I_SPRITE_COUNT], GFX_MAX_SPRITES));
    const spriteRunCount = Math.max(0, Math.min(i32[GFX_I_SPRITE_RUN_COUNT], GFX_MAX_SPRITE_RUNS));
    const textCount = Math.max(0, Math.min(i32[GFX_I_TEXT_COUNT], GFX_MAX_TEXT));
    const rectCount = Math.max(0, Math.min(i32[GFX_I_RECT_COUNT], GFX_MAX_GEOMETRY - lineCount));
    const orderCount = Math.max(0, Math.min(i32[GFX_I_ORDER_COUNT], GFX_MAX_ORDER));
    const clipCount = Math.max(0, Math.min(i32[GFX_I_CLIP_COUNT], GFX_MAX_CLIPS));
    const clipStack = [];
    const pushClip = index => {
      if (index < 0 || index >= clipCount) return;
      const base = GFX_F_CLIP_BASE + index * GFX_CLIP_STRIDE_F32;
      let clip = { x: f32[base], y: f32[base + 1], width: f32[base + 2], height: f32[base + 3] };
      const parent = clipStack[clipStack.length - 1];
      if (parent) {
        const x = Math.max(parent.x, clip.x);
        const y = Math.max(parent.y, clip.y);
        const right = Math.min(parent.x + parent.width, clip.x + clip.width);
        const bottom = Math.min(parent.y + parent.height, clip.y + clip.height);
        clip = { x, y, width: Math.max(0, right - x), height: Math.max(0, bottom - y) };
      }
      clipStack.push(clip);
      batcher.setClip(clip);
    };
    const popClip = () => {
      if (clipStack.length === 0) return;
      clipStack.pop();
      batcher.setClip(clipStack[clipStack.length - 1] || null);
    };
    performanceWorkload.commands += lineCount + rectCount + spriteCount + textCount;
    if (orderCount > 0) {
      for (let order = 0; order < orderCount; order += 1) {
        const encoded = i32[GFX_I_ORDER_BASE + order];
        const kind = Math.floor(encoded / GFX_ORDER_KIND_SCALE);
        const index = encoded % GFX_ORDER_KIND_SCALE;
        if (kind === GFX_ORDER_RECT || kind === GFX_ORDER_SPRITE) {
          let mixedLength = 1;
          while (order + mixedLength < orderCount) {
            const next = i32[GFX_I_ORDER_BASE + order + mixedLength];
            const nextKind = Math.floor(next / GFX_ORDER_KIND_SCALE);
            if (nextKind !== GFX_ORDER_RECT && nextKind !== GFX_ORDER_SPRITE) break;
            mixedLength += 1;
          }
          if (drawMixedOrderRun(order, mixedLength)) {
            order += mixedLength - 1;
            continue;
          }
        }
        if (kind === GFX_ORDER_CLIP_PUSH) pushClip(index);
        else if (kind === GFX_ORDER_CLIP_POP && index === 0) popClip();
        else if (kind === GFX_ORDER_LINE && index < lineCount) drawLine(index);
        else if (kind === GFX_ORDER_SPRITE && index < spriteRunCount) {
          const runBase = GFX_I_SPRITE_RUN_BASE + index * GFX_SPRITE_RUN_STRIDE_I32;
          drawSpriteRun(i32[runBase], i32[runBase + 1]);
        }
        else if (kind === GFX_ORDER_TEXT && index < textCount) drawText(index);
        else if (kind === GFX_ORDER_RECT && index < rectCount) {
          let runCount = 1;
          while (order + runCount < orderCount) {
            const next = i32[GFX_I_ORDER_BASE + order + runCount];
            if (Math.floor(next / GFX_ORDER_KIND_SCALE) !== GFX_ORDER_RECT
                || next % GFX_ORDER_KIND_SCALE >= rectCount) break;
            runCount += 1;
          }
          drawRectRun(order, runCount, true);
          order += runCount - 1;
        }
      }
    } else {
      for (let index = 0; index < lineCount; index += 1) drawLine(index);
      drawRectRun(0, rectCount, false);
      for (let run = 0; run < spriteRunCount; run += 1) {
        const runBase = GFX_I_SPRITE_RUN_BASE + run * GFX_SPRITE_RUN_STRIDE_I32;
        drawSpriteRun(i32[runBase], i32[runBase + 1]);
      }
      for (let index = 0; index < textCount; index += 1) drawText(index);
    }
    batcher.setClip(null);
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
    syncDisplayState();
    const iLayout = game.memory.host_i32;
    const fLayout = game.memory.host_f32;
    if (!iLayout || !fLayout || !instance.exports.memory) {
      resizeGenerationPending = false;
      return;
    }
    const i32 = new Int32Array(instance.exports.memory.buffer, iLayout.offset, iLayout.length);
    const f32 = new Float32Array(instance.exports.memory.buffer, fLayout.offset, fLayout.length);
    i32.fill(0);
    f32.fill(0);
    const elapsedMs = timestamp - startedAt;
    const focused = document.hasFocus() ? 1 : 0;
    const pointerCount = pointer.hover || pointer.down || pointer.wentDown || pointer.wentUp ? 1 : 0;
    i32[0] = Math.floor(elapsedMs) | 0;
    i32[7] = pointerCount;
    i32[8] = 0;
    i32[9] = 0;
    i32[10] = tickIndex++;
    i32[11] = resized ? 1 : 0;
    i32[12] = hostDesktopDimension(globalThis.screen?.width, display.cssWidth);
    i32[13] = hostDesktopDimension(globalThis.screen?.height, display.cssHeight);
    i32[14] = 4;
    i32[15] = (focused ? 2 : 0) | (document.hidden ? 4 : 0) | (resized ? 8 : 0);
    i32[16] = 0;
    i32[17] = focused;
    i32[18] = document.hidden ? 1 : 0;
    i32[19] = Math.floor(elapsedMs * 1000) | 0;
    i32[20] = Math.round(display.logicalWidth);
    i32[21] = Math.round(display.logicalHeight);
    i32[22] = Math.round(display.cssWidth);
    i32[23] = Math.round(display.cssHeight);
    i32[24] = display.backingWidth;
    i32[25] = display.backingHeight;
    i32[26] = 0;
    i32[27] = 0;
    i32[28] = Math.round(display.logicalWidth);
    i32[29] = Math.round(display.logicalHeight);
    i32[30] = display.displayGeneration;
    i32[31] = display.densityGeneration;
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
      f32[4] = display.logicalWidth ? pointer.x / display.logicalWidth : 0;
      f32[5] = display.logicalHeight ? pointer.y / display.logicalHeight : 0;
    }
    f32[48] = display.contentScale;
    f32[49] = display.rasterScale;
    f32[50] = display.logicalWidth;
    f32[51] = display.logicalHeight;
    f32[52] = 0;
    f32[53] = 0;
    f32[54] = display.logicalWidth;
    f32[55] = display.logicalHeight;
    f32[56] = display.availableWidth;
    f32[57] = display.availableHeight;
    f32[58] = display.effectiveDpr;
    f32[59] = display.scaleX;
    f32[60] = display.scaleY;
    document.body.dataset.hostTick = String(i32[10]);
    document.body.dataset.hostTimeMs = String(i32[0]);
    if (resized) document.body.dataset.resizeTick = String(i32[10]);
    resized = false;
    resizeGenerationPending = false;
  }

  function finishHostFrame() {
    clearExternalActionGesture();
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
    setLogicalCanvas(width, height);
  }

  function markResized() {
    requestDisplaySync();
  }

  function applyWindowRequest() {
    const sequence = exportedI32("host_req_seq");
    if (sequence === undefined || sequence === lastWindowRequest) return;
    lastWindowRequest = sequence;
    const flags = exportedI32("host_req_flags") || 0;
    const width = exportedI32("host_req_window_w_px") || display.logicalWidth;
    const height = exportedI32("host_req_window_h_px") || display.logicalHeight;
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
    if (!getGpuBatcher()) {
      // Context loss suspends publication. The restore event makes the same
      // visible WebGL2 renderer recreatable; there is no alternate backend.
      requestAnimationFrame(frame);
      return;
    }
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
    performanceWorkload.composites = 0;
    performanceWorkload.renderSubmissions = 0;
    performanceWorkload.uploadedBytes = 0;
    performanceWorkload.textureBinds = 0;
    performanceWorkload.atlasTransitions = 0;
    performanceWorkload.pipelineBoundaries = 0;
    performanceBackend = "WebGL2";
    gpuBatcher.resetFrameMetrics();
    const replayStart = performance.now();
    try {
      executeCommands();
    } catch (error) {
      document.body.dataset.gpuError = String(error);
      requestAnimationFrame(frame);
      return;
    }
    const browserReplayMs = performance.now() - replayStart;
    const atlasMetrics = gpuBatcher?.metrics?.();
    performanceWorkload.atlasPages = atlasMetrics?.pages ?? -1;
    performanceWorkload.atlasLiveEntries = atlasMetrics?.liveEntries ?? -1;
    performanceWorkload.atlasAllocatedBytes = atlasMetrics?.allocatedBytes ?? -1;
    performanceWorkload.atlasUploadCount = atlasMetrics?.uploadCount ?? -1;
    performanceWorkload.atlasUploadBytes = atlasMetrics?.uploadBytes ?? -1;
    performanceWorkload.textureBinds = atlasMetrics?.frameTextureBinds ?? 0;
    performanceWorkload.atlasTransitions = atlasMetrics?.frameAtlasTransitions ?? 0;
    publishDisplayReceipt();
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
    performanceWorkload.renderSubmissions = performanceWorkload.drawCalls + performanceWorkload.composites;
    const underBudget = frameWorkMs <= 16.67;
    if (hud) {
      const uploadText = performanceWorkload.uploadedBytes > 0 ? ` · uploaded ${performanceWorkload.uploadedBytes} B` : "";
      const instanceText = ` · instances ${performanceWorkload.instances} · batches ${performanceWorkload.batches}`;
      const atlasText = performanceWorkload.atlasPages >= 0
        ? ` · atlas ${performanceWorkload.atlasPages} pages/${performanceWorkload.atlasLiveEntries} live · ${performanceWorkload.atlasAllocatedBytes} B · uploads total ${performanceWorkload.atlasUploadCount}` : "";
      hud.textContent = `${performanceBackend} · frame ${frames}\ntick ${tickMs.toFixed(3)} ms (worst ${worstTick.toFixed(3)}) · guest render ${wasmRenderMs.toFixed(3)} ms (worst ${worstWasmRender.toFixed(3)})\nhost replay ${browserReplayMs.toFixed(3)} ms (worst ${worstBrowserReplay.toFixed(3)})\nframe work ${frameWorkMs.toFixed(3)} ms (worst ${worstFrameWork.toFixed(3)}) · ${underBudget ? "UNDER 16.67 ms" : "OVER 16.67 ms"}\ncommands ${performanceWorkload.commands} · lines ${performanceWorkload.lines} · rects ${performanceWorkload.rectangles} · sprites ${performanceWorkload.sprites} · text ${performanceWorkload.text}\ndraws ${performanceWorkload.drawCalls} · composites ${performanceWorkload.composites} · submissions ${performanceWorkload.renderSubmissions} · binds ${performanceWorkload.textureBinds} · atlas transitions ${performanceWorkload.atlasTransitions}${instanceText}${uploadText}${atlasText}`;
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
    document.body.dataset.instances = String(performanceWorkload.instances);
    document.body.dataset.batches = String(performanceWorkload.batches);
    document.body.dataset.drawCalls = String(performanceWorkload.drawCalls);
    document.body.dataset.composites = String(performanceWorkload.composites);
    document.body.dataset.renderSubmissions = String(performanceWorkload.renderSubmissions);
    document.body.dataset.uploadedBytes = String(performanceWorkload.uploadedBytes);
    document.body.dataset.textureBinds = String(performanceWorkload.textureBinds);
    document.body.dataset.atlasTransitions = String(performanceWorkload.atlasTransitions);
    document.body.dataset.pipelineBoundaries = String(performanceWorkload.pipelineBoundaries);
    document.body.dataset.atlasPages = String(performanceWorkload.atlasPages);
    document.body.dataset.atlasLiveEntries = String(performanceWorkload.atlasLiveEntries);
    document.body.dataset.atlasAllocatedBytes = String(performanceWorkload.atlasAllocatedBytes);
    document.body.dataset.atlasUploadCount = String(performanceWorkload.atlasUploadCount);
    document.body.dataset.atlasUploadBytes = String(performanceWorkload.atlasUploadBytes);
    document.body.dataset.preparedTextEntries = String(preparedText.size);
    document.body.dataset.preparedTextBytes = String(preparedTextBytes);
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
    const width = Math.max(1, finitePositive(bounds.width, display.cssWidth));
    const height = Math.max(1, finitePositive(bounds.height, display.cssHeight));
    const right = Number.isFinite(bounds.right) ? bounds.right : bounds.left + width;
    const bottom = Number.isFinite(bounds.bottom) ? bounds.bottom : bounds.top + height;
    const x = Math.round(Math.max(0, Math.min(display.logicalWidth,
      (event.clientX - bounds.left) * display.logicalWidth / width)));
    const y = Math.round(Math.max(0, Math.min(display.logicalHeight,
      (event.clientY - bounds.top) * display.logicalHeight / height)));
    const inside = event.clientX >= bounds.left && event.clientX <= right
      && event.clientY >= bounds.top && event.clientY <= bottom;
    pointer.dx += x - pointer.x;
    pointer.dy += y - pointer.y;
    pointer.x = x;
    pointer.y = y;
    pointer.id = event.pointerId | 0;
    pointer.hover = event.pointerType !== "touch" && inside;
  }
  addEventListener("keydown", event => {
    if (!event.repeat && !keys.has(event.code)) markExternalActionGesture();
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
    if (!pointer.down) markExternalActionGesture();
    pointer.down = true;
    pointer.wentDown = true;
    canvas.setPointerCapture(event.pointerId);
    void applyFullscreenGesture();
  });
  canvas.addEventListener("pointerleave", () => { pointer.hover = false; });
  canvas.addEventListener("pointerup", event => {
    updatePointer(event);
    pointer.down = false;
    pointer.wentUp = true;
  });
  canvas.addEventListener("pointercancel", () => { pointer.hover = false; pointer.down = false; pointer.wentUp = true; });
  addEventListener("blur", clearExternalActionGesture);
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) clearExternalActionGesture();
  });
  addEventListener("resize", markResized);
  addEventListener("orientationchange", markResized);
  if (window.visualViewport) window.visualViewport.addEventListener("resize", markResized);
  addEventListener("stasis-viewport-extent", markResized);
  document.addEventListener("fullscreenchange", markResized);
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
      if (!getGpuBatcher()) throw new Error("WebGL2 is required by the Stasis Web renderer");
      const result = await WebAssembly.instantiate(await wasmBytes(), imports);
      instance = result.instance;
      writeHostFrame(performance.now());
      const mainResult = instance.exports.main();
      finishHostFrame();
      applyWindowRequest();
      await Promise.all([
        ...Array.from(sprites.values(), resource => resource.readyPromise),
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
