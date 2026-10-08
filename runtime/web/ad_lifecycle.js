(function registerStasisAdLifecycle(globalObject) {
  "use strict";

  const AdState = Object.freeze({
    Idle: 0,
    Requesting: 1,
    Playing: 2,
    Finished: 3,
    Failed: 4,
    Unavailable: 5,
  });
  const AdKind = Object.freeze({ Midgame: 0, Rewarded: 1 });
  const LifecycleEvent = Object.freeze({
    LoadingStarted: 1,
    LoadingFinished: 2,
    GameplayStarted: 3,
    GameplayStopped: 4,
  });
  const MAX_HANDLE = 0x7fffffff;
  const MAX_UNRELEASED_HANDLES = 16;
  const INIT_TIMEOUT_MS = 8000;
  const NO_START_TIMEOUT_MS = 15000;
  const GD_GESTURE_TIMEOUT_MS = 60000;
  const MAX_PENDING_LIFECYCLE_EVENTS = 8;

  function createAdLifecycle(options = {}) {
    const provider = String(options.provider || "none").toLowerCase();
    const gameId = typeof options.game_id === "string" ? options.game_id : "";
    const windowObject = options.windowObject || globalObject;
    const documentObject = options.documentObject || globalObject.document;
    const setTimeoutFn = options.setTimeoutFn || globalObject.setTimeout?.bind(globalObject);
    const clearTimeoutFn = options.clearTimeoutFn || globalObject.clearTimeout?.bind(globalObject);
    const onGameplayBlocked = options.onGameplayBlocked;
    const onAudioPaused = options.onAudioPaused;
    const onGestureNeeded = options.onGestureNeeded;
    const onDiagnostic = options.onDiagnostic;

    let disposed = false;
    let readySettled = false;
    let readyResult = null;
    let readyTimer = null;
    let readyResolve;
    let nextHandle = 1;
    let providerPaused = false;
    let localPauseCall = null;
    let activeCall = null;
    // GameDistribution reports reward completion globally without a request ID.
    // Keep only the latest issued rewarded call as the local attribution owner.
    let gameDistributionRewardCall = null;
    let gestureTimer = null;
    let gestureCall = null;
    let lastBlocked = null;
    let lastAudioPaused = null;
    let lastGameplayLifecycleEvent = null;
    const records = new Map();
    const pendingLifecycleEvents = [];

    const ready = new Promise((resolve) => {
      readyResolve = resolve;
    });

    function safeCallback(callback, value) {
      if (typeof callback !== "function") return;
      try {
        callback(value);
      } catch (_error) {
        // Host UI callbacks cannot be allowed to break SDK state transitions.
      }
    }

    function diagnostic(message) {
      if (!disposed) safeCallback(onDiagnostic, message);
    }

    function isAudioPaused() {
      return providerPaused || localPauseCall !== null;
    }

    function isGameplayBlocked() {
      if (providerPaused || localPauseCall !== null) return true;
      return Boolean(
        activeCall &&
          isPublicRecord(activeCall) &&
          (activeCall.state === AdState.Requesting || activeCall.state === AdState.Playing),
      );
    }

    function syncHostState(force = false) {
      if (disposed) return;
      const blocked = isGameplayBlocked();
      const audioPaused = isAudioPaused();
      if (force || blocked !== lastBlocked) {
        lastBlocked = blocked;
        safeCallback(onGameplayBlocked, blocked);
      }
      if (force || audioPaused !== lastAudioPaused) {
        lastAudioPaused = audioPaused;
        safeCallback(onAudioPaused, audioPaused);
      }
    }

    function schedule(callback, delay) {
      if (typeof setTimeoutFn !== "function") return null;
      try {
        return setTimeoutFn(callback, delay);
      } catch (_error) {
        return null;
      }
    }

    function clearTimer(timer) {
      if (timer === null || timer === undefined || typeof clearTimeoutFn !== "function") return;
      try {
        clearTimeoutFn(timer);
      } catch (_error) {
        // Timer cleanup is best effort; callbacks also check their owner state.
      }
    }

    function settleReady(available, message) {
      if (readySettled || disposed) return;
      readySettled = true;
      clearTimer(readyTimer);
      readyTimer = null;
      readyResult = message ? { available: Boolean(available), diagnostic: message } : { available: Boolean(available) };
      if (message) diagnostic(message);
      readyResolve(readyResult);

      if (!available) {
        pendingLifecycleEvents.length = 0;
        lastGameplayLifecycleEvent = null;
        const call = activeCall;
        if (call && !call.issued) {
          setPublicState(call, AdState.Unavailable);
          cancelUnissuedCall(call);
        }
        return;
      }

      flushPendingLifecycleEvents();
      if (activeCall && !activeCall.issued) beginCall(activeCall);
    }

    function loadScript(src, id, onLoad, onError) {
      if (disposed) return;
      if (!documentObject || typeof documentObject.createElement !== "function") {
        onError();
        return;
      }
      let script;
      try {
        script = documentObject.createElement("script");
        script.async = true;
        script.id = id;
        script.src = src;
        script.onload = () => {
          if (!disposed) onLoad();
        };
        script.onerror = () => {
          if (!disposed) onError();
        };
        const parent = documentObject.head || documentObject.documentElement;
        if (!parent || typeof parent.appendChild !== "function") {
          onError();
          return;
        }
        parent.appendChild(script);
      } catch (_error) {
        onError();
      }
    }

    function clearNoStartTimer(call) {
      clearTimer(call.noStartTimer);
      call.noStartTimer = null;
    }

    function clearGestureTimer() {
      clearTimer(gestureTimer);
      gestureTimer = null;
    }

    function updateGesture(visible, call) {
      if (disposed) return;
      if (visible) {
        gestureCall = call;
        const activate = (event) => activateGameDistribution(call, event);
        const cancel = () => cancelGameDistributionWait(call, "The ad request was canceled before it started.");
        safeCallback(onGestureNeeded, { visible: true, label: "Watch ad", activate, cancel });
        return;
      }
      if (gestureCall === call || call === null) gestureCall = null;
      safeCallback(onGestureNeeded, {
        visible: false,
        label: "",
        activate: () => false,
        cancel: () => {},
      });
    }

    function setPublicState(call, state) {
      if (!call || !call.guestOwned || records.get(call.handle) !== call) return false;
      if (call.state === AdState.Finished || call.state === AdState.Failed || call.state === AdState.Unavailable) return false;
      call.state = state;
      syncHostState();
      return true;
    }

    function isPublicRecord(call) {
      return Boolean(call && call.guestOwned && records.get(call.handle) === call);
    }

    function grantRewardProof(call) {
      if (
        !call ||
        call.kind !== AdKind.Rewarded ||
        !isPublicRecord(call) ||
        call.rewardConsumed ||
        call.rewardAbandoned
      ) return;
      call.rewardAvailable = true;
    }

    function clearLocalPause(call) {
      if (localPauseCall === call) {
        localPauseCall = null;
        syncHostState();
      }
    }

    function retireCall(call) {
      if (!call || call.physicalTerminal) return;
      call.physicalTerminal = true;
      clearNoStartTimer(call);
      clearGestureTimer();
      if (gestureCall === call) updateGesture(false, call);
      clearLocalPause(call);
      if (activeCall === call) activeCall = null;
      syncHostState();
    }

    function cancelUnissuedCall(call) {
      if (!call || call.issued || call.physicalTerminal) return;
      clearGestureTimer();
      if (gestureCall === call) updateGesture(false, call);
      call.physicalTerminal = true;
      if (activeCall === call) activeCall = null;
      syncHostState();
    }

    function failBeforeIssue(call, state, message) {
      if (!call || call.issued || call.physicalTerminal) return;
      setPublicState(call, state);
      if (message) diagnostic(message);
      cancelUnissuedCall(call);
    }

    function failIssuedCall(call, message) {
      if (!call || call.physicalTerminal) return;
      call.rewardAbandoned = true;
      call.rewardAvailable = false;
      setPublicState(call, AdState.Failed);
      if (message) diagnostic(message);
      retireCall(call);
    }

    function failIssuedUncertain(call, message) {
      if (!call || call.physicalTerminal) return;
      if (call.provider === "gamedistribution") {
        // A synchronous throw does not revoke a global completion proof already emitted
        // by the SDK, and the uncertain physical call remains its attribution owner.
        call.promiseSettled = true;
      } else {
        call.rewardAbandoned = true;
        call.rewardAvailable = false;
      }
      setPublicState(call, AdState.Failed);
      if (message) diagnostic(message);
      clearNoStartTimer(call);
      if (call.provider === "gamedistribution" && call.pauseSeen && call.resumeSeen) {
        maybeRetireGameDistribution(call);
      }
      // A synchronous throw does not prove an issued provider call is no longer visible.
      // Keep its private reservation until documented terminal evidence or host disposal.
    }

    function noStartExpired(call) {
      call.noStartTimer = null;
      if (disposed || activeCall !== call || call.physicalTerminal || call.started) return;
      call.rewardAbandoned = true;
      call.rewardAvailable = false;
      if (gameDistributionRewardCall === call) gameDistributionRewardCall = null;
      setPublicState(call, AdState.Failed);
      diagnostic("The ad did not start before the host deadline.");
      // The public token is terminal, but the issued provider call still owns the slot.
    }

    function beginNoStartTimer(call) {
      clearNoStartTimer(call);
      call.noStartTimer = schedule(() => noStartExpired(call), NO_START_TIMEOUT_MS);
    }

    function markActualStart(call, useLocalPause) {
      if (!call || call.physicalTerminal || call.started) return;
      call.started = true;
      clearNoStartTimer(call);
      if (useLocalPause) {
        localPauseCall = call;
        syncHostState();
      }
      setPublicState(call, AdState.Playing);
    }

    function markActualEnd(call, terminalState = AdState.Finished) {
      if (!call || call.physicalTerminal) return;
      setPublicState(call, terminalState);
      retireCall(call);
    }

    function onCrazyGamesStart(call) {
      if (disposed || activeCall !== call || call.provider !== "crazygames" || call.physicalTerminal) return;
      markActualStart(call, true);
    }

    function onCrazyGamesFinish(call) {
      if (disposed || call.provider !== "crazygames" || call.physicalTerminal) return;
      // CrazyGames documents adFinished as successful completion and reward proof.
      if (call.kind === AdKind.Rewarded) grantRewardProof(call);
      markActualEnd(call, AdState.Finished);
    }

    function onCrazyGamesError(call) {
      if (disposed || call.provider !== "crazygames" || call.physicalTerminal) return;
      failIssuedCall(call, "The ad provider could not show this ad.");
    }

    function onPokiStart(call) {
      if (disposed || activeCall !== call || call.provider !== "poki" || call.physicalTerminal) return;
      markActualStart(call, true);
    }

    function onPokiResolved(call, result) {
      if (disposed || call.provider !== "poki" || call.physicalTerminal) return;
      if (call.kind === AdKind.Rewarded && result === true) grantRewardProof(call);
      markActualEnd(call, AdState.Finished);
    }

    function onPokiRejected(call) {
      if (disposed || call.provider !== "poki" || call.physicalTerminal) return;
      failIssuedCall(call, "The ad provider could not complete this break.");
    }

    function gameDistributionCanActivate(event) {
      if (!event || event.type !== "pointerup" || event.isTrusted !== true) return false;
      if (!Number.isInteger(event.eventPhase) || event.eventPhase <= 0 || !event.currentTarget) return false;
      if (event.pointerType !== "mouse" && event.pointerType !== "touch") return false;
      if (event.isPrimary === false || (event.button !== undefined && event.button !== 0)) return false;
      return windowObject?.navigator?.userActivation?.isActive === true;
    }

    function activateGameDistribution(call, event) {
      if (
        disposed ||
        activeCall !== call ||
        call.provider !== "gamedistribution" ||
        call.issued ||
        call.physicalTerminal ||
        providerPaused ||
        !gameDistributionCanActivate(event)
      ) {
        return false;
      }
      invokeGameDistribution(call);
      return call.issued;
    }

    function cancelGameDistributionWait(call, message) {
      if (disposed || activeCall !== call || call.issued || call.physicalTerminal) return;
      failBeforeIssue(call, AdState.Failed, message);
    }

    function onGameMonetizeEvent(event) {
      if (disposed) return;
      const eventName = typeof event?.name === "string" ? event.name : "";
      if (eventName === "SDK_READY") {
        const sdk = windowObject.sdk;
        if (!sdk || typeof sdk.showBanner !== "function") {
          settleReady(false, "GameMonetize SDK is missing the documented showBanner API.");
        } else {
          settleReady(true);
        }
        return;
      }
      if (eventName === "SDK_ERROR") {
        if (!readySettled) settleReady(false, "GameMonetize SDK initialization failed.");
        else diagnostic("GameMonetize reported an SDK error.");
        return;
      }
      if (eventName === "SDK_GAME_PAUSE") {
        providerPaused = true;
        const call = activeCall;
        if (call && call.provider === "gamemonetize" && call.issued && !call.physicalTerminal) {
          call.pauseSeen = true;
          markActualStart(call, false);
        }
        syncHostState();
        return;
      }
      if (eventName === "SDK_GAME_START") {
        providerPaused = false;
        const call = activeCall;
        if (call && call.provider === "gamemonetize" && call.pauseSeen && !call.physicalTerminal) {
          call.resumeSeen = true;
          markActualEnd(call, AdState.Finished);
        } else if (call && call.provider === "gamemonetize" && !call.issued && !call.physicalTerminal) {
          beginCall(call);
        }
        syncHostState();
      }
    }

    function onGameDistributionEvent(event) {
      if (disposed) return;
      const eventName = typeof event?.name === "string" ? event.name : "";
      if (eventName === "SDK_READY") {
        const sdk = windowObject.gdsdk;
        if (!sdk || typeof sdk.showAd !== "function") {
          settleReady(false, "GameDistribution SDK is missing the documented showAd API.");
        } else {
          settleReady(true);
        }
        return;
      }
      if (eventName === "SDK_ERROR") {
        if (!readySettled) settleReady(false, "GameDistribution SDK initialization failed.");
        else diagnostic("GameDistribution reported an SDK error.");
        return;
      }
      if (eventName === "SDK_GAME_PAUSE") {
        providerPaused = true;
        const call = activeCall;
        if (call && call.provider === "gamedistribution" && call.issued && !call.physicalTerminal) {
          call.pauseSeen = true;
          markActualStart(call, false);
        }
        syncHostState();
        return;
      }
      if (eventName === "SDK_GAME_START") {
        providerPaused = false;
        const call = activeCall;
        if (call && call.provider === "gamedistribution" && call.pauseSeen && !call.physicalTerminal) {
          call.resumeSeen = true;
          maybeRetireGameDistribution(call);
        } else if (call && call.provider === "gamedistribution" && !call.issued && !call.physicalTerminal) {
          beginCall(call);
        }
        syncHostState();
        return;
      }
      if (eventName === "SDK_REWARDED_WATCH_COMPLETE") {
        grantRewardProof(gameDistributionRewardCall);
        return;
      }
    }

    function maybeRetireGameDistribution(call) {
      if (!call || call.physicalTerminal || !call.promiseSettled) return;
      if (call.pauseSeen && !call.resumeSeen) return;
      retireCall(call);
    }

    function settleGameDistributionPromise(call, succeeded) {
      if (disposed || call.provider !== "gamedistribution" || call.physicalTerminal) return;
      call.promiseSettled = true;
      if (succeeded) setPublicState(call, AdState.Finished);
      else setPublicState(call, AdState.Failed);
      if (!succeeded) diagnostic("GameDistribution could not complete this break.");
      maybeRetireGameDistribution(call);
    }

    function invokeGameDistribution(call) {
      const sdk = windowObject.gdsdk;
      if (!sdk || typeof sdk.showAd !== "function") {
        failBeforeIssue(call, AdState.Unavailable, "GameDistribution SDK is missing the documented showAd API.");
        return;
      }
      call.issued = true;
      if (call.kind === AdKind.Rewarded) gameDistributionRewardCall = call;
      beginNoStartTimer(call);
      if (gestureCall === call) updateGesture(false, call);
      try {
        const result = call.kind === AdKind.Rewarded ? sdk.showAd("rewarded") : sdk.showAd();
        Promise.resolve(result).then(
          () => settleGameDistributionPromise(call, true),
          () => settleGameDistributionPromise(call, false),
        );
      } catch (_error) {
        failIssuedUncertain(call, "GameDistribution threw while starting the ad.");
      }
    }

    function issueCrazyGames(call) {
      const sdk = windowObject.CrazyGames?.SDK;
      const requestAd = sdk?.ad?.requestAd;
      if (typeof requestAd !== "function") {
        failBeforeIssue(call, AdState.Unavailable, "CrazyGames SDK is missing the documented requestAd API.");
        return;
      }
      call.issued = true;
      beginNoStartTimer(call);
      try {
        requestAd.call(sdk.ad, call.kind === AdKind.Midgame ? "midgame" : "rewarded", {
          adStarted: () => onCrazyGamesStart(call),
          adFinished: () => onCrazyGamesFinish(call),
          adError: () => onCrazyGamesError(call),
        });
      } catch (_error) {
        failIssuedUncertain(call, "CrazyGames threw while starting the ad.");
      }
    }

    function issuePoki(call) {
      const sdk = windowObject.PokiSDK;
      const method = call.kind === AdKind.Midgame ? sdk?.commercialBreak : sdk?.rewardedBreak;
      if (typeof method !== "function") {
        failBeforeIssue(call, AdState.Unavailable, "Poki SDK is missing the requested break API.");
        return;
      }
      call.issued = true;
      beginNoStartTimer(call);
      try {
        const result = method.call(sdk, () => onPokiStart(call));
        Promise.resolve(result).then(
          (value) => onPokiResolved(call, value),
          () => onPokiRejected(call),
        );
      } catch (_error) {
        failIssuedUncertain(call, "Poki threw while starting the ad break.");
      }
    }

    function issueGameMonetize(call) {
      const sdk = windowObject.sdk;
      if (!sdk || typeof sdk.showBanner !== "function") {
        failBeforeIssue(call, AdState.Unavailable, "GameMonetize SDK is missing the documented showBanner API.");
        return;
      }
      call.issued = true;
      beginNoStartTimer(call);
      try {
        sdk.showBanner();
      } catch (_error) {
        failIssuedUncertain(call, "GameMonetize threw while starting the ad break.");
      }
    }

    function beginCall(call) {
      if (disposed || activeCall !== call || call.issued || call.physicalTerminal || providerPaused || !readySettled || !readyResult?.available) return;
      if (call.provider === "gamedistribution") {
        if (gestureCall === call) return;
        updateGesture(true, call);
        clearGestureTimer();
        gestureTimer = schedule(() => {
          gestureTimer = null;
          if (activeCall !== call || call.issued || call.physicalTerminal) return;
          failBeforeIssue(call, AdState.Failed, "No mouse or touch selection was made before the ad request expired.");
        }, GD_GESTURE_TIMEOUT_MS);
        return;
      }
      if (call.provider === "crazygames") issueCrazyGames(call);
      else if (call.provider === "poki") issuePoki(call);
      else if (call.provider === "gamemonetize") issueGameMonetize(call);
      else failBeforeIssue(call, AdState.Unavailable, "The selected ad provider is unavailable.");
    }

    function initializeCrazyGames() {
      const alreadyLoaded = Boolean(windowObject.CrazyGames?.SDK);
      const onSdkLoaded = () => {
        const sdk = windowObject.CrazyGames?.SDK;
        if (!sdk || typeof sdk.init !== "function") {
          settleReady(false, "CrazyGames SDK is missing its documented init API.");
          return;
        }
        try {
          Promise.resolve(sdk.init()).then(
            () => settleReady(true),
            () => settleReady(false, "CrazyGames SDK initialization failed."),
          );
        } catch (_error) {
          settleReady(false, "CrazyGames SDK initialization failed.");
        }
      };
      if (alreadyLoaded) {
        onSdkLoaded();
        return;
      }
      loadScript(
        "https://sdk.crazygames.com/crazygames-sdk-v3.js",
        "crazygames-sdk-v3",
        onSdkLoaded,
        () => settleReady(false, "CrazyGames SDK script failed to load."),
      );
    }

    function initializePoki() {
      const alreadyLoaded = Boolean(windowObject.PokiSDK);
      const onSdkLoaded = () => {
        const sdk = windowObject.PokiSDK;
        if (!sdk || typeof sdk.init !== "function") {
          settleReady(false, "Poki SDK is missing its documented init API.");
          return;
        }
        try {
          Promise.resolve(sdk.init()).then(
            () => settleReady(true),
            () => settleReady(false, "Poki SDK initialization failed; the game can continue without ads."),
          );
        } catch (_error) {
          settleReady(false, "Poki SDK initialization failed; the game can continue without ads.");
        }
      };
      if (alreadyLoaded) {
        onSdkLoaded();
        return;
      }
      loadScript(
        "https://game-cdn.poki.com/scripts/v2/poki-sdk.js",
        "poki-sdk-v2",
        onSdkLoaded,
        () => settleReady(false, "Poki SDK script failed to load; the game can continue without ads."),
      );
    }

    function initializeGameMonetize() {
      if (!gameId.trim()) {
        settleReady(false, "GameMonetize requires a game_id.");
        return;
      }
      if (windowObject.sdk && typeof windowObject.sdk.showBanner === "function") {
        settleReady(false, "GameMonetize SDK was loaded before its event handler was registered.");
        return;
      }
      windowObject.SDK_OPTIONS = {
        gameId,
        onEvent: onGameMonetizeEvent,
      };
      loadScript(
        "https://api.gamemonetize.com/sdk.js",
        "gamemonetize-sdk",
        () => {},
        () => settleReady(false, "GameMonetize SDK script failed to load."),
      );
    }

    function initializeGameDistribution() {
      if (!gameId.trim()) {
        settleReady(false, "GameDistribution requires a game_id.");
        return;
      }
      if (windowObject.gdsdk && typeof windowObject.gdsdk.showAd === "function") {
        settleReady(false, "GameDistribution SDK was loaded before its event handler was registered.");
        return;
      }
      windowObject.GD_OPTIONS = {
        gameId,
        onEvent: onGameDistributionEvent,
      };
      loadScript(
        "https://html5.api.gamedistribution.com/main.min.js",
        "gamedistribution-jssdk",
        () => {},
        () => settleReady(false, "GameDistribution SDK script failed to load."),
      );
    }

    function initialize() {
      if (provider === "none") {
        settleReady(false, "No ad provider is selected.");
        return;
      }
      if (!["crazygames", "gamemonetize", "gamedistribution", "poki"].includes(provider)) {
        settleReady(false, "The selected ad provider is not supported.");
        return;
      }
      readyTimer = schedule(() => settleReady(false, "Ad SDK initialization timed out."), INIT_TIMEOUT_MS);
      try {
        if (provider === "crazygames") initializeCrazyGames();
        else if (provider === "poki") initializePoki();
        else if (provider === "gamemonetize") initializeGameMonetize();
        else initializeGameDistribution();
      } catch (_error) {
        settleReady(false, "Ad SDK initialization failed.");
      }
    }

    function request(kind) {
      if (disposed || !readySettled && provider === "none") return 0;
      if (kind !== AdKind.Midgame && kind !== AdKind.Rewarded) return 0;
      if (provider === "none" || !["crazygames", "gamemonetize", "gamedistribution", "poki"].includes(provider)) return 0;
      if (readySettled && !readyResult?.available) return 0;
      if (provider === "gamemonetize" && kind === AdKind.Rewarded) return 0;
      if (providerPaused || activeCall) return -1;
      if (records.size >= MAX_UNRELEASED_HANDLES || nextHandle > MAX_HANDLE) return -2;

      const handle = nextHandle;
      nextHandle += 1;
      const call = {
        handle,
        provider,
        kind,
        state: AdState.Requesting,
        guestOwned: true,
        issued: false,
        started: false,
        physicalTerminal: false,
        pauseSeen: false,
        resumeSeen: false,
        promiseSettled: false,
        rewardAvailable: false,
        rewardConsumed: false,
        rewardAbandoned: false,
        noStartTimer: null,
      };
      records.set(handle, call);
      activeCall = call;
      syncHostState();
      if (readySettled && readyResult?.available) beginCall(call);
      return handle;
    }

    function poll(handle) {
      const call = Number.isInteger(handle) ? records.get(handle) : null;
      return call && call.guestOwned ? call.state : AdState.Failed;
    }

    function takeReward(handle) {
      const call = Number.isInteger(handle) ? records.get(handle) : null;
      if (!call || !call.guestOwned || !call.rewardAvailable || call.rewardConsumed) return 0;
      call.rewardAvailable = false;
      call.rewardConsumed = true;
      return 1;
    }

    function release(handle) {
      const call = Number.isInteger(handle) ? records.get(handle) : null;
      if (!call) return;
      records.delete(handle);
      call.guestOwned = false;
      call.rewardAvailable = false;
      call.rewardAbandoned = true;
      if (gameDistributionRewardCall === call) gameDistributionRewardCall = null;
      if (activeCall === call && !call.issued) cancelUnissuedCall(call);
      syncHostState();
    }

    function sendLifecycle(event) {
      let target = null;
      let method = null;
      if (provider === "crazygames") {
        target = windowObject.CrazyGames?.SDK?.game;
        method = {
          [LifecycleEvent.LoadingStarted]: "loadingStart",
          [LifecycleEvent.LoadingFinished]: "loadingStop",
          [LifecycleEvent.GameplayStarted]: "gameplayStart",
          [LifecycleEvent.GameplayStopped]: "gameplayStop",
        }[event];
      } else if (provider === "poki") {
        target = windowObject.PokiSDK;
        method = {
          [LifecycleEvent.LoadingFinished]: "gameLoadingFinished",
          [LifecycleEvent.GameplayStarted]: "gameplayStart",
          [LifecycleEvent.GameplayStopped]: "gameplayStop",
        }[event];
      }
      if (!target || !method || typeof target[method] !== "function") return;
      try {
        target[method]();
      } catch (_error) {
        diagnostic("The ad provider rejected a lifecycle notification.");
      }
    }

    function flushPendingLifecycleEvents() {
      if (disposed || !readyResult?.available || pendingLifecycleEvents.length === 0) return;
      const events = pendingLifecycleEvents.splice(0, pendingLifecycleEvents.length);
      for (const event of events) sendLifecycle(event);
    }

    function lifecycle(event) {
      if (
        disposed ||
        !Number.isInteger(event) ||
        event < LifecycleEvent.LoadingStarted ||
        event > LifecycleEvent.GameplayStopped ||
        (provider !== "crazygames" && provider !== "poki")
      ) return;
      if (provider === "poki" && event === LifecycleEvent.LoadingStarted) return;
      if (event === LifecycleEvent.GameplayStarted || event === LifecycleEvent.GameplayStopped) {
        if (lastGameplayLifecycleEvent === event) return;
        lastGameplayLifecycleEvent = event;
      }
      if (readySettled) {
        if (readyResult?.available) sendLifecycle(event);
        return;
      }
      if (pendingLifecycleEvents.length >= MAX_PENDING_LIFECYCLE_EVENTS) {
        // Retain the latest gameplay state; discard the oldest load notification first.
        const loadEventIndex = pendingLifecycleEvents.findIndex(
          (queued) => queued !== LifecycleEvent.GameplayStarted && queued !== LifecycleEvent.GameplayStopped,
        );
        pendingLifecycleEvents.splice(loadEventIndex >= 0 ? loadEventIndex : 0, 1);
      }
      pendingLifecycleEvents.push(event);
    }

    function resetGuest() {
      if (disposed) return;
      for (const call of records.values()) {
        call.guestOwned = false;
        call.rewardAvailable = false;
        call.rewardConsumed = true;
        call.rewardAbandoned = true;
        if (activeCall === call && !call.issued) cancelUnissuedCall(call);
      }
      records.clear();
      gameDistributionRewardCall = null;
      pendingLifecycleEvents.length = 0;
      lastGameplayLifecycleEvent = null;
      syncHostState();
    }

    function dispose() {
      if (disposed) return;
      const shouldUnblock = lastBlocked === true;
      const shouldResumeAudio = lastAudioPaused === true;
      disposed = true;
      clearTimer(readyTimer);
      clearTimer(gestureTimer);
      readyTimer = null;
      gestureTimer = null;
      for (const call of records.values()) clearNoStartTimer(call);
      if (activeCall) clearNoStartTimer(activeCall);
      activeCall = null;
      gameDistributionRewardCall = null;
      localPauseCall = null;
      providerPaused = false;
      gestureCall = null;
      records.clear();
      if (!readySettled) {
        readySettled = true;
        readyResult = { available: false, diagnostic: "Ad lifecycle disposed." };
        readyResolve(readyResult);
      }
      safeCallback(onGestureNeeded, {
        visible: false,
        label: "",
        activate: () => false,
        cancel: () => {},
      });
      if (windowObject.SDK_OPTIONS?.onEvent === onGameMonetizeEvent) {
        windowObject.SDK_OPTIONS.onEvent = () => {};
      }
      if (windowObject.GD_OPTIONS?.onEvent === onGameDistributionEvent) {
        windowObject.GD_OPTIONS.onEvent = () => {};
      }
      lastBlocked = false;
      lastAudioPaused = false;
      if (shouldUnblock) safeCallback(onGameplayBlocked, false);
      if (shouldResumeAudio) safeCallback(onAudioPaused, false);
    }

    const api = Object.freeze({
      request,
      poll,
      gameplayBlocked: () => (isGameplayBlocked() ? 1 : 0),
      takeReward,
      release,
      lifecycle,
      ready,
      resetGuest,
      dispose,
    });

    syncHostState(true);
    initialize();
    return api;
  }

  globalObject.STASIS_AD_LIFECYCLE = Object.freeze({ createAdLifecycle });
})(typeof globalThis === "object" ? globalThis : window);
