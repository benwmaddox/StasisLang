import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";

const html = fs.readFileSync(new URL("../index.html", import.meta.url), "utf8");
const fitter = html.match(/<script>\s*([\s\S]*?)\s*<\/script>/)?.[1];
assert.ok(fitter, "index.html has an inline viewport fitter");

function runFitter({ layoutWidth = 393, layoutHeight = 844, visualWidth = 393, visualHeight = 650, backingWidth = 640, backingHeight = 360, safe = {}, visual = true } = {}) {
  const windowListeners = new Map();
  const visualListeners = new Map();
  const mutations = [];
  const mutationOptions = [];
  const rootStyle = {
    values: {},
    setProperty(name, value) { this.values[name] = value; }
  };
  const shellStyle = {};
  const canvasStyle = {};
  const canvas = {
    width: backingWidth,
    height: backingHeight,
    style: canvasStyle,
    parentElement: { style: shellStyle }
  };
  const visualViewport = visual ? {
    width: visualWidth,
    height: visualHeight,
    offsetLeft: 0,
    offsetTop: 0,
    addEventListener(type, listener) { visualListeners.set(type, listener); }
  } : undefined;
  const document = {
    body: {},
    documentElement: { clientWidth: layoutWidth, clientHeight: layoutHeight, style: rootStyle },
    getElementById(id) { return id === "stasis-canvas" ? canvas : null; }
  };
  const context = {
    document,
    window: { visualViewport, innerWidth: layoutWidth, innerHeight: layoutHeight },
    getComputedStyle: () => ({
      getPropertyValue(name) {
        const side = name.slice("padding-".length);
        return String(safe[side] || 0);
      }
    }),
    addEventListener(type, listener) {
      const listeners = windowListeners.get(type) || [];
      listeners.push(listener);
      windowListeners.set(type, listeners);
    },
    MutationObserver: class {
      constructor(callback) { mutations.push(callback); }
      observe(_target, options) { mutationOptions.push(options); }
    }
  };
  vm.runInNewContext(fitter, context, { filename: "runtime/web/index.html" });
  return {
    canvas,
    shellStyle,
    rootStyle,
    windowListeners,
    visualListeners,
    mutations,
    mutationOptions,
    canvasStyle,
    visualViewport,
    dispatch(type) { for (const listener of windowListeners.get(type) || []) listener(); },
    dispatchVisual(type) { visualListeners.get(type)?.(); }
  };
}

test("shared web shell uses the visible viewport and preserves backing size", () => {
  const fit = runFitter({ backingWidth: 160, backingHeight: 900, safe: { top: 24, right: 0, bottom: 34, left: 0 } });
  assert.equal(fit.rootStyle.values["--stasis-visible-width"], "393px");
  assert.equal(fit.rootStyle.values["--stasis-visible-height"], "650px");
  assert.ok(Math.abs(parseFloat(fit.shellStyle.width) - 105.24444444444444) < 1e-9);
  assert.equal(fit.shellStyle.height, "592px");
  assert.notStrictEqual(fit.shellStyle, fit.canvasStyle);
  assert.equal(fit.canvasStyle.width, undefined);
  assert.equal(fit.canvas.width, 160);
  assert.equal(fit.canvas.height, 900);
  assert.equal(fit.visualListeners.size, 2);
  assert.equal(fit.windowListeners.get("resize")?.length, 1);
  assert.equal(fit.windowListeners.get("orientationchange")?.length, 1);
  assert.equal(fit.mutationOptions.length, 1);
  assert.equal(fit.mutationOptions[0].attributes, true);
  assert.deepEqual(Array.from(fit.mutationOptions[0].attributeFilter), ["width", "height"]);
  assert.equal(parseFloat(fit.shellStyle.width) <= 393 && 592 <= 650 - 24 - 34, true);
  assert.equal(parseFloat(fit.shellStyle.width) <= 393 && 592 <= 844, true, "the VM models 393x844 layout and 393x650 visual viewports");
});

test("visual viewport and orientation changes refit once without duplicate listeners", () => {
  const fit = runFitter({ layoutWidth: 844, layoutHeight: 393, visualWidth: 844, visualHeight: 393 });
  fit.visualViewport.width = 393;
  fit.visualViewport.height = 650;
  fit.dispatchVisual("resize");
  assert.equal(fit.shellStyle.width, "393px");
  assert.equal(fit.shellStyle.height, "221.0625px");
  const beforeScroll = { ...fit.shellStyle };
  fit.visualViewport.offsetTop = 12;
  fit.dispatchVisual("scroll");
  assert.deepEqual(fit.shellStyle, beforeScroll, "origin-only scroll does not translate the grid-centered shell");
  fit.visualViewport.height = 640;
  fit.dispatchVisual("scroll");
  assert.equal(fit.rootStyle.values["--stasis-visible-height"], "640px", "scroll can still refit when the visible extent changes");
  fit.dispatch("resize");
  fit.dispatch("orientationchange");
  assert.equal(fit.windowListeners.get("resize")?.length, 1);
  assert.equal(fit.windowListeners.get("orientationchange")?.length, 1);
  assert.equal(fit.visualListeners.get("resize") !== undefined, true);
});

test("intrinsic backing mutation changes fit ratio without changing the backing", () => {
  const fit = runFitter();
  fit.canvas.width = 320;
  fit.canvas.height = 240;
  fit.mutations[0]();
  assert.equal(fit.shellStyle.width, "393px");
  assert.equal(fit.shellStyle.height, "294.75px");
  assert.equal(fit.canvas.width, 320);
  assert.equal(fit.canvas.height, 240);
});

test("layout viewport fallback works when visualViewport is unavailable", () => {
  const fit = runFitter({ layoutWidth: 480, layoutHeight: 800, visual: false });
  assert.equal(fit.rootStyle.values["--stasis-visible-width"], "480px");
  assert.equal(fit.rootStyle.values["--stasis-visible-height"], "800px");
  assert.equal(fit.visualListeners.size, 0);
  assert.equal(fit.windowListeners.get("resize")?.length, 1);
});

test("index shell contract is safe-area aware and has one fitter", () => {
  assert.match(html, /viewport-fit=cover/);
  assert.match(html, /safe-area-inset-top/);
  assert.match(html, /safe-area-inset-bottom/);
  assert.match(html, /100svh/);
  assert.match(html, /100dvh/);
  assert.equal((html.match(/<script>\s*\(\(\) =>/g) || []).length, 1);
  assert.equal((html.match(/addEventListener\("resize", fit\)/g) || []).length, 2);
  assert.equal((html.match(/addEventListener\("orientationchange", fit\)/g) || []).length, 1);
});
