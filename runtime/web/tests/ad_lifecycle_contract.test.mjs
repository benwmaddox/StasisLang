import test from "node:test";
import assert from "node:assert/strict";
import "../ad_lifecycle.js";

const { createAdLifecycle } = globalThis.STASIS_AD_LIFECYCLE ?? {};

class FakeClock {
  now = 0;
  nextId = 1;
  timers = new Map();

  setTimeout = (callback, delay = 0) => {
    const id = this.nextId++;
    this.timers.set(id, {
      at: this.now + Math.max(0, Number(delay) || 0),
      callback,
    });
    return id;
  };

  clearTimeout = (id) => this.timers.delete(id);

  advance(milliseconds) {
    const target = this.now + milliseconds;
    let steps = 0;
    while (true) {
      let nextId = 0;
      let next = null;
      for (const [id, timer] of this.timers) {
        if (timer.at <= target && (!next || timer.at < next.at)) {
          nextId = id;
          next = timer;
        }
      }
      if (!next) break;
      assert.ok(++steps < 10000, "fake timers should not loop forever");
      this.timers.delete(nextId);
      this.now = next.at;
      next.callback();
    }
    this.now = target;
  }
}

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

async function flushPromises() {
  for (let i = 0; i < 12; i += 1) await Promise.resolve();
}

async function readyAfterMicrotasks(manager) {
  let settled = false;
  let result;
  manager.ready.then((value) => {
    settled = true;
    result = value;
  });
  await flushPromises();
  assert.equal(settled, true, "provider readiness should settle after its documented ready event");
  return result;
}

function makeHarness(provider, { game_id = "test-game", onAppend, onBeforeCreate } = {}) {
  const clock = new FakeClock();
  const scripts = [];
  let capturedVendorEvent = null;
  const head = {
    appendChild(script) {
      scripts.push(script);
      const options = provider === "gamemonetize"
        ? windowObject.SDK_OPTIONS
        : provider === "gamedistribution"
          ? windowObject.GD_OPTIONS
          : null;
      capturedVendorEvent = options?.onEvent ?? null;
      onAppend?.(script, windowObject);
      return script;
    },
    removeChild() {},
  };
  const documentObject = {
    head,
    documentElement: head,
    createElement(tagName) {
      return { tagName, async: false, src: "", id: "" };
    },
  };
  const windowObject = {
    navigator: { userActivation: { isActive: false } },
  };
  onBeforeCreate?.(windowObject);
  const calls = { blocked: [], audio: [], gestures: [], diagnostics: [] };
  const manager = createAdLifecycle({
    provider,
    game_id,
    windowObject,
    documentObject,
    setTimeoutFn: clock.setTimeout,
    clearTimeoutFn: clock.clearTimeout,
    nowFn: () => clock.now,
    onGameplayBlocked: (value) => calls.blocked.push(value),
    onAudioPaused: (value) => calls.audio.push(value),
    onGestureNeeded: (value) => calls.gestures.push(value),
    onDiagnostic: (value) => calls.diagnostics.push(value),
  });
  return {
    manager,
    clock,
    windowObject,
    documentObject,
    calls,
    scripts,
    emitVendorEvent(name) {
      capturedVendorEvent?.({ name });
    },
  };
}

function attachCrazy(harness, { init = async () => undefined } = {}) {
  const adCalls = [];
  const lifecycleCalls = [];
  harness.windowObject.CrazyGames = {
    SDK: {
      init,
      ad: {
        requestAd(kind, callbacks) {
          adCalls.push({ kind, callbacks });
        },
      },
      game: {
        loadingStart() { lifecycleCalls.push("loadingStart"); },
        loadingStop() { lifecycleCalls.push("loadingStop"); },
        gameplayStart() { lifecycleCalls.push("gameplayStart"); },
        gameplayStop() { lifecycleCalls.push("gameplayStop"); },
      },
    },
  };
  for (const script of harness.scripts) script.onload?.();
  adCalls.lifecycleCalls = lifecycleCalls;
  return adCalls;
}

function attachPoki(harness, { init = async () => undefined } = {}) {
  const adCalls = [];
  const lifecycleCalls = [];
  harness.windowObject.PokiSDK = {
    init,
    gameLoadingFinished() { lifecycleCalls.push("gameLoadingFinished"); },
    gameplayStart() { lifecycleCalls.push("gameplayStart"); },
    gameplayStop() { lifecycleCalls.push("gameplayStop"); },
    commercialBreak(onStart) {
      const result = deferred();
      adCalls.push({ kind: 0, onStart, result });
      return result.promise;
    },
    rewardedBreak(onStart) {
      const result = deferred();
      adCalls.push({ kind: 1, onStart, result });
      return result.promise;
    },
  };
  for (const script of harness.scripts) script.onload?.();
  adCalls.lifecycleCalls = lifecycleCalls;
  return adCalls;
}

async function markReady(harness, event = "SDK_READY") {
  if (harness.windowObject.SDK_OPTIONS) {
    harness.windowObject.SDK_OPTIONS.onEvent({ name: event });
  } else if (harness.windowObject.GD_OPTIONS) {
    harness.windowObject.GD_OPTIONS.onEvent({ name: event });
  }
  await flushPromises();
  return harness.manager.ready;
}

function activateGameDistributionGesture(harness) {
  harness.windowObject.navigator.userActivation.isActive = true;
  const gesture = harness.calls.gestures.at(-1);
  assert.equal(gesture?.visible, true, "GameDistribution should present its activation prompt");
  assert.equal(gesture.activate({
    type: "pointerup",
    pointerType: "mouse",
    isPrimary: true,
    button: 0,
    isTrusted: true,
    eventPhase: 2,
    currentTarget: {},
  }), true);
}

test("factory exposes the reviewed ABI, lifecycle codes, and deterministic none profile", async () => {
  assert.equal(typeof createAdLifecycle, "function");
  const h = makeHarness("none");
  assert.deepEqual(await readyAfterMicrotasks(h.manager), { available: false, diagnostic: "No ad provider is selected." });
  assert.equal(h.manager.request(0), 0);
  assert.equal(h.manager.request(1), 0);
  assert.equal(h.manager.request(2), 0);
  assert.deepEqual(h.scripts, []);
  for (const event of [1, 2, 3, 4, 99]) h.manager.lifecycle(event);
  assert.equal(h.manager.poll(1), 4);
  assert.equal(h.manager.takeReward(1), 0);
  h.manager.release(1);
  assert.equal(h.manager.gameplayBlocked(), 0);
});

test("lifecycle notifications wait for SDK readiness and replay documented events", async () => {
  const cgInit = deferred();
  const cg = makeHarness("crazygames");
  const cgCalls = attachCrazy(cg, { init: () => cgInit.promise });
  cg.manager.lifecycle(1);
  cg.manager.lifecycle(3);
  cg.manager.lifecycle(4);
  assert.deepEqual(cgCalls.lifecycleCalls, []);
  cgInit.resolve();
  await readyAfterMicrotasks(cg.manager);
  assert.deepEqual(cgCalls.lifecycleCalls, ["loadingStart", "gameplayStart", "gameplayStop"]);

  const pkInit = deferred();
  const pk = makeHarness("poki");
  const pkCalls = attachPoki(pk, { init: () => pkInit.promise });
  pk.manager.lifecycle(1);
  pk.manager.lifecycle(2);
  pk.manager.lifecycle(3);
  assert.deepEqual(pkCalls.lifecycleCalls, []);
  assert.equal(pk.windowObject.PokiSDK.gameLoadingStart, undefined);
  pkInit.resolve();
  await readyAfterMicrotasks(pk.manager);
  assert.deepEqual(pkCalls.lifecycleCalls, ["gameLoadingFinished", "gameplayStart"]);

  const overflowInit = deferred();
  const overflow = makeHarness("crazygames");
  const overflowCalls = attachCrazy(overflow, { init: () => overflowInit.promise });
  for (let index = 0; index < 10; index += 1) overflow.manager.lifecycle(index % 2 === 0 ? 1 : 2);
  overflowInit.resolve();
  await readyAfterMicrotasks(overflow.manager);
  assert.deepEqual(overflowCalls.lifecycleCalls, [
    "loadingStart", "loadingStop", "loadingStart", "loadingStop",
    "loadingStart", "loadingStop", "loadingStart", "loadingStop",
  ]);
});


test("request deadline covers an unresolved SDK initialization and init timeout remains bounded", async () => {
  const init = deferred();
  const h = makeHarness("crazygames");
  const adCalls = attachCrazy(h, { init: () => init.promise });
  const handle = h.manager.request(0);
  assert.ok(handle > 0);
  assert.equal(h.manager.poll(handle), 1);
  assert.equal(h.manager.gameplayBlocked(), 1);
  assert.deepEqual(h.calls.audio, [false]);
  h.clock.advance(4999);
  assert.equal(h.manager.poll(handle), 1);
  h.clock.advance(1);
  assert.equal(h.manager.poll(handle), 4);
  h.clock.advance(1);
  assert.equal(h.manager.poll(handle), 4);
  assert.equal(h.manager.gameplayBlocked(), 0);
  assert.equal(adCalls.length, 0);
  h.clock.advance(2999);
  assert.deepEqual(await h.manager.ready, {
    available: false,
    diagnostic: "Ad SDK initialization timed out.",
  });
  assert.equal(h.manager.poll(handle), 4);
  assert.equal(h.manager.gameplayBlocked(), 0);
  init.resolve();
  await flushPromises();
  assert.equal(h.manager.poll(handle), 4);
});

test("CrazyGames rewards only its successful rewarded finish and pauses only on actual start", async () => {
  const h = makeHarness("crazygames");
  const adCalls = attachCrazy(h);
  assert.deepEqual(await readyAfterMicrotasks(h.manager), { available: true });
  const handle = h.manager.request(1);
  assert.equal(h.manager.poll(handle), 1);
  assert.equal(adCalls.length, 1);
  assert.equal(adCalls[0].kind, "rewarded");
  assert.equal(h.manager.gameplayBlocked(), 1);
  assert.deepEqual(h.calls.audio, [false]);

  adCalls[0].callbacks.adStarted();
  assert.equal(h.manager.poll(handle), 2);
  assert.equal(h.manager.gameplayBlocked(), 1);
  assert.equal(h.calls.audio.at(-1), true);

  adCalls[0].callbacks.adFinished();
  assert.equal(h.manager.poll(handle), 3);
  assert.equal(h.manager.gameplayBlocked(), 0);
  assert.equal(h.calls.audio.at(-1), false);
  assert.equal(h.manager.takeReward(handle), 1);
  assert.equal(h.manager.takeReward(handle), 0);
  adCalls[0].callbacks.adFinished();
  adCalls[0].callbacks.adError({ code: "unfilled" });
  assert.equal(h.manager.poll(handle), 3);
  assert.equal(h.manager.takeReward(handle), 0);
});

test("CrazyGames no-fill, Basic Launch, init rejection, and synchronous invocation errors never pause or reward", async () => {
  const h = makeHarness("crazygames");
  const adCalls = attachCrazy(h);
  const handle = h.manager.request(0);
  await readyAfterMicrotasks(h.manager);
  assert.equal(h.manager.gameplayBlocked(), 1);
  adCalls[0].callbacks.adError({ code: "adsDisabledBasicLaunch" });
  assert.equal(h.manager.poll(handle), 4);
  assert.equal(h.manager.gameplayBlocked(), 0);
  assert.deepEqual(h.calls.audio, [false]);
  assert.equal(h.manager.takeReward(handle), 0);

  const rejected = makeHarness("crazygames");
  attachCrazy(rejected, { init: () => Promise.reject(new Error("init rejected")) });
  const waiting = rejected.manager.request(1);
  await flushPromises();
  assert.deepEqual(await readyAfterMicrotasks(rejected.manager), {
    available: false,
    diagnostic: "CrazyGames SDK initialization failed.",
  });
  assert.equal(rejected.manager.poll(waiting), 5);

  const throwing = makeHarness("crazygames");
  const throwingCalls = attachCrazy(throwing);
  throwingCalls.length = 0;
  let lateCallbacks;
  throwing.windowObject.CrazyGames.SDK.ad.requestAd = (_kind, callbacks) => {
    lateCallbacks = callbacks;
    throw new Error("provider throw");
  };
  const thrownHandle = throwing.manager.request(1);
  await readyAfterMicrotasks(throwing.manager);
  assert.equal(throwing.manager.poll(thrownHandle), 4);
  assert.equal(throwing.manager.gameplayBlocked(), 0);
  assert.equal(throwing.manager.takeReward(thrownHandle), 0);
  assert.equal(throwing.manager.request(0), -1);
  lateCallbacks.adStarted();
  assert.equal(throwing.manager.gameplayBlocked(), 1);
  lateCallbacks.adError({ code: "other" });
  assert.equal(throwing.manager.gameplayBlocked(), 0);
});

test("CrazyGames unknown-state deadline keeps an issued orphan reserved and honors a late actual start", async () => {
  const h = makeHarness("crazygames");
  const adCalls = attachCrazy(h);
  const handle = h.manager.request(1);
  await readyAfterMicrotasks(h.manager);
  assert.equal(h.manager.gameplayBlocked(), 1);
  h.clock.advance(4999);
  assert.equal(h.manager.poll(handle), 1);
  h.clock.advance(1);
  assert.equal(h.manager.poll(handle), 4);
  assert.equal(h.manager.gameplayBlocked(), 0);
  h.manager.release(handle);

  assert.equal(h.manager.request(0), -1);
  adCalls[0].callbacks.adStarted();
  assert.equal(h.manager.gameplayBlocked(), 1);
  assert.equal(h.calls.audio.at(-1), true);
  assert.equal(h.manager.request(0), -1);
  adCalls[0].callbacks.adFinished();
  assert.equal(h.manager.gameplayBlocked(), 0);
  assert.equal(h.calls.audio.at(-1), false);
});

test("CrazyGames unknown-state deadline abandons reward even if a late finish callback arrives", async () => {
  const h = makeHarness("crazygames");
  const adCalls = attachCrazy(h);
  const handle = h.manager.request(1);
  await readyAfterMicrotasks(h.manager);
  h.clock.advance(5000);
  assert.equal(h.manager.poll(handle), 4);
  adCalls[0].callbacks.adFinished();
  assert.equal(h.manager.poll(handle), 4);
  assert.equal(h.manager.takeReward(handle), 0);
});

test("actual CrazyGames playback clears the deadline and remains blocked past both limits", async () => {
  const h = makeHarness("crazygames");
  const adCalls = attachCrazy(h);
  const handle = h.manager.request(0);
  await readyAfterMicrotasks(h.manager);
  h.clock.advance(4999);
  adCalls[0].callbacks.adStarted();
  assert.equal(h.manager.poll(handle), 2);
  assert.equal(h.calls.audio.at(-1), true);
  h.clock.advance(30001);
  assert.equal(h.manager.poll(handle), 2);
  assert.equal(h.manager.gameplayBlocked(), 1);
  assert.equal(h.calls.audio.at(-1), true);
  adCalls[0].callbacks.adFinished();
  assert.equal(h.manager.poll(handle), 3);
  assert.equal(h.manager.gameplayBlocked(), 0);
});

test("Poki resolves no-ad breaks without audio changes and rewards only boolean true", async () => {
  const h = makeHarness("poki");
  const adCalls = attachPoki(h);
  await readyAfterMicrotasks(h.manager);
  const midgame = h.manager.request(0);
  assert.equal(h.manager.gameplayBlocked(), 1);
  assert.deepEqual(h.calls.audio, [false]);
  adCalls[0].result.resolve(undefined);
  await flushPromises();
  assert.equal(h.manager.poll(midgame), 3);
  assert.equal(h.manager.gameplayBlocked(), 0);
  assert.deepEqual(h.calls.audio, [false]);

  const rewarded = h.manager.request(1);
  adCalls[1].result.resolve(false);
  await flushPromises();
  assert.equal(h.manager.poll(rewarded), 3);
  assert.equal(h.manager.takeReward(rewarded), 0);

  const success = h.manager.request(1);
  assert.equal(adCalls.length, 3);
  adCalls[2].result.resolve(true);
  await flushPromises();
  assert.equal(h.manager.poll(success), 3);
  assert.equal(h.manager.takeReward(success), 1);
  assert.equal(h.manager.takeReward(success), 0);

  const noResult = h.manager.request(1);
  adCalls[3].result.resolve(undefined);
  await flushPromises();
  assert.equal(h.manager.poll(noResult), 3);
  assert.equal(h.manager.takeReward(noResult), 0);

  const objectResult = h.manager.request(1);
  adCalls[4].result.resolve({ success: true });
  await flushPromises();
  assert.equal(h.manager.poll(objectResult), 3);
  assert.equal(h.manager.takeReward(objectResult), 0);
});

test("Poki onStart pauses before promise settlement; rejection and synchronous throws fail without reward", async () => {
  const h = makeHarness("poki");
  const adCalls = attachPoki(h);
  const handle = h.manager.request(0);
  await readyAfterMicrotasks(h.manager);
  assert.equal(h.manager.gameplayBlocked(), 1);
  adCalls[0].onStart();
  assert.equal(h.manager.poll(handle), 2);
  assert.equal(h.manager.gameplayBlocked(), 1);
  assert.equal(h.calls.audio.at(-1), true);
  adCalls[0].result.resolve(undefined);
  await flushPromises();
  assert.equal(h.manager.poll(handle), 3);
  assert.equal(h.manager.gameplayBlocked(), 0);
  assert.equal(h.calls.audio.at(-1), false);

  const rejectHarness = makeHarness("poki");
  const rejectCalls = attachPoki(rejectHarness);
  const rejected = rejectHarness.manager.request(1);
  await readyAfterMicrotasks(rejectHarness.manager);
  rejectCalls[0].result.reject(new Error("ad failed"));
  await flushPromises();
  assert.equal(rejectHarness.manager.poll(rejected), 4);
  assert.equal(rejectHarness.manager.takeReward(rejected), 0);

  const throwHarness = makeHarness("poki");
  attachPoki(throwHarness);
  throwHarness.windowObject.PokiSDK.rewardedBreak = () => {
    throw new Error("provider throw");
  };
  const thrown = throwHarness.manager.request(1);
  await readyAfterMicrotasks(throwHarness.manager);
  assert.equal(throwHarness.manager.poll(thrown), 4);
  assert.equal(throwHarness.manager.gameplayBlocked(), 0);
  assert.equal(throwHarness.manager.request(0), -1);
  throwHarness.manager.dispose();
});

test("a synchronous throw after CrazyGames or Poki reports start cannot clear the physical pause", async () => {
  const cg = makeHarness("crazygames");
  const cgAdCalls = attachCrazy(cg);
  let callbacks;
  cg.windowObject.CrazyGames.SDK.ad.requestAd = (_kind, adCallbacks) => {
    callbacks = adCallbacks;
    adCallbacks.adStarted();
    throw new Error("throw after start");
  };
  const cgHandle = cg.manager.request(0);
  await readyAfterMicrotasks(cg.manager);
  assert.equal(cg.manager.poll(cgHandle), 4);
  assert.equal(cg.manager.gameplayBlocked(), 1);
  assert.equal(cg.calls.audio.at(-1), true);
  assert.equal(cg.manager.request(0), -1);
  callbacks.adFinished();
  assert.equal(cg.manager.poll(cgHandle), 4);
  assert.equal(cg.manager.gameplayBlocked(), 0);
  assert.equal(cg.manager.takeReward(cgHandle), 0);
  assert.equal(cgAdCalls.length, 0);

  const pk = makeHarness("poki");
  attachPoki(pk);
  pk.windowObject.PokiSDK.commercialBreak = (onStart) => {
    onStart();
    throw new Error("throw after start");
  };
  const pkHandle = pk.manager.request(0);
  await readyAfterMicrotasks(pk.manager);
  assert.equal(pk.manager.poll(pkHandle), 4);
  assert.equal(pk.manager.gameplayBlocked(), 1);
  assert.equal(pk.calls.audio.at(-1), true);
  assert.equal(pk.manager.request(0), -1);
  pk.manager.dispose();
  assert.equal(pk.manager.gameplayBlocked(), 0);
});


test("Poki unknown-state deadline settles publicly while retaining an issued call until promise settlement", async () => {
  const h = makeHarness("poki");
  const adCalls = attachPoki(h);
  const first = h.manager.request(0);
  await readyAfterMicrotasks(h.manager);
  assert.equal(h.manager.gameplayBlocked(), 1);
  h.clock.advance(5000);
  assert.equal(h.manager.poll(first), 4);
  assert.equal(h.manager.gameplayBlocked(), 0);
  assert.equal(h.manager.request(0), -1);
  h.clock.advance(30000);
  assert.equal(h.manager.poll(first), 4, "unknown state must use one 5-second request budget");
  adCalls[0].result.resolve(undefined);
  await flushPromises();
  assert.equal(h.manager.poll(first), 4);
  assert.ok(h.manager.request(0) > first);
});

test("Poki unknown-state deadline abandons a rewarded handle despite a later boolean success", async () => {
  const h = makeHarness("poki");
  const adCalls = attachPoki(h);
  const handle = h.manager.request(1);
  await readyAfterMicrotasks(h.manager);
  h.clock.advance(5000);
  assert.equal(h.manager.poll(handle), 4);
  adCalls[0].result.resolve(true);
  await flushPromises();
  assert.equal(h.manager.poll(handle), 4);
  assert.equal(h.manager.takeReward(handle), 0);
});


test("GameMonetize waits for SDK_READY, supports only documented break calls, and tracks global pause", async () => {
  const h = makeHarness("gamemonetize");
  const invocations = [];
  h.windowObject.sdk = { showBanner: () => invocations.push("showBanner") };
  assert.equal(typeof h.windowObject.SDK_OPTIONS.onEvent, "function");
  const handle = h.manager.request(0);
  assert.equal(h.manager.poll(handle), 1);
  assert.equal(h.manager.gameplayBlocked(), 1);
  assert.deepEqual(h.calls.audio, [false]);
  assert.deepEqual(h.windowObject.SDK_OPTIONS.gameId, "test-game");
  assert.equal(typeof h.windowObject.SDK_OPTIONS.onEvent, "function");
  assert.deepEqual(h.scripts.map((script) => script.src), ["https://api.gamemonetize.com/sdk.js"]);
  h.windowObject.SDK_OPTIONS.onEvent({ name: "SDK_READY" });
  await flushPromises();
  assert.equal((await readyAfterMicrotasks(h.manager)).available, true);
  assert.deepEqual(invocations, ["showBanner"]);
  assert.equal(h.manager.request(1), 0);

  h.windowObject.SDK_OPTIONS.onEvent({ name: "SDK_GAME_START" });
  assert.equal(h.manager.poll(handle), 1);
  assert.equal(h.manager.gameplayBlocked(), 1);
  h.windowObject.SDK_OPTIONS.onEvent({ name: "SDK_GAME_PAUSE" });
  assert.equal(h.manager.poll(handle), 2);
  assert.equal(h.manager.gameplayBlocked(), 1);
  assert.equal(h.calls.audio.at(-1), true);
  h.windowObject.SDK_OPTIONS.onEvent({ name: "SDK_GAME_START" });
  assert.equal(h.manager.poll(handle), 3);
  assert.equal(h.manager.gameplayBlocked(), 0);
  assert.equal(h.calls.audio.at(-1), false);

  h.windowObject.SDK_OPTIONS.onEvent({ name: "SDK_GAME_PAUSE" });
  assert.equal(h.manager.gameplayBlocked(), 1);
  assert.equal(h.manager.request(0), -1);
  h.windowObject.SDK_OPTIONS.onEvent({ name: "SDK_GAME_START" });
  assert.equal(h.manager.gameplayBlocked(), 0);
  assert.equal(invocations.length, 1);
});

test("GameMonetize unknown-state deadline leaves the issued call orphaned through release and reset", async () => {
  const h = makeHarness("gamemonetize");
  h.windowObject.sdk = { showBanner() {} };
  const handle = h.manager.request(0);
  h.windowObject.SDK_OPTIONS.onEvent({ name: "SDK_READY" });
  await readyAfterMicrotasks(h.manager);
  h.clock.advance(5000);
  assert.equal(h.manager.poll(handle), 4);
  assert.equal(h.manager.gameplayBlocked(), 0);
  h.manager.resetGuest();
  assert.equal(h.manager.poll(handle), 4);
  assert.equal(h.manager.request(0), -1);
  h.windowObject.SDK_OPTIONS.onEvent({ name: "SDK_GAME_PAUSE" });
  assert.equal(h.manager.gameplayBlocked(), 1);
  h.windowObject.SDK_OPTIONS.onEvent({ name: "SDK_GAME_START" });
  assert.equal(h.manager.gameplayBlocked(), 0);
  assert.ok(h.manager.request(0) > handle);
});

test("GameMonetize and GameDistribution capture options before dynamic SDK scripts initialize", async () => {
  let gmEvent;
  const bannerCalls = [];
  const gm = makeHarness("gamemonetize", {
    onAppend(script, windowObject) {
      assert.equal(script.src, "https://api.gamemonetize.com/sdk.js");
      const options = windowObject.SDK_OPTIONS;
      assert.equal(options.gameId, "test-game");
      assert.equal(typeof options.onEvent, "function");
      gmEvent = options.onEvent;
      windowObject.sdk = { showBanner: () => bannerCalls.push("showBanner") };
      gmEvent({ name: "SDK_READY" });
    },
  });
  assert.deepEqual(await gm.manager.ready, { available: true });
  const gmHandle = gm.manager.request(0);
  assert.deepEqual(bannerCalls, ["showBanner"]);
  gmEvent({ name: "SDK_GAME_PAUSE" });
  assert.equal(gm.manager.poll(gmHandle), 2);
  assert.equal(gm.calls.audio.at(-1), true);
  gmEvent({ name: "SDK_GAME_START" });
  assert.equal(gm.manager.poll(gmHandle), 3);
  assert.equal(gm.manager.gameplayBlocked(), 0);

  let gdEvent;
  const showAdResult = deferred();
  const showAdCalls = [];
  const gd = makeHarness("gamedistribution", {
    onAppend(script, windowObject) {
      assert.equal(script.src, "https://html5.api.gamedistribution.com/main.min.js");
      const options = windowObject.GD_OPTIONS;
      assert.equal(options.gameId, "test-game");
      assert.equal(typeof options.onEvent, "function");
      gdEvent = options.onEvent;
      windowObject.gdsdk = { showAd: () => { showAdCalls.push(1); return showAdResult.promise; } };
      gdEvent({ name: "SDK_READY" });
    },
  });
  assert.deepEqual(await gd.manager.ready, { available: true });
  const gdHandle = gd.manager.request(0);
  const gesture = gd.calls.gestures.at(-1);
  gd.windowObject.navigator.userActivation.isActive = true;
  assert.equal(gesture.activate({
    type: "pointerup",
    pointerType: "mouse",
    isPrimary: true,
    button: 0,
    isTrusted: true,
    eventPhase: 2,
    currentTarget: {},
  }), true);
  assert.deepEqual(showAdCalls, [1]);
  gdEvent({ name: "SDK_GAME_PAUSE" });
  assert.equal(gd.manager.poll(gdHandle), 2);
  assert.equal(gd.calls.audio.at(-1), true);
  gdEvent({ name: "SDK_GAME_START" });
  showAdResult.resolve();
  await flushPromises();
  assert.equal(gd.manager.poll(gdHandle), 3);
  assert.equal(gd.manager.gameplayBlocked(), 0);
});

test("GameMonetize waits until global resume before issuing a pre-ready request", async () => {
  const bannerCalls = [];
  const h = makeHarness("gamemonetize", {
    onAppend(_script, windowObject) {
      windowObject.sdk = { showBanner: () => bannerCalls.push("showBanner") };
    },
  });
  const handle = h.manager.request(0);
  assert.equal(h.manager.poll(handle), 1);
  assert.equal(h.manager.gameplayBlocked(), 1);

  h.emitVendorEvent("SDK_GAME_PAUSE");
  assert.equal(h.manager.gameplayBlocked(), 1);
  assert.equal(h.calls.audio.at(-1), true);
  h.emitVendorEvent("SDK_READY");
  assert.equal((await readyAfterMicrotasks(h.manager)).available, true);
  assert.deepEqual(bannerCalls, []);
  assert.equal(h.manager.poll(handle), 1);
  assert.equal(h.calls.audio.at(-1), true);

  h.emitVendorEvent("SDK_GAME_START");
  assert.deepEqual(bannerCalls, ["showBanner"]);
  h.emitVendorEvent("SDK_GAME_START");
  assert.deepEqual(bannerCalls, ["showBanner"]);
  assert.equal(h.manager.poll(handle), 1);
  assert.equal(h.manager.gameplayBlocked(), 1);
  assert.equal(h.calls.audio.at(-1), false);

  h.emitVendorEvent("SDK_GAME_PAUSE");
  assert.equal(h.manager.poll(handle), 2);
  assert.equal(h.calls.audio.at(-1), true);
  h.emitVendorEvent("SDK_GAME_START");
  assert.equal(h.manager.poll(handle), 3);
  assert.equal(h.manager.gameplayBlocked(), 0);
  assert.equal(h.calls.audio.at(-1), false);
});

test("GameDistribution waits until global resume before showing a pre-ready gesture", async () => {
  const showAdResult = deferred();
  const showAdCalls = [];
  const h = makeHarness("gamedistribution", {
    onAppend(_script, windowObject) {
      windowObject.gdsdk = {
        showAd() {
          showAdCalls.push("showAd");
          return showAdResult.promise;
        },
      };
    },
  });
  const handle = h.manager.request(0);
  assert.equal(h.manager.poll(handle), 1);
  h.emitVendorEvent("SDK_GAME_PAUSE");
  assert.equal(h.manager.gameplayBlocked(), 1);
  assert.equal(h.calls.audio.at(-1), true);
  h.emitVendorEvent("SDK_READY");
  assert.equal((await readyAfterMicrotasks(h.manager)).available, true);
  assert.deepEqual(h.calls.gestures, []);
  assert.deepEqual(showAdCalls, []);

  h.emitVendorEvent("SDK_GAME_START");
  assert.equal(h.calls.gestures.length, 1);
  assert.equal(h.calls.gestures[0].visible, true);
  h.emitVendorEvent("SDK_GAME_START");
  assert.equal(h.calls.gestures.length, 1);
  assert.deepEqual(showAdCalls, []);

  h.windowObject.navigator.userActivation.isActive = true;
  assert.equal(h.calls.gestures[0].activate({
    type: "pointerup",
    pointerType: "mouse",
    isPrimary: true,
    button: 0,
    isTrusted: true,
    eventPhase: 2,
    currentTarget: {},
  }), true);
  assert.deepEqual(showAdCalls, ["showAd"]);
  assert.equal(h.manager.poll(handle), 1);
  assert.equal(h.calls.audio.at(-1), false);

  h.emitVendorEvent("SDK_GAME_PAUSE");
  assert.equal(h.manager.poll(handle), 2);
  assert.equal(h.calls.audio.at(-1), true);
  h.emitVendorEvent("SDK_GAME_START");
  assert.equal(h.calls.audio.at(-1), false);
  showAdResult.resolve();
  await flushPromises();
  assert.equal(h.manager.poll(handle), 3);
  assert.equal(h.manager.gameplayBlocked(), 0);
});

test("release and guest reset cancel paused pre-ready requests before global resume", async () => {
  const gmCalls = [];
  const gm = makeHarness("gamemonetize", {
    onAppend(_script, windowObject) {
      windowObject.sdk = { showBanner: () => gmCalls.push("showBanner") };
    },
  });
  const gmHandle = gm.manager.request(0);
  gm.emitVendorEvent("SDK_GAME_PAUSE");
  gm.emitVendorEvent("SDK_READY");
  await readyAfterMicrotasks(gm.manager);
  gm.manager.release(gmHandle);
  gm.emitVendorEvent("SDK_GAME_START");
  assert.deepEqual(gmCalls, []);
  assert.equal(gm.manager.gameplayBlocked(), 0);

  const gdCalls = [];
  const gd = makeHarness("gamedistribution", {
    onAppend(_script, windowObject) {
      windowObject.gdsdk = { showAd: () => gdCalls.push("showAd") };
    },
  });
  const gdHandle = gd.manager.request(0);
  gd.emitVendorEvent("SDK_GAME_PAUSE");
  gd.emitVendorEvent("SDK_READY");
  await readyAfterMicrotasks(gd.manager);
  assert.deepEqual(gd.calls.gestures, []);
  gd.manager.resetGuest();
  gd.emitVendorEvent("SDK_GAME_START");
  assert.deepEqual(gd.calls.gestures, []);
  assert.deepEqual(gdCalls, []);
  assert.equal(gd.manager.poll(gdHandle), 4);
  assert.equal(gd.manager.gameplayBlocked(), 0);
});

test("preloaded GameMonetize and GameDistribution SDK globals fail closed without a registered handler", async () => {
  const gmExistingEvent = () => {};
  const gmExistingOptions = { gameId: "existing-gm-game", onEvent: gmExistingEvent };
  const gmCalls = [];
  const gm = makeHarness("gamemonetize", {
    onBeforeCreate(windowObject) {
      windowObject.SDK_OPTIONS = gmExistingOptions;
      windowObject.sdk = { showBanner() { gmCalls.push("showBanner"); } };
    },
  });
  assert.equal((await gm.manager.ready).available, false);
  assert.equal(gm.scripts.length, 0);
  assert.equal(gm.manager.request(0), 0);
  assert.equal(gm.windowObject.SDK_OPTIONS, gmExistingOptions);
  assert.equal(gm.windowObject.SDK_OPTIONS.onEvent, gmExistingEvent);
  assert.deepEqual(gmCalls, []);

  const gdExistingEvent = () => {};
  const gdExistingOptions = { gameId: "existing-gd-game", onEvent: gdExistingEvent };
  const gdCalls = [];
  const gd = makeHarness("gamedistribution", {
    onBeforeCreate(windowObject) {
      windowObject.GD_OPTIONS = gdExistingOptions;
      windowObject.gdsdk = { showAd() { gdCalls.push("showAd"); } };
    },
  });
  assert.equal((await gd.manager.ready).available, false);
  assert.equal(gd.scripts.length, 0);
  assert.equal(gd.manager.request(0), 0);
  assert.equal(gd.windowObject.GD_OPTIONS, gdExistingOptions);
  assert.equal(gd.windowObject.GD_OPTIONS.onEvent, gdExistingEvent);
  assert.deepEqual(gdCalls, []);
});

test("GameDistribution requires an explicit trusted pointerup and active user activation at showAd", async () => {
  const h = makeHarness("gamedistribution");
  const calls = [];
  const process = deferred();
  h.windowObject.gdsdk = {
    showAd(...args) {
      calls.push({
        args,
        duringPointerup: h.currentEventType === "pointerup",
        trusted: h.currentEvent?.isTrusted,
        active: h.windowObject.navigator.userActivation.isActive,
      });
      return process.promise;
    },
  };
  const handle = h.manager.request(0);
  h.windowObject.GD_OPTIONS.onEvent({ name: "SDK_READY" });
  await readyAfterMicrotasks(h.manager);
  assert.equal(h.manager.poll(handle), 1);
  assert.equal(h.calls.gestures.at(-1).visible, true);
  assert.equal(h.manager.gameplayBlocked(), 1);
  assert.deepEqual(h.calls.audio, [false]);
  assert.equal(calls.length, 0);

  const gesture = h.calls.gestures.at(-1);
  assert.equal(gesture.activate({ type: "pointerup", pointerType: "mouse", isTrusted: false }), false);
  h.windowObject.navigator.userActivation.isActive = true;
  assert.equal(gesture.activate({ type: "keydown", key: "Enter", isTrusted: true }), false);
  assert.equal(gesture.activate({ type: "pointerup", pointerType: "pen", isTrusted: true }), false);
  assert.equal(calls.length, 0);

  const event = {
    type: "pointerup",
    pointerType: "touch",
    isPrimary: true,
    button: 0,
    isTrusted: true,
    eventPhase: 2,
    currentTarget: {},
  };
  h.currentEventType = event.type;
  h.currentEvent = event;
  const result = gesture.activate(event);
  h.currentEventType = null;
  h.currentEvent = null;
  assert.equal(result, true);
  assert.deepEqual(calls, [{ args: [], duringPointerup: true, trusted: true, active: true }]);
  assert.equal(h.calls.gestures.at(-1).visible, false);
  event.eventPhase = 0;
  event.currentTarget = null;
  assert.equal(gesture.activate(event), false);
  assert.equal(calls.length, 1);

  h.windowObject.GD_OPTIONS.onEvent({ name: "SDK_GAME_PAUSE" });
  assert.equal(h.manager.poll(handle), 2);
  assert.equal(h.manager.gameplayBlocked(), 1);
  h.windowObject.GD_OPTIONS.onEvent({ name: "SDK_GAME_START" });
  assert.equal(h.manager.gameplayBlocked(), 1);
  assert.equal(h.calls.audio.at(-1), false);
  assert.equal(h.manager.request(0), -1);
  process.resolve();
  await flushPromises();
  assert.equal(h.manager.poll(handle), 3);
  assert.equal(h.manager.gameplayBlocked(), 0);
  assert.ok(h.manager.request(0) > handle);
});

test("GameDistribution request deadline bounds activation waiting and cancellation issues no SDK call", async () => {
  const h = makeHarness("gamedistribution");
  const calls = [];
  h.windowObject.gdsdk = { showAd: (...args) => calls.push(args) };
  const first = h.manager.request(0);
  h.windowObject.GD_OPTIONS.onEvent({ name: "SDK_READY" });
  await readyAfterMicrotasks(h.manager);
  assert.equal(h.manager.poll(first), 1);
  h.calls.gestures.at(-1).cancel();
  assert.equal(h.manager.poll(first), 4);
  assert.equal(calls.length, 0);
  assert.ok(h.manager.request(0) > first);
  h.manager.resetGuest();

  const second = h.manager.request(0);
  assert.ok(second > first);
  h.clock.advance(4999);
  assert.equal(h.manager.poll(second), 1);
  h.clock.advance(1);
  assert.equal(h.manager.poll(second), 4);
  assert.equal(calls.length, 0);
  assert.equal(h.manager.gameplayBlocked(), 0);
  assert.equal(
    h.calls.diagnostics.at(-1),
    "The ad did not become available before the host deadline.",
  );
});

test("GameDistribution loading progress promotes state once and uses a fixed 30-second deadline", async () => {
  const h = makeHarness("gamedistribution");
  const result = deferred();
  h.windowObject.gdsdk = { showAd: () => result.promise };
  const handle = h.manager.request(1);
  h.emitVendorEvent("AD_METADATA");
  assert.equal(h.manager.poll(handle), 1, "progress without an issued request is ignored");
  h.emitVendorEvent("SDK_READY");
  await readyAfterMicrotasks(h.manager);
  activateGameDistributionGesture(h);

  h.emitVendorEvent("AD_METADATA");
  assert.equal(h.manager.poll(handle), 6);
  assert.equal(h.manager.gameplayBlocked(), 1);
  assert.equal(h.calls.audio.at(-1), false, "loading does not pause audio before playback starts");
  h.clock.advance(15000);
  h.emitVendorEvent("LOADED");
  h.emitVendorEvent("AD_SDK_MANAGER_READY");
  h.clock.advance(14999);
  assert.equal(h.manager.poll(handle), 6);
  h.clock.advance(1);
  assert.equal(h.manager.poll(handle), 4);
  assert.equal(h.manager.gameplayBlocked(), 0);
  h.clock.advance(1);
  h.emitVendorEvent("AD_METADATA");
  assert.equal(h.manager.poll(handle), 4, "stale or repeated progress cannot resurrect a terminal request");

  h.emitVendorEvent("SDK_GAME_PAUSE");
  assert.equal(h.manager.poll(handle), 4);
  assert.equal(h.calls.diagnostics.at(-1), "", "a late actual start clears the stale deadline diagnostic");
  h.emitVendorEvent("SDK_REWARDED_WATCH_COMPLETE");
  assert.equal(h.manager.takeReward(handle), 0, "a late actual start cannot restore abandoned reward proof");
  assert.equal(h.manager.gameplayBlocked(), 1, "a late physical start still pauses gameplay");
  assert.equal(h.calls.audio.at(-1), true, "audio follows the late actual start");
  assert.equal(h.manager.request(0), -1, "the late ad keeps the provider slot reserved");
  h.emitVendorEvent("SDK_GAME_START");
  assert.equal(h.manager.gameplayBlocked(), 0);
  assert.equal(h.calls.audio.at(-1), false);
  assert.equal(h.manager.request(0), -1, "the provider promise still owns the physical slot");
  result.resolve();
  await flushPromises();
  assert.equal(h.manager.poll(handle), 4);
  assert.ok(h.manager.request(0) > handle);
});

test("GameDistribution loading events without a live public request are ignored", async () => {
  const h = makeHarness("gamedistribution");
  for (const event of ["AD_METADATA", "LOADED", "AD_SDK_MANAGER_READY"]) {
    assert.doesNotThrow(() => h.emitVendorEvent(event));
  }
  assert.equal(h.manager.gameplayBlocked(), 0);
  h.manager.dispose();
});

test("GameDistribution actual playback clears the loading deadline and waits for terminal evidence", async () => {
  const h = makeHarness("gamedistribution");
  const result = deferred();
  h.windowObject.gdsdk = { showAd: () => result.promise };
  const handle = h.manager.request(0);
  h.emitVendorEvent("SDK_READY");
  await readyAfterMicrotasks(h.manager);
  activateGameDistributionGesture(h);
  h.emitVendorEvent("AD_METADATA");
  assert.equal(h.manager.poll(handle), 6);
  h.clock.advance(29999);
  assert.equal(h.manager.poll(handle), 6);
  h.emitVendorEvent("SDK_GAME_PAUSE");
  assert.equal(h.manager.poll(handle), 2);
  assert.equal(h.calls.audio.at(-1), true);
  h.clock.advance(30001);
  assert.equal(h.manager.poll(handle), 2, "actual playback has no load deadline");
  assert.equal(h.manager.gameplayBlocked(), 1);
  assert.equal(h.calls.audio.at(-1), true);
  h.emitVendorEvent("SDK_GAME_START");
  result.resolve();
  await flushPromises();
  assert.equal(h.manager.poll(handle), 3);
  assert.equal(h.manager.gameplayBlocked(), 0);
  assert.equal(h.calls.audio.at(-1), false);
});

test("GameDistribution global resume without a matching pause does not settle a loading request", async () => {
  const h = makeHarness("gamedistribution");
  const result = deferred();
  h.windowObject.gdsdk = { showAd: () => result.promise };
  const handle = h.manager.request(0);
  h.emitVendorEvent("SDK_READY");
  await readyAfterMicrotasks(h.manager);
  activateGameDistributionGesture(h);
  h.emitVendorEvent("AD_METADATA");
  assert.equal(h.manager.poll(handle), 6);

  h.emitVendorEvent("SDK_GAME_START");
  h.emitVendorEvent("SDK_GAME_START");
  assert.equal(h.manager.poll(handle), 6, "global resume without a preceding pause is not terminal evidence");
  assert.equal(h.manager.gameplayBlocked(), 1);
  assert.equal(h.manager.request(0), -1, "the unresolved provider call retains its slot");

  h.clock.advance(29999);
  assert.equal(h.manager.poll(handle), 6);
  h.clock.advance(1);
  assert.equal(h.manager.poll(handle), 4, "the fixed loading deadline still expires the request");
  assert.equal(h.manager.gameplayBlocked(), 0);
  assert.equal(h.manager.request(0), -1, "the issued physical call remains reserved after public timeout");
  h.emitVendorEvent("AD_SDK_CANCELED");
  assert.ok(h.manager.request(0) > handle, "documented cancellation retires the physical reservation");
  result.resolve();
  await flushPromises();
  h.manager.dispose();
});

test("GameDistribution no-fill and provider errors resume immediately without rewarding", async () => {
  for (const terminal of ["AD_ERROR", "SDK_ERROR", "AD_SDK_CANCELED"]) {
    const h = makeHarness("gamedistribution");
    const result = deferred();
    h.windowObject.gdsdk = { showAd: () => result.promise };
    const handle = h.manager.request(1);
    h.emitVendorEvent("SDK_READY");
    await readyAfterMicrotasks(h.manager);
    activateGameDistributionGesture(h);
    h.emitVendorEvent("AD_METADATA");
    assert.equal(h.manager.poll(handle), 6);
    h.emitVendorEvent(terminal);
    assert.equal(h.manager.poll(handle), 4, `${terminal} should fail a request with no playback`);
    assert.equal(h.manager.gameplayBlocked(), 0, `${terminal} should resume gameplay immediately`);
    assert.equal(h.manager.takeReward(handle), 0);
    assert.ok(h.manager.request(0) > handle, `${terminal} should release the physical reservation`);
    result.resolve();
    await flushPromises();
    assert.equal(h.manager.poll(handle), 4);
    h.manager.dispose();
  }
});

test("GameDistribution terminal errors preserve prior reward proof exactly once", async () => {
  for (const terminal of ["AD_ERROR", "SDK_ERROR"]) {
    const h = makeHarness("gamedistribution");
    const result = deferred();
    h.windowObject.gdsdk = { showAd: () => result.promise };
    const handle = h.manager.request(1);
    h.emitVendorEvent("SDK_READY");
    await readyAfterMicrotasks(h.manager);
    activateGameDistributionGesture(h);
    h.emitVendorEvent("AD_METADATA");
    assert.equal(h.manager.poll(handle), 6);

    h.emitVendorEvent("SDK_REWARDED_WATCH_COMPLETE");
    h.emitVendorEvent(terminal);
    assert.equal(h.manager.poll(handle), 4, `${terminal} should fail the public request`);
    assert.equal(h.manager.gameplayBlocked(), 0, `${terminal} should resume gameplay immediately`);
    assert.equal(h.manager.takeReward(handle), 1, `${terminal} must preserve previously verified proof`);
    assert.equal(h.manager.takeReward(handle), 0, `${terminal} proof remains single-use`);

    result.resolve();
    await flushPromises();
    assert.equal(h.manager.poll(handle), 4, "promise settlement cannot revive the failed request");
    h.manager.dispose();
  }
});

test("GameDistribution cancellation after playback preserves the SDK pause until SDK_GAME_START", async () => {
  const h = makeHarness("gamedistribution");
  const result = deferred();
  h.windowObject.gdsdk = { showAd: () => result.promise };
  const handle = h.manager.request(0);
  h.emitVendorEvent("SDK_READY");
  await readyAfterMicrotasks(h.manager);
  activateGameDistributionGesture(h);
  h.emitVendorEvent("AD_METADATA");
  h.emitVendorEvent("SDK_GAME_PAUSE");
  assert.equal(h.manager.poll(handle), 2);
  assert.equal(h.manager.gameplayBlocked(), 1);
  assert.equal(h.calls.audio.at(-1), true);

  h.emitVendorEvent("AD_SDK_CANCELED");
  assert.equal(h.manager.poll(handle), 3);
  assert.equal(h.manager.gameplayBlocked(), 1, "IMA cancellation must not clear the SDK pause reason");
  assert.equal(h.calls.audio.at(-1), true);
  assert.equal(h.manager.request(0), -1);
  h.emitVendorEvent("SDK_GAME_START");
  assert.equal(h.manager.gameplayBlocked(), 0);
  assert.equal(h.calls.audio.at(-1), false);
  assert.ok(h.manager.request(0) > handle);
  result.resolve();
  await flushPromises();
  h.manager.dispose();
});

test("release before GameDistribution SDK issue cancels its reservation and gesture closure", async () => {
  const h = makeHarness("gamedistribution");
  h.windowObject.gdsdk = { showAd() { throw new Error("must not be called"); } };
  const first = h.manager.request(0);
  h.windowObject.GD_OPTIONS.onEvent({ name: "SDK_READY" });
  await readyAfterMicrotasks(h.manager);
  const oldGesture = h.calls.gestures.at(-1);
  assert.equal(h.manager.gameplayBlocked(), 1);
  h.manager.release(first);
  assert.equal(h.manager.gameplayBlocked(), 0);
  assert.equal(h.calls.gestures.at(-1).visible, false);
  assert.equal(oldGesture.activate({
    type: "pointerup",
    pointerType: "mouse",
    isTrusted: true,
    eventPhase: 2,
    currentTarget: {},
  }), false);
  assert.ok(h.manager.request(0) > first);
});


test("GameDistribution promise and global pause remain separate until both terminal facts arrive", async () => {
  const h = makeHarness("gamedistribution");
  const result = deferred();
  h.windowObject.gdsdk = { showAd: () => result.promise };
  const handle = h.manager.request(0);
  h.windowObject.GD_OPTIONS.onEvent({ name: "SDK_READY" });
  await readyAfterMicrotasks(h.manager);
  const event = {
    type: "pointerup",
    pointerType: "mouse",
    isPrimary: true,
    button: 0,
    isTrusted: true,
    eventPhase: 2,
    currentTarget: {},
  };
  h.windowObject.navigator.userActivation.isActive = true;
  h.calls.gestures.at(-1).activate(event);
  h.windowObject.GD_OPTIONS.onEvent({ name: "SDK_GAME_PAUSE" });
  result.resolve();
  await flushPromises();
  assert.equal(h.manager.poll(handle), 3);
  assert.equal(h.manager.gameplayBlocked(), 1);
  assert.equal(h.manager.request(0), -1);
  h.windowObject.GD_OPTIONS.onEvent({ name: "SDK_GAME_START" });
  assert.equal(h.manager.gameplayBlocked(), 0);
  assert.ok(h.manager.request(0) > handle);
});

test("GameDistribution rewarded requests invoke showAd from trusted pointerup and require proof", async () => {
  const h = makeHarness("gamedistribution");
  const result = deferred();
  const invocations = [];
  h.windowObject.gdsdk = { showAd: (...args) => { invocations.push(args); return result.promise; } };
  const handle = h.manager.request(1);
  assert.ok(handle > 0);
  h.windowObject.GD_OPTIONS.onEvent({ name: "SDK_READY" });
  await readyAfterMicrotasks(h.manager);
  assert.equal(h.manager.poll(handle), 1);
  assert.equal(h.calls.gestures.at(-1).visible, true);
  h.windowObject.GD_OPTIONS.onEvent({ name: "SDK_REWARDED_WATCH_COMPLETE" });
  assert.equal(h.manager.takeReward(handle), 0, "a request awaiting its gesture cannot receive proof");

  const event = {
    type: "pointerup",
    pointerType: "mouse",
    isPrimary: true,
    button: 0,
    isTrusted: true,
    eventPhase: 2,
    currentTarget: {},
  };
  h.windowObject.navigator.userActivation.isActive = true;
  assert.equal(h.calls.gestures.at(-1).activate(event), true);
  assert.deepEqual(invocations, [["rewarded"]]);
  h.windowObject.GD_OPTIONS.onEvent({ name: "SDK_GAME_PAUSE" });
  h.windowObject.GD_OPTIONS.onEvent({ name: "SDK_GAME_START" });
  result.resolve();
  await flushPromises();
  assert.equal(h.manager.poll(handle), 3);
  assert.equal(h.manager.gameplayBlocked(), 0);
  assert.equal(h.manager.takeReward(handle), 0, "resume and promise fulfillment are not reward proof");

  h.windowObject.GD_OPTIONS.onEvent({ name: "SDK_REWARDED_WATCH_COMPLETE" });
  h.windowObject.GD_OPTIONS.onEvent({ name: "SDK_REWARDED_WATCH_COMPLETE" });
  assert.equal(h.manager.takeReward(handle), 1);
  assert.equal(h.manager.takeReward(handle), 0);
  h.windowObject.GD_OPTIONS.onEvent({ name: "SDK_REWARDED_WATCH_COMPLETE" });
  assert.equal(h.manager.takeReward(handle), 0, "duplicate proof cannot replenish a consumed reward");
});

test("GameDistribution proof received before promise rejection remains consumable once", async () => {
  const h = makeHarness("gamedistribution");
  const result = deferred();
  h.windowObject.gdsdk = { showAd: () => result.promise };
  const handle = h.manager.request(1);
  h.windowObject.GD_OPTIONS.onEvent({ name: "SDK_READY" });
  await readyAfterMicrotasks(h.manager);
  h.windowObject.navigator.userActivation.isActive = true;
  h.calls.gestures.at(-1).activate({
    type: "pointerup", pointerType: "touch", isPrimary: true, button: 0,
    isTrusted: true, eventPhase: 2, currentTarget: {},
  });

  h.windowObject.GD_OPTIONS.onEvent({ name: "SDK_REWARDED_WATCH_COMPLETE" });
  result.reject(new Error("provider rejected after completion proof"));
  await flushPromises();
  assert.equal(h.manager.poll(handle), 4);
  assert.equal(h.manager.takeReward(handle), 1);
  assert.equal(h.manager.takeReward(handle), 0);
});

test("GameDistribution promise rejection without a completion proof grants no reward", async () => {
  const h = makeHarness("gamedistribution");
  const result = deferred();
  h.windowObject.gdsdk = { showAd: () => result.promise };
  const handle = h.manager.request(1);
  h.windowObject.GD_OPTIONS.onEvent({ name: "SDK_READY" });
  await readyAfterMicrotasks(h.manager);
  h.windowObject.navigator.userActivation.isActive = true;
  h.calls.gestures.at(-1).activate({
    type: "pointerup", pointerType: "mouse", isPrimary: true, button: 0,
    isTrusted: true, eventPhase: 2, currentTarget: {},
  });

  result.reject(new Error("provider rejection is not proof"));
  await flushPromises();
  assert.equal(h.manager.poll(handle), 4);
  assert.equal(h.manager.takeReward(handle), 0);
});

test("GameDistribution verified proof survives a synchronous showAd throw without being synthesized", async () => {
  const withProof = makeHarness("gamedistribution");
  withProof.windowObject.gdsdk = {
    showAd() {
      withProof.windowObject.GD_OPTIONS.onEvent({ name: "SDK_REWARDED_WATCH_COMPLETE" });
      throw new Error("provider threw after reporting completion proof");
    },
  };
  const verified = withProof.manager.request(1);
  withProof.windowObject.GD_OPTIONS.onEvent({ name: "SDK_READY" });
  await readyAfterMicrotasks(withProof.manager);
  withProof.windowObject.navigator.userActivation.isActive = true;
  withProof.calls.gestures.at(-1).activate({
    type: "pointerup", pointerType: "mouse", isPrimary: true, button: 0,
    isTrusted: true, eventPhase: 2, currentTarget: {},
  });
  assert.equal(withProof.manager.poll(verified), 4);
  assert.equal(withProof.manager.takeReward(verified), 1);
  assert.equal(withProof.manager.takeReward(verified), 0);

  const withoutProof = makeHarness("gamedistribution");
  withoutProof.windowObject.gdsdk = { showAd() { throw new Error("provider throw without proof"); } };
  const unverified = withoutProof.manager.request(1);
  withoutProof.windowObject.GD_OPTIONS.onEvent({ name: "SDK_READY" });
  await readyAfterMicrotasks(withoutProof.manager);
  withoutProof.windowObject.navigator.userActivation.isActive = true;
  withoutProof.calls.gestures.at(-1).activate({
    type: "pointerup", pointerType: "touch", isPrimary: true, button: 0,
    isTrusted: true, eventPhase: 2, currentTarget: {},
  });
  assert.equal(withoutProof.manager.poll(unverified), 4);
  assert.equal(withoutProof.manager.takeReward(unverified), 0);
  withoutProof.windowObject.GD_OPTIONS.onEvent({ name: "SDK_REWARDED_WATCH_COMPLETE" });
  assert.equal(withoutProof.manager.takeReward(unverified), 1, "a later verified proof still belongs to the current uncertain call");
});

test("GameDistribution proof may arrive after fulfillment until a newer rewarded call is issued", async () => {
  const h = makeHarness("gamedistribution");
  const results = [deferred(), deferred()];
  const invocations = [];
  h.windowObject.gdsdk = {
    showAd: (...args) => {
      invocations.push(args);
      return results[invocations.length - 1].promise;
    },
  };
  const first = h.manager.request(1);
  h.windowObject.GD_OPTIONS.onEvent({ name: "SDK_READY" });
  await readyAfterMicrotasks(h.manager);
  h.windowObject.navigator.userActivation.isActive = true;
  h.calls.gestures.at(-1).activate({
    type: "pointerup", pointerType: "mouse", isPrimary: true, button: 0,
    isTrusted: true, eventPhase: 2, currentTarget: {},
  });
  results[0].resolve();
  await flushPromises();
  assert.equal(h.manager.poll(first), 3);

  const second = h.manager.request(1);
  assert.ok(second > first);
  h.windowObject.GD_OPTIONS.onEvent({ name: "SDK_REWARDED_WATCH_COMPLETE" });
  assert.equal(h.manager.takeReward(first), 1, "an accepted but unissued gesture does not replace the current owner");
  assert.equal(h.manager.takeReward(second), 0);

  h.calls.gestures.at(-1).activate({
    type: "pointerup", pointerType: "mouse", isPrimary: true, button: 0,
    isTrusted: true, eventPhase: 2, currentTarget: {},
  });
  assert.deepEqual(invocations, [["rewarded"], ["rewarded"]]);
  h.windowObject.GD_OPTIONS.onEvent({ name: "SDK_REWARDED_WATCH_COMPLETE" });
  assert.equal(h.manager.takeReward(first), 0);
  assert.equal(h.manager.takeReward(second), 1, "the latest issued rewarded call owns subsequent untagged proof");
});

test("GameDistribution ignores reward proof for midgame calls and abandons it on release, reset, or timeout", async () => {
  const midgame = makeHarness("gamedistribution");
  const midgameCalls = [];
  midgame.windowObject.gdsdk = { showAd: (...args) => midgameCalls.push(args) };
  const midgameHandle = midgame.manager.request(0);
  midgame.windowObject.GD_OPTIONS.onEvent({ name: "SDK_READY" });
  await readyAfterMicrotasks(midgame.manager);
  midgame.windowObject.navigator.userActivation.isActive = true;
  midgame.calls.gestures.at(-1).activate({
    type: "pointerup", pointerType: "mouse", isPrimary: true, button: 0,
    isTrusted: true, eventPhase: 2, currentTarget: {},
  });
  assert.deepEqual(midgameCalls, [[]]);
  midgame.windowObject.GD_OPTIONS.onEvent({ name: "SDK_REWARDED_WATCH_COMPLETE" });
  assert.equal(midgame.manager.takeReward(midgameHandle), 0);

  for (const cleanup of ["release", "reset", "timeout"]) {
    const h = makeHarness("gamedistribution");
    const result = deferred();
    h.windowObject.gdsdk = { showAd: () => result.promise };
    const handle = h.manager.request(1);
    h.windowObject.GD_OPTIONS.onEvent({ name: "SDK_READY" });
    await readyAfterMicrotasks(h.manager);
    h.windowObject.navigator.userActivation.isActive = true;
    h.calls.gestures.at(-1).activate({
      type: "pointerup", pointerType: "touch", isPrimary: true, button: 0,
      isTrusted: true, eventPhase: 2, currentTarget: {},
    });
    h.windowObject.GD_OPTIONS.onEvent({ name: "SDK_REWARDED_WATCH_COMPLETE" });
    if (cleanup === "release") h.manager.release(handle);
    else if (cleanup === "reset") h.manager.resetGuest();
    else h.clock.advance(5000);
    assert.equal(h.manager.takeReward(handle), 0, `${cleanup} abandons unconsumed proof`);
  }
});

test("GameDistribution start followed by a synchronous throw remains blocked until SDK_GAME_START", async () => {
  const h = makeHarness("gamedistribution");
  h.windowObject.gdsdk = {
    showAd() {
      h.windowObject.GD_OPTIONS.onEvent({ name: "SDK_GAME_PAUSE" });
      throw new Error("throw after provider pause");
    },
  };
  const handle = h.manager.request(0);
  h.windowObject.GD_OPTIONS.onEvent({ name: "SDK_READY" });
  await readyAfterMicrotasks(h.manager);
  h.windowObject.navigator.userActivation.isActive = true;
  h.calls.gestures.at(-1).activate({
    type: "pointerup",
    pointerType: "mouse",
    isTrusted: true,
    eventPhase: 2,
    currentTarget: {},
  });
  assert.equal(h.manager.poll(handle), 4);
  assert.equal(h.manager.gameplayBlocked(), 1);
  assert.equal(h.calls.audio.at(-1), true);
  assert.equal(h.manager.request(0), -1);
  h.windowObject.GD_OPTIONS.onEvent({ name: "SDK_GAME_START" });
  assert.equal(h.manager.gameplayBlocked(), 0);
  assert.equal(h.calls.audio.at(-1), false);
  assert.ok(h.manager.request(0) > handle);
});

test("script failures, SDK errors, missing APIs, and invalid game IDs fail closed", async () => {
  const crazy = makeHarness("crazygames");
  const waiting = crazy.manager.request(0);
  crazy.scripts[0].onerror?.(new Error("script failed"));
  await flushPromises();
  assert.equal((await readyAfterMicrotasks(crazy.manager)).available, false);
  assert.equal(crazy.manager.poll(waiting), 5);

  const appendThrow = makeHarness("poki", {
    onAppend() {
      throw new Error("append failed");
    },
  });
  assert.equal((await readyAfterMicrotasks(appendThrow.manager)).available, false);
  assert.equal(appendThrow.manager.request(0), 0);

  const gmError = makeHarness("gamemonetize");
  gmError.windowObject.sdk = { showBanner() {} };
  const gmWaiting = gmError.manager.request(0);
  gmError.windowObject.SDK_OPTIONS.onEvent({ name: "SDK_ERROR" });
  assert.equal((await readyAfterMicrotasks(gmError.manager)).available, false);
  assert.equal(gmError.manager.poll(gmWaiting), 5);

  const gdMissing = makeHarness("gamedistribution");
  const gdWaiting = gdMissing.manager.request(0);
  gdMissing.windowObject.GD_OPTIONS.onEvent({ name: "SDK_READY" });
  assert.equal((await readyAfterMicrotasks(gdMissing.manager)).available, false);
  assert.equal(gdMissing.manager.poll(gdWaiting), 5);

  const invalid = makeHarness("gamemonetize", { game_id: "" });
  assert.equal((await invalid.manager.ready).available, false);
  assert.equal(invalid.manager.request(0), 0);
  assert.deepEqual(invalid.scripts, []);
});

test("resetGuest invalidates guest handles, cancels unissued work, and preserves issued physical calls", async () => {
  const gd = makeHarness("gamedistribution");
  const gdCalls = [];
  gd.windowObject.gdsdk = { showAd: (...args) => gdCalls.push(args) };
  const unissued = gd.manager.request(0);
  gd.windowObject.GD_OPTIONS.onEvent({ name: "SDK_READY" });
  await readyAfterMicrotasks(gd.manager);
  const oldGesture = gd.calls.gestures.at(-1);
  gd.manager.resetGuest();
  assert.equal(gd.manager.poll(unissued), 4);
  assert.equal(oldGesture.activate({ type: "pointerup", pointerType: "mouse", isTrusted: true }), false);
  assert.equal(gdCalls.length, 0);
  const next = gd.manager.request(0);
  assert.ok(next > unissued);

  const cg = makeHarness("crazygames");
  const adCalls = attachCrazy(cg);
  const issued = cg.manager.request(1);
  await readyAfterMicrotasks(cg.manager);
  adCalls[0].callbacks.adStarted();
  cg.manager.resetGuest();
  assert.equal(cg.manager.poll(issued), 4);
  assert.equal(cg.manager.takeReward(issued), 0);
  assert.equal(cg.manager.request(0), -1);
  adCalls[0].callbacks.adFinished();
  assert.equal(cg.manager.gameplayBlocked(), 0);
  const afterTerminal = cg.manager.request(0);
  assert.ok(afterTerminal > issued);
  adCalls[1].callbacks.adFinished();
  for (let i = 0; i < 20; i += 1) cg.manager.resetGuest();
  const afterResets = cg.manager.request(0);
  assert.ok(afterResets > afterTerminal);
});

test("capacity is 16 unreleased handles; releasing frees capacity without reusing IDs", async () => {
  const h = makeHarness("crazygames");
  const adCalls = attachCrazy(h);
  await readyAfterMicrotasks(h.manager);
  const handles = [];
  for (let i = 0; i < 16; i += 1) {
    const handle = h.manager.request(0);
    handles.push(handle);
    adCalls[i].callbacks.adFinished();
  }
  assert.equal(handles.length, 16);
  assert.equal(new Set(handles).size, 16);
  assert.equal(h.manager.request(0), -2);
  h.manager.release(handles[0]);
  const next = h.manager.request(0);
  assert.equal(next, handles[15] + 1);
  assert.equal(h.manager.poll(handles[0]), 4);
  assert.equal(h.manager.takeReward(handles[0]), 0);
});

test("dispose tears down callbacks and makes all handles inaccessible", async () => {
  const h = makeHarness("crazygames");
  const adCalls = attachCrazy(h);
  const handle = h.manager.request(0);
  await readyAfterMicrotasks(h.manager);
  h.manager.dispose();
  adCalls[0].callbacks.adStarted();
  assert.equal(h.manager.poll(handle), 4);
  assert.equal(h.manager.request(0), 0);
  assert.equal(h.manager.gameplayBlocked(), 0);
  assert.equal(h.manager.takeReward(handle), 0);
});
