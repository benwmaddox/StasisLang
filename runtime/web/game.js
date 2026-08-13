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
  let instance;
  let audioContext;
  let audioEvents = 0;
  let frames = 0;
  let worstTick = 0;
  let worstRender = 0;

  const color = (r, g, b) => `rgb(${r & 255} ${g & 255} ${b & 255})`;
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
    const underBudget = tickMs < 16 && renderMs < 16;
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
      instance.exports.main();
      document.body.dataset.ready = "true";
      document.body.dataset.runtime = "wasm";
      requestAnimationFrame(frame);
    } catch (error) {
      document.body.dataset.ready = "false";
      errorBox.textContent = String(error && error.stack || error);
      throw error;
    }
  })();
})();
