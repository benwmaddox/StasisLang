const wasmPath = "./brickout_revenge_v1.wasm";

const canvas = document.getElementById("canvas");
const ctx = canvas.getContext("2d", { alpha: false, desynchronized: true });
const logEl = document.getElementById("log");
const startBtn = document.getElementById("start");

function log(line) {
  console.log(line);
  logEl.textContent += line + "\n";
  logEl.scrollTop = logEl.scrollHeight;
}

function resizeCanvas(w, h) {
  const dpr = Math.max(1, Math.min(3, window.devicePixelRatio || 1));
  canvas.width = Math.max(1, Math.floor(w * dpr));
  canvas.height = Math.max(1, Math.floor(h * dpr));
  canvas.style.width = `${w}px`;
  canvas.style.height = `${h}px`;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
}

function readCString(memU8, ptr, cap = 4096) {
  ptr = ptr >>> 0;
  let s = "";
  const end = Math.min(memU8.length, ptr + cap);
  for (let i = ptr; i < end; i++) {
    const b = memU8[i];
    if (b === 0) break;
    s += String.fromCharCode(b);
  }
  return s;
}

function i32ToSigned(x) {
  x = x | 0;
  return x;
}

function scorePrintableAscii(s) {
  let score = 0;
  for (let i = 0; i < s.length; i++) {
    const c = s.charCodeAt(i);
    if (c === 9 || c === 10 || c === 13) score += 1;
    else if (c >= 32 && c <= 126) score += 2;
    else score -= 5;
  }
  return score;
}

function readMaybeStasisString(memU8, ptr) {
  const candidates = [
    readCString(memU8, ptr, 4096),
    readCString(memU8, (ptr + 4) | 0, 4096), // ascii[N] payload
    readCString(memU8, (ptr + 8) | 0, 4096), // utf8[N] payload
  ];
  let best = candidates[0];
  let bestScore = scorePrintableAscii(best);
  for (let i = 1; i < candidates.length; i++) {
    const s = candidates[i];
    const sc = scorePrintableAscii(s);
    if (sc > bestScore) {
      bestScore = sc;
      best = s;
    }
  }
  return best;
}

async function start() {
  startBtn.disabled = true;
  logEl.textContent = "";

  const wasmBytes = await fetch(wasmPath).then((r) => r.arrayBuffer());

  let exports = null;
  let memU8 = null;

  const sprites = new Map(); // path -> { id, img }
  const spriteById = new Map(); // id -> { path, img }
  let nextSpriteId = 1;

  let resizedFlag = 1;
  let canvasW = 600;
  let canvasH = 1200;
  resizeCanvas(canvasW, canvasH);

  const pointerState = new Map(); // pointerId -> { id, x, y, down, wentDown, wentUp }
  let pointerSnapshot = [];

  function snapPointers() {
    const arr = [];
    for (const p of pointerState.values()) {
      if (p.down || p.wentDown || p.wentUp) arr.push(p);
    }
    arr.sort((a, b) => a.id - b.id);
    pointerSnapshot = arr;
  }

  function clearPointerEdges() {
    for (const p of pointerState.values()) {
      p.wentDown = false;
      p.wentUp = false;
    }
    for (const [id, p] of pointerState.entries()) {
      if (!p.down) pointerState.delete(id);
    }
  }

  function canvasToGamePx(clientX, clientY) {
    const rect = canvas.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return { x: 0, y: 0 };
    const x = ((clientX - rect.left) / rect.width) * canvasW;
    const y = ((clientY - rect.top) / rect.height) * canvasH;
    return { x, y };
  }

  canvas.tabIndex = 0;
  canvas.style.touchAction = "none";

  canvas.addEventListener("pointerdown", (e) => {
    canvas.focus();
    const pos = canvasToGamePx(e.clientX, e.clientY);
    const id = e.pointerId | 0;
    let p = pointerState.get(id);
    if (!p) {
      p = { id, x: pos.x, y: pos.y, down: true, wentDown: true, wentUp: false };
      pointerState.set(id, p);
    } else {
      p.x = pos.x;
      p.y = pos.y;
      p.down = true;
      p.wentDown = true;
    }
    try {
      canvas.setPointerCapture(e.pointerId);
    } catch {
      // ignore
    }
    e.preventDefault();
  });

  canvas.addEventListener("pointermove", (e) => {
    const id = e.pointerId | 0;
    const p = pointerState.get(id);
    if (!p) return;
    const pos = canvasToGamePx(e.clientX, e.clientY);
    p.x = pos.x;
    p.y = pos.y;
    e.preventDefault();
  });

  function pointerUpLike(e) {
    const id = e.pointerId | 0;
    const p = pointerState.get(id);
    if (!p) return;
    const pos = canvasToGamePx(e.clientX, e.clientY);
    p.x = pos.x;
    p.y = pos.y;
    p.down = false;
    p.wentUp = true;
    e.preventDefault();
  }

  canvas.addEventListener("pointerup", pointerUpLike);
  canvas.addEventListener("pointercancel", pointerUpLike);

  let escapeDown = false;
  let quitRequested = false;

  window.addEventListener("keydown", (e) => {
    if (e.code === "Escape") {
      escapeDown = true;
      quitRequested = true;
      e.preventDefault();
    }
  });

  window.addEventListener("keyup", (e) => {
    if (e.code === "Escape") {
      escapeDown = false;
      e.preventDefault();
    }
  });

  function getSpriteHandleForPath(path) {
    const existing = sprites.get(path);
    if (existing) return existing.id;
    const id = nextSpriteId++;
    const img = new Image();
    img.decoding = "async";
    img.src = path.startsWith("/") ? path : `/${path}`;
    sprites.set(path, { id, img });
    spriteById.set(id, { path, img });
    return id;
  }

  const imports = {
    env: {
      memory: undefined,

      sinf: (x) => Math.sin(x),
      cosf: (x) => Math.cos(x),

      stasis_init_window: (w, h, titlePtr) => {
        canvasW = w | 0;
        canvasH = h | 0;
        resizedFlag = 1;
        resizeCanvas(canvasW, canvasH);
        const title = memU8 ? readCString(memU8, titlePtr) : "Stasis";
        document.title = title;
        log(`init_window(${canvasW}x${canvasH}, "${title}")`);
        return 1;
      },

      stasis_begin_frame: () => {
        // no-op
      },

      stasis_end_frame: () => {
        // no-op
      },

      stasis_clear: (r, g, b, a) => {
        const rr = Math.max(0, Math.min(255, Math.floor(r * 255)));
        const gg = Math.max(0, Math.min(255, Math.floor(g * 255)));
        const bb = Math.max(0, Math.min(255, Math.floor(b * 255)));
        const aa = Math.max(0, Math.min(1, a));
        ctx.fillStyle = `rgba(${rr},${gg},${bb},${aa})`;
        ctx.fillRect(0, 0, canvasW, canvasH);
      },

      stasis_draw_line: (x1, y1, x2, y2, r, g, b, a) => {
        const rr = Math.max(0, Math.min(255, Math.floor(r * 255)));
        const gg = Math.max(0, Math.min(255, Math.floor(g * 255)));
        const bb = Math.max(0, Math.min(255, Math.floor(b * 255)));
        ctx.strokeStyle = `rgba(${rr},${gg},${bb},${a})`;
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.moveTo(x1, y1);
        ctx.lineTo(x2, y2);
        ctx.stroke();
      },

      stasis_gfx_load_sprite: (pathPtr, _maxW, _maxH) => {
        const path = readCString(memU8, pathPtr);
        return getSpriteHandleForPath(path);
      },

      stasis_gfx_draw_sprite: (handle, x, y, w, h, rotDeg, a) => {
        handle = handle | 0;
        const entry = spriteById.get(handle);
        if (!entry || !entry.img || !entry.img.complete) return;
        const alpha = Math.max(0, Math.min(1, (a | 0) / 255));
        const rot = ((rotDeg | 0) * Math.PI) / 180;
        ctx.save();
        ctx.translate(x | 0, y | 0);
        if (rot !== 0) ctx.rotate(rot);
        ctx.globalAlpha = alpha;
        const ww = w | 0;
        const hh = h | 0;
        ctx.drawImage(entry.img, -ww / 2, -hh / 2, ww, hh);
        ctx.restore();
      },

      stasis_gfx_poll_reload: (_handle) => 0,

      stasis_gfx_window_width: () => canvasW | 0,
      stasis_gfx_window_height: () => canvasH | 0,
      stasis_gfx_window_resized: () => {
        const v = resizedFlag;
        resizedFlag = 0;
        return v;
      },

      stasis_input_viewport_x_px: () => 0,
      stasis_input_viewport_y_px: () => 0,
      stasis_input_viewport_w_px: () => canvasW | 0,
      stasis_input_viewport_h_px: () => canvasH | 0,

      stasis_input_pointer_count: () => pointerSnapshot.length | 0,
      stasis_input_pointer_id: (index) => {
        index = index | 0;
        if (index < 0 || index >= pointerSnapshot.length) return 0;
        return pointerSnapshot[index].id | 0;
      },
      stasis_input_pointer_went_down: (index) => {
        index = index | 0;
        if (index < 0 || index >= pointerSnapshot.length) return 0;
        return pointerSnapshot[index].wentDown ? 1 : 0;
      },
      stasis_input_pointer_went_up: (index) => {
        index = index | 0;
        if (index < 0 || index >= pointerSnapshot.length) return 0;
        return pointerSnapshot[index].wentUp ? 1 : 0;
      },
      stasis_input_pointer_is_down: (index) => {
        index = index | 0;
        if (index < 0 || index >= pointerSnapshot.length) return 0;
        return pointerSnapshot[index].down ? 1 : 0;
      },
      stasis_input_pointer_x_px: (index) => {
        index = index | 0;
        if (index < 0 || index >= pointerSnapshot.length) return 0.0;
        return +pointerSnapshot[index].x;
      },
      stasis_input_pointer_y_px: (index) => {
        index = index | 0;
        if (index < 0 || index >= pointerSnapshot.length) return 0.0;
        return +pointerSnapshot[index].y;
      },

      stasis_should_quit: () => (quitRequested ? 1 : 0),
      stasis_is_key_down: (scancode) => {
        scancode = scancode | 0;
        // SDL scancode 41 is Escape.
        if (scancode === 41) return escapeDown ? 1 : 0;
        return 0;
      },

      stasis_get_time_ms: () => (performance.now() | 0),

      stasis_audio_is_available: () => 0,
      stasis_audio_get_sample_rate: () => 0,
      stasis_audio_get_queued_frames: () => 0,
      stasis_audio_push_f32_interleaved: (_samplesPtr, _frames) => 0,

      printf: (fmtPtr, arg0) => {
        const fmt = readMaybeStasisString(memU8, fmtPtr);
        const arg = arg0 | 0;

        let out = fmt;
        if (fmt.includes("%s")) {
          out = fmt.replace("%s", readMaybeStasisString(memU8, arg));
        } else if (fmt.includes("%d") || fmt.includes("%i")) {
          out = fmt.replace(/%[di]/, String(i32ToSigned(arg)));
        } else if (fmt.includes("%c")) {
          out = fmt.replace("%c", String.fromCharCode(arg & 0xff));
        } else if (fmt.includes("%%")) {
          out = fmt.replace("%%", "%");
        }

        out = out.replace(/\r?\n$/, "");
        if (out.length > 0) log(out);
        return 0;
      },
    },
  };

  const { instance } = await WebAssembly.instantiate(wasmBytes, imports);
  exports = instance.exports;
  memU8 = new Uint8Array(exports.memory.buffer);

  if (typeof exports.main !== "function" || typeof exports.tick !== "function") {
    throw new Error("wasm exports missing main/tick");
  }

  const rc = exports.main();
  log(`main() -> ${rc | 0}`);
  if ((rc | 0) !== 0) {
    startBtn.disabled = false;
    return;
  }

  const tickHz = 60;
  const tickMs = 1000 / tickHz;
  let last = performance.now();
  let acc = 0;

  function frame(now) {
    const dt = now - last;
    last = now;
    acc += dt;

    let steps = 0;
    while (acc >= tickMs && steps < 5) {
      snapPointers();
      const t = exports.tick() | 0;
      clearPointerEdges();
      quitRequested = false;
      if (t !== 0) {
        log(`tick() -> ${t} (stop)`);
        startBtn.disabled = false;
        return;
      }
      acc -= tickMs;
      steps++;
    }

    requestAnimationFrame(frame);
  }

  requestAnimationFrame(frame);
}

startBtn.addEventListener("click", () => {
  start().catch((e) => {
    console.error(e);
    log(`error: ${e && e.stack ? e.stack : String(e)}`);
    startBtn.disabled = false;
  });
});
