(() => {
  "use strict";
  const canvas = document.getElementById("stasis-canvas");
  const context = canvas.getContext("2d", { alpha: false });
  const hud = document.getElementById("stasis-hud");
  const errorBox = document.getElementById("stasis-error");
  const game = window.STASIS_GAME || { strings: {} };
  const commands = [];
  const stringValue = id => game.strings[String(id)] || "";
  const color = (r, g, b) => `rgb(${r & 255} ${g & 255} ${b & 255})`;
  const imports = { env: {
__STASIS_IMPORTS__
  }};
  let instance;
  let frames = 0;
  let worstTick = 0;
  let worstRender = 0;

  function executeCommands() {
    for (const command of commands) {
      if (command[0] === 0) {
        context.fillStyle = color(command[1], command[2], command[3]);
        context.fillRect(0, 0, canvas.width, canvas.height);
      } else if (command[0] === 1) {
        context.fillStyle = color(command[5], command[6], command[7]);
        context.fillRect(command[1], command[2], command[3], command[4]);
      }
      else if (command[0] === 2) {
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
    const underBudget = worstTick < 16 && worstRender < 16;
    if (hud) hud.textContent = `Wasm frame ${frames}\ntick ${tickMs.toFixed(3)} ms\nrender ${renderMs.toFixed(3)} ms\n${underBudget ? "UNDER 16 ms" : "OVER BUDGET"}`;
    document.body.dataset.frames = String(frames);
    document.body.dataset.underBudget = String(underBudget);
    requestAnimationFrame(frame);
  }

  (async () => {
    try {
      const response = await fetch("game.wasm");
      if (!response.ok) throw new Error(`failed to load game.wasm: ${response.status}`);
      instance = (await WebAssembly.instantiate(await response.arrayBuffer(), imports)).instance;
      document.body.dataset.mainResult = String(instance.exports.main());
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
