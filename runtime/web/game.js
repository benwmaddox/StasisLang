(() => {
  "use strict";
  const canvas = document.getElementById("stasis-canvas");
  const context = canvas.getContext("2d", { alpha: false });
  const hud = document.getElementById("stasis-hud");
  const audioButton = document.getElementById("stasis-audio");
  const errorBox = document.getElementById("stasis-error");
  const keys = new Set();
  const pointer = { id: 0, x: 0, y: 0, dx: 0, dy: 0, down: false, wentDown: false, wentUp: false };
  const commands = [];
  const game = window.STASIS_GAME || { strings: {}, memory: {}, assets: {} };
  const sprites = new Map();
  const fonts = new Map();
  const cachedText = new Map();
  let nextHandle = 1;
  let instance;
  let audioContext;
  let audioEvents = 0;
  let audioSampleRate = 48000;
  let audioChannels = 2;
  let audioNextStart = 0;
  let audioUnderruns = 0;
  let audioSuspendedByLifecycle = false;
  const audioAssets = new Map();
  const audioVoices = new Map();
  const pendingAudio = [];
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
  const startedAt = performance.now();

  const color = (r, g, b) => `rgb(${r & 255} ${g & 255} ${b & 255})`;
  const stringValue = id => game.strings[String(id)] || "";
  const assetKey = value => value.replace(/^(?:\.\.\/|\.\/)+/, "");
  const assetValue = id => {
    const value = stringValue(id);
    return game.assets[assetKey(value)] || value;
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
  const updateAudioState = () => {
    document.body.dataset.audioState = audioContext?.state || "closed";
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
        updateAudioState();
      }
    }).catch(() => {
      if (resumingContext === audioContext) audioSuspendedByLifecycle = true;
    });
  };
  const shutdownWebAudio = () => {
    audioSuspendedByLifecycle = false;
    pendingAudio.length = 0;
    for (const handle of Array.from(audioVoices.keys())) stopAudio(handle);
    const closingContext = audioContext;
    audioContext = undefined;
    audioNextStart = 0;
    if (closingContext && closingContext.state !== "closed") void closingContext.close();
    updateAudioState();
  };
  const pushAudio = (byteOffset, frameCount) => {
    if (!instance?.exports.memory || frameCount <= 0) return 0;
    const sampleCount = frameCount * audioChannels;
    if (byteOffset < 0 || byteOffset + sampleCount * 4 > instance.exports.memory.buffer.byteLength) return 0;
    const samples = new Float32Array(instance.exports.memory.buffer, byteOffset, sampleCount).slice();
    const start = async () => {
      const audio = ensureAudio();
      const buffer = audio.createBuffer(audioChannels, frameCount, audioSampleRate);
      for (let channel = 0; channel < audioChannels; channel += 1) {
        const output = buffer.getChannelData(channel);
        for (let frame = 0; frame < frameCount; frame += 1) output[frame] = samples[frame * audioChannels + channel];
      }
      const source = audio.createBufferSource();
      source.buffer = buffer;
      source.connect(audio.destination);
      const earliest = audio.currentTime + 0.005;
      if (audioNextStart > 0 && audioNextStart < audio.currentTime) audioUnderruns += 1;
      const startAt = Math.max(earliest, audioNextStart);
      source.start(startAt);
      audioNextStart = startAt + frameCount / audioSampleRate;
      audioEvents += 1;
      document.body.dataset.audioEvents = String(audioEvents);
      document.body.dataset.audioMode = "stream";
    };
    if (!audioContext || audioContext.state !== "running") pendingAudio.push(start);
    else void start();
    return frameCount;
  };
  const imports = { env: {
    sin_fast: value => Math.sin(value),
    cos_fast: value => Math.cos(value),
    print_i32: value => console.log(value),
    print_int: value => console.log(value),
    print_char: value => console.log(String.fromCodePoint(value)),
    print_string: value => console.log(stringValue(value)),
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
      return setViewField(base, index, "font", font)
        && setViewField(base, index, "handle", handle)
        && setViewField(base, index, "width", text.length * fontInfo.size * 0.6)
        && setViewField(base, index, "height", fontInfo.size) ? 1 : 0;
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
    audio_get_queued_frames: () => audioContext ? Math.max(0, Math.round((audioNextStart - audioContext.currentTime) * audioSampleRate)) : 0,
    audio_get_underruns: () => audioUnderruns,
    audio_push_f32_interleaved: (byteOffset, frameCount) => pushAudio(byteOffset, frameCount),
    audio_load_wav: pathId => loadAudio(pathId),
    audio_release: handle => { audioAssets.delete(handle); stopAudio(handle); },
    audio_play: (handle, loop, volume) => startAudio(handle, loop, volume) ? handle : 0,
    audio_stop: handle => stopAudio(handle),
    audio_voice_is_playing: handle => audioVoices.has(handle) ? 1 : 0,
    audio_voice_set_paused: (handle, paused) => {
      const voice = audioVoices.get(handle);
      if (!voice || voice.paused === Boolean(paused)) return;
      if (paused) voice.source.disconnect();
      else voice.source.connect(voice.gain);
      voice.paused = Boolean(paused);
    },
    audio_voice_set_volume_pan: (handle, volume) => {
      const voice = audioVoices.get(handle);
      if (voice) voice.gain.gain.value = Math.max(0, Math.min(1, volume));
    },
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

  document.addEventListener("paste", event => {
    clipboardText = event.clipboardData?.getData("text/plain") || clipboardText;
  });
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
      context.drawImage(image, sourceX, sourceY, sourceWidth, sourceHeight, -width / 2, -height / 2, width, height);
      context.restore();
    };
    const drawText = index => {
      const baseI = 12320 + index * 3;
      const baseF = textBase + index * 6;
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
    const rectCount = version >= 4 ? Math.max(0, Math.min(i32[24], 10000 - lineCount)) : 0;
    const orderCount = version >= 3 ? Math.max(0, Math.min(i32[22], 16144)) : 0;
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
    const pointerCount = pointer.down || pointer.wentDown || pointer.wentUp ? 1 : 0;
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
    if (hud) {
      hud.textContent = `Wasm frame ${frames}\ntick ${tickMs.toFixed(3)} ms (worst ${worstTick.toFixed(3)})\nrender ${renderMs.toFixed(3)} ms (worst ${worstRender.toFixed(3)})\n${underBudget ? "UNDER 16 ms" : "OVER BUDGET"}`;
    }
    document.body.dataset.frames = String(frames);
    document.body.dataset.tickMs = tickMs.toFixed(3);
    document.body.dataset.renderMs = renderMs.toFixed(3);
    document.body.dataset.worstTickMs = worstTick.toFixed(3);
    document.body.dataset.worstRenderMs = worstRender.toFixed(3);
    document.body.dataset.underBudget = String(underBudget);
    if (instance.exports.player_x) document.body.dataset.playerX = String(instance.exports.player_x.value);
    finishHostFrame();
    requestAnimationFrame(frame);
  }

  function updatePointer(event) {
    const bounds = canvas.getBoundingClientRect();
    const x = Math.round((event.clientX - bounds.left) * canvas.width / bounds.width);
    const y = Math.round((event.clientY - bounds.top) * canvas.height / bounds.height);
    pointer.dx += x - pointer.x;
    pointer.dy += y - pointer.y;
    pointer.x = x;
    pointer.y = y;
    pointer.id = event.pointerId | 0;
  }
  addEventListener("keydown", event => {
    keys.add(event.code);
    void applyFullscreenGesture();
    if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Space"].includes(event.code)) event.preventDefault();
  });
  addEventListener("keyup", event => { keys.delete(event.code); void applyFullscreenGesture(); });
  canvas.addEventListener("pointermove", updatePointer);
  canvas.addEventListener("pointerdown", event => {
    updatePointer(event);
    pointer.down = true;
    pointer.wentDown = true;
    canvas.setPointerCapture(event.pointerId);
    canvas.focus();
    void applyFullscreenGesture();
  });
  canvas.addEventListener("pointerup", event => {
    updatePointer(event);
    pointer.down = false;
    pointer.wentUp = true;
  });
  canvas.addEventListener("pointercancel", () => { pointer.down = false; pointer.wentUp = true; });
  addEventListener("resize", () => { resized = true; displayGeneration += 1; });
  document.addEventListener("fullscreenchange", () => { resized = true; displayGeneration += 1; });
  audioButton.addEventListener("click", async () => {
    await applyFullscreenGesture();
    audioContext ||= new AudioContext();
    await audioContext.resume();
    for (const start of pendingAudio.splice(0)) await start();
    audioButton.textContent = "Sound enabled";
    audioButton.disabled = true;
    updateAudioState();
    canvas.focus();
  });

  async function wasmBytes() {
    const response = await fetch("game.wasm");
    if (!response.ok) throw new Error(`failed to load game.wasm: ${response.status}`);
    return response.arrayBuffer();
  }

  (async () => {
    try {
      const result = await WebAssembly.instantiate(await wasmBytes(), imports);
      instance = result.instance;
      writeHostFrame(performance.now());
      const mainResult = instance.exports.main();
      finishHostFrame();
      applyWindowRequest();
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
