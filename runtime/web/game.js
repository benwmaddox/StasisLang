(() => {
  "use strict";
  const canvas = document.getElementById("stasis-canvas");
  const context = canvas.getContext("2d", { alpha: false });
  const hud = document.getElementById("stasis-hud");
  const audioButton = document.getElementById("stasis-audio");
  const errorBox = document.getElementById("stasis-error");
  const keys = new Set();
  const pointer = { x: 0, down: false };
  const commands = [];
  const game = window.STASIS_GAME || { strings: {}, memory: {}, assets: {} };
  const sprites = new Map();
  const fonts = new Map();
  const cachedText = new Map();
  let nextHandle = 1;
  let instance;
  let audioContext;
  let audioEvents = 0;
  const audioAssets = new Map();
  const audioVoices = new Map();
  const pendingAudio = [];
  let frames = 0;
  let worstTick = 0;
  let worstRender = 0;

  const color = (r, g, b) => `rgb(${r & 255} ${g & 255} ${b & 255})`;
  const stringValue = id => game.strings[String(id)] || "";
  const assetValue = id => game.assets[stringValue(id)] || stringValue(id);
  const loadSprite = pathId => {
    const handle = nextHandle++;
    const image = new Image();
    image.src = assetValue(pathId);
    sprites.set(handle, image);
    return handle;
  };
  const loadFont = (pathId, size) => {
    const handle = nextHandle++;
    const family = `stasis-font-${handle}`;
    const font = new FontFace(family, `url(${assetValue(pathId)})`);
    fonts.set(handle, { family, size });
    font.load().then(loaded => document.fonts.add(loaded)).catch(error => console.error(error));
    return handle;
  };
  const ensureAudio = () => {
    audioContext ||= new AudioContext();
    return audioContext;
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
  const startAudio = (handle, loop, volume) => {
    const start = async () => {
      const audio = ensureAudio();
      const buffer = await audioAssets.get(handle);
      if (!buffer) return false;
      const source = audio.createBufferSource();
      const gain = audio.createGain();
      source.buffer = buffer;
      source.loop = Boolean(loop);
      gain.gain.value = Math.max(0, Math.min(1, volume));
      source.connect(gain).connect(audio.destination);
      source.start();
      audioVoices.set(handle, { source, gain, paused: false });
      source.addEventListener("ended", () => {
        if (audioVoices.get(handle)?.source === source) audioVoices.delete(handle);
      });
      audioEvents += 1;
      document.body.dataset.audioEvents = String(audioEvents);
      return true;
    };
    if (!audioContext || audioContext.state !== "running") {
      pendingAudio.push(start);
      return true;
    }
    void start();
    return true;
  };
  const stopAudio = handle => {
    const voice = audioVoices.get(handle);
    if (voice) voice.source.stop();
    audioVoices.delete(handle);
  };
  const imports = { env: {
    web_input_axis: () => (keys.has("ArrowRight") || keys.has("KeyD") ? 1 : 0) - (keys.has("ArrowLeft") || keys.has("KeyA") ? 1 : 0),
    web_input_fire: () => keys.has("Space") || pointer.down ? 1 : 0,
    web_pointer_x: () => pointer.x | 0,
    web_pointer_down: () => pointer.down ? 1 : 0,
    web_begin_frame: (r, g, b) => { commands.length = 0; commands.push([0, r, g, b]); },
    web_draw_rect: (x, y, width, height, r, g, b) => commands.push([1, x, y, width, height, r, g, b]),
    web_draw_text: (x, y, value) => commands.push([2, x, y, value]),
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
    gfx_load_sprite: pathId => loadSprite(pathId),
    stasis_gfx_load_sprite: pathId => loadSprite(pathId),
    load_font: (pathId, size) => loadFont(pathId, size),
    stasis_load_font: (pathId, size) => loadFont(pathId, size),
    stasis_jit_gfx_cache_text: (font, textId) => {
      const handle = nextHandle++;
      cachedText.set(handle, { font, text: stringValue(textId) });
      return handle;
    },
    audio_init: () => { ensureAudio(); return 1; },
    audio_is_available: () => 1,
    audio_get_sample_rate: () => 48000,
    audio_get_channels: () => 2,
    audio_get_queued_frames: () => 0,
    audio_get_underruns: () => 0,
    stasis_jit_audio_load_music: pathId => loadAudio(pathId),
    stasis_jit_audio_load_effect: pathId => loadAudio(pathId),
    stasis_jit_audio_play_music: (handle, loop, volume) => startAudio(handle, loop, volume) ? 1 : 0,
    stasis_jit_audio_play_effect: (handle, volume) => startAudio(handle, false, volume) ? 1 : 0,
    stasis_jit_audio_stop_music: handle => stopAudio(handle),
    stasis_jit_audio_pause_music: (handle, paused) => {
      const voice = audioVoices.get(handle);
      if (!voice || voice.paused === Boolean(paused)) return;
      if (paused) voice.source.disconnect();
      else voice.source.connect(voice.gain);
      voice.paused = Boolean(paused);
    },
    stasis_jit_audio_set_music_volume: (handle, volume) => {
      const voice = audioVoices.get(handle);
      if (voice) voice.gain.gain.value = Math.max(0, Math.min(1, volume));
    }
  }};

  function executeCommands() {
    for (const command of commands) {
      if (command[0] === 0) {
        context.fillStyle = color(command[1], command[2], command[3]);
        context.fillRect(0, 0, canvas.width, canvas.height);
      } else if (command[0] === 1) {
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
    const flags = i32[2];
    if (flags & 1) {
      context.save();
      context.globalAlpha = Math.max(0, Math.min(1, f32[3]));
      context.fillStyle = `rgb(${Math.round(f32[0] * 255)} ${Math.round(f32[1] * 255)} ${Math.round(f32[2] * 255)})`;
      context.fillRect(0, 0, canvas.width, canvas.height);
      context.restore();
    }
    const drawLine = index => {
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
      context.save();
      context.globalAlpha = f32[base + 7];
      context.fillStyle = `rgb(${Math.round(f32[base + 4] * 255)} ${Math.round(f32[base + 5] * 255)} ${Math.round(f32[base + 6] * 255)})`;
      context.fillRect(f32[base], f32[base + 1], f32[base + 2], f32[base + 3]);
      context.restore();
    };
    const drawSprite = index => {
      const baseI = 32 + index * 3;
      const baseF = 80004 + index * 4;
      const image = sprites.get(i32[baseI]);
      if (!image || !image.complete || !image.naturalWidth) return;
      const x = f32[baseF];
      const y = f32[baseF + 1];
      const width = f32[baseF + 2];
      const height = f32[baseF + 3];
      context.save();
      context.globalAlpha = Math.max(0, Math.min(1, i32[baseI + 2] / 255));
      context.translate(x + width / 2, y + height / 2);
      context.rotate(i32[baseI + 1] * Math.PI / 180);
      context.drawImage(image, -width / 2, -height / 2, width, height);
      context.restore();
    };
    const drawText = index => {
      const baseI = 12320 + index * 3;
      const baseF = 96388 + index * 6;
      const offset = i32[baseI + 1];
      const cached = offset < 0 ? cachedText.get(-offset) : null;
      const fontHandle = cached ? cached.font : i32[baseI];
      const font = fonts.get(fontHandle) || { family: "ui-monospace", size: 18 };
      let text = cached ? cached.text : "";
      if (!cached && game.memory.gfx_cmd_u8) {
        const bytesLayout = game.memory.gfx_cmd_u8;
        const bytes = new Uint8Array(instance.exports.memory.buffer, bytesLayout.offset + offset, i32[baseI + 2]);
        text = new TextDecoder().decode(bytes);
      }
      context.save();
      context.globalAlpha = f32[baseF + 5];
      context.fillStyle = `rgb(${Math.round(f32[baseF + 2] * 255)} ${Math.round(f32[baseF + 3] * 255)} ${Math.round(f32[baseF + 4] * 255)})`;
      context.font = `${font.size}px ${font.family}`;
      context.textBaseline = "top";
      context.fillText(text, f32[baseF], f32[baseF + 1]);
      context.restore();
    };
    const lineCount = Math.max(0, Math.min(i32[3], 10000));
    const spriteCount = Math.max(0, Math.min(i32[4], 4096));
    const textCount = Math.max(0, Math.min(i32[7], 2048));
    const rectCount = Math.max(0, Math.min(i32[24], 10000 - lineCount));
    const orderCount = Math.max(0, Math.min(i32[22], 16144));
    if (orderCount > 0) {
      for (let order = 0; order < orderCount; order += 1) {
        const encoded = i32[18464 + order];
        const kind = Math.floor(encoded / 16384);
        const index = encoded % 16384;
        if (kind === 1 && index < lineCount) drawLine(index);
        else if (kind === 2 && index < spriteCount) drawSprite(index);
        else if (kind === 3 && index < textCount) drawText(index);
        else if (kind === 4 && index < rectCount) drawRect(index);
      }
    } else {
      for (let index = 0; index < lineCount; index += 1) drawLine(index);
      for (let index = 0; index < rectCount; index += 1) drawRect(index);
      for (let index = 0; index < spriteCount; index += 1) drawSprite(index);
      for (let index = 0; index < textCount; index += 1) drawText(index);
    }
  }

  function frame() {
    const tickStart = performance.now();
    instance.exports.tick();
    const tickMs = performance.now() - tickStart;
    const renderStart = performance.now();
    instance.exports.render();
    executeCommands();
    const renderMs = performance.now() - renderStart;
    frames += 1;
    if (frames > 5) {
      worstTick = Math.max(worstTick, tickMs);
      worstRender = Math.max(worstRender, renderMs);
    }
    const underBudget = worstTick < 16 && worstRender < 16;
    hud.textContent = `Wasm frame ${frames}\ntick ${tickMs.toFixed(3)} ms (worst ${worstTick.toFixed(3)})\nrender ${renderMs.toFixed(3)} ms (worst ${worstRender.toFixed(3)})\n${underBudget ? "UNDER 16 ms" : "OVER BUDGET"}`;
    document.body.dataset.frames = String(frames);
    document.body.dataset.tickMs = tickMs.toFixed(3);
    document.body.dataset.renderMs = renderMs.toFixed(3);
    document.body.dataset.worstTickMs = worstTick.toFixed(3);
    document.body.dataset.worstRenderMs = worstRender.toFixed(3);
    document.body.dataset.underBudget = String(underBudget);
    if (instance.exports.player_x) document.body.dataset.playerX = String(instance.exports.player_x.value);
    requestAnimationFrame(frame);
  }

  function updatePointer(event) {
    const bounds = canvas.getBoundingClientRect();
    pointer.x = Math.round((event.clientX - bounds.left) * canvas.width / bounds.width);
  }
  addEventListener("keydown", event => { keys.add(event.code); if (["ArrowLeft", "ArrowRight", "Space"].includes(event.code)) event.preventDefault(); });
  addEventListener("keyup", event => keys.delete(event.code));
  canvas.addEventListener("pointermove", updatePointer);
  canvas.addEventListener("pointerdown", event => { updatePointer(event); pointer.down = true; canvas.setPointerCapture(event.pointerId); canvas.focus(); });
  canvas.addEventListener("pointerup", event => { updatePointer(event); pointer.down = false; });
  canvas.addEventListener("pointercancel", () => { pointer.down = false; });
  audioButton.addEventListener("click", async () => {
    audioContext ||= new AudioContext();
    await audioContext.resume();
    for (const start of pendingAudio.splice(0)) await start();
    audioButton.textContent = "Sound enabled";
    audioButton.disabled = true;
    document.body.dataset.audioState = audioContext.state;
    canvas.focus();
  });

  async function wasmBytes() {
    if (window.STASIS_WASM_BASE64) {
      const binary = atob(window.STASIS_WASM_BASE64);
      return Uint8Array.from(binary, value => value.charCodeAt(0));
    }
    const response = await fetch("game.wasm");
    if (!response.ok) throw new Error(`failed to load game.wasm: ${response.status}`);
    return response.arrayBuffer();
  }

  (async () => {
    try {
      const result = await WebAssembly.instantiate(await wasmBytes(), imports);
      instance = result.instance;
      const mainResult = instance.exports.main();
      if (instance.exports.host_req_window_w_px && instance.exports.host_req_window_h_px) {
        const width = instance.exports.host_req_window_w_px.value;
        const height = instance.exports.host_req_window_h_px.value;
        if (width > 0 && height > 0) {
          canvas.width = width;
          canvas.height = height;
          canvas.style.aspectRatio = `${width} / ${height}`;
        }
      }
      await Promise.all([
        ...Array.from(sprites.values(), image => image.decode().catch(() => undefined)),
        document.fonts.ready
      ]);
      document.body.dataset.ready = "true";
      document.body.dataset.runtime = "wasm";
      document.body.dataset.mainResult = String(mainResult);
      requestAnimationFrame(frame);
    } catch (error) {
      document.body.dataset.ready = "false";
      errorBox.textContent = String(error && error.stack || error);
      throw error;
    }
  })();
})();
