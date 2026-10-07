# Web portal ads

Web ad requests are asynchronous host operations. The guest owns request intent, polling, reward consumption, and the gameplay guard; the browser host owns one selected portal SDK, user activation, pause/audio state, and the small activation prompt.

## Select a portal profile

Set a default in `stasis.json`:

```json
{
  "web": {
    "portal": {
      "provider": "gamedistribution",
      "game_id": "public-game-id"
    }
  }
}
```

Supported providers are `crazygames`, `gamemonetize`, `gamedistribution`, and `poki`. GameMonetize and GameDistribution require their public game ID. Do not put account credentials or private keys in a manifest. A project can select a different profile for each build without editing generated output:

```text
stasis package --target web --portal-profile crazygames
stasis package --target web --portal-profile gamemonetize --portal-game-id public-game-id
stasis package --target web --portal-profile gamedistribution --portal-game-id public-game-id
stasis package --target web --portal-profile poki
stasis package --target web --portal-profile none
```

An explicit profile overrides the manifest profile. By default, `none` writes `dist/<project>-web/`; a portal profile writes `dist/<project>-web-<provider>/`. `--out` selects an exact package directory relative to the project root. An explicit non-`none` profile packages `ad_lifecycle.js` even when the current guest does not import ad calls. The runtime loads only the selected provider SDK. The default/`none` profile has no ad module or provider SDK request; unsupported or unavailable host calls fail closed so the game can continue.

## Guest request and poll loop

Import the standard-library modules for ad tasks and portal lifecycle. The imports below use the public vendored path in a project with the selected toolchain stdlib. Keep the `AdTask` in global storage; local struct initialization is not supported on every target. `begin` accepts one request, while `should_block_gameplay` polls its host-owned handle:

```stasis
import "/vendor/stasis/src/stdlib/ad_tasks.stasis";
import "/vendor/stasis/src/stdlib/portal_lifecycle.stasis";

global midgame_ad: AdTask;
global midgame_ad_open: bool;
global last_midgame_result: AdState;
global simulation_steps: i32;

function request_midgame(): bool {
    let result: AdBeginResult = midgame_ad.begin(AdKind.Midgame);
    if (result == AdBeginResult.Accepted) {
        midgame_ad_open = true;
    }
    return result == AdBeginResult.Accepted;
}

function tick(): i32 {
    // Poll even when no request is pending: an SDK can issue an unsolicited
    // global pause, and the guest owns the gameplay decision.
    let blocked: bool = midgame_ad.should_block_gameplay();
    if (midgame_ad_open && (midgame_ad.state == AdState.Finished
        || midgame_ad.state == AdState.Failed
        || midgame_ad.state == AdState.Unavailable)) {
        last_midgame_result = midgame_ad.state;
        ad_tasks.release(midgame_ad);
        midgame_ad_open = false;
        return 0;
    }
    if (blocked) {
        // If your simulation uses elapsed time, refresh its previous-time
        // baseline here so resuming does not catch up the time spent in the ad.
        return 0;
    }

    simulation_steps += 1;
    return 0;
}
```

The browser continues calling guest `tick` and `render` during a break. The guard must run before gameplay updates on every tick: it polls while blocked, then skips simulation. Store the terminal state before `release()` because release resets the task to `Idle`; this also frees its handle for the next break. This is the portable contract across JIT, Wasm, and native/mobile stubs. The host also clears and suppresses held key/pointer input while blocked, but guest code must still guard its simulation. If the simulation derives delta time from `HostFrame`, update its time baseline while blocked to avoid a catch-up step afterward.

When another imported library exports a common function name, use the module-qualified ad-task form such as `ad_tasks.status(task)`, `ad_tasks.release(task)`, or `ad_tasks.reset(task)`. Unique operations such as `task.begin(...)` and `task.should_block_gameplay()` can use receiver form.

Requesting starts blocking gameplay as soon as the host accepts the request; it does not suspend audio. Audio pauses only on an actual SDK start/pause signal. The host keeps page visibility, pagehide, and ad pause reasons independent, remembers whether audio was running (including a pending user-gesture resume), and leaves an initially suspended context suspended after the break. Rendering and lifecycle polling remain active while gameplay/input are blocked.

`AdBeginResult` values are `Accepted = 0`, `Unavailable = 1`, `Busy = 2`, `Capacity = 3`, and `HandleOpen = 4`. `Accepted` means the request was queued, not that an ad has started. The task states are `Idle = 0`, `Requesting = 1`, `Playing = 2`, `Finished = 3`, `Failed = 4`, and `Unavailable = 5`; kinds are `Midgame = 0` and `Rewarded = 1`. A raw request returns a positive handle, `0` for unavailable/invalid input, `-1` when another provider break is active, or `-2` when the host's 16-handle table is full. Polling an invalid or released handle returns `Failed`.

Do not overwrite an open `AdTask` with a second request. `begin` returns `AdBeginResult.HandleOpen` and preserves the first live handle. Handles remain monotonic for the lifetime of the page. Guest reset invalidates guest-owned handles and reward tokens; a full page reload ends that handle lifetime.

## Reward consumption and release

Only the host's provider-specific proof can make `take_reward()` succeed. It consumes that handle's token once, including when the handle was copied. A second call returns false. Release, guest reset, disposal, and no-start timeout abandon any unconsumed reward; releasing a handle is not proof of ad completion. Ordinary provider error and no-fill outcomes do not grant a reward. GameDistribution completion proof is independent of its promise: proof already received remains available on the same unreleased rewarded handle if `showAd()` later throws or its promise rejects. Keep request outcome separate from reward evidence.

The current adapter capability is:

- **CrazyGames:** midgame and rewarded requests. Only `adStarted` pauses audio; `adFinished` settles the break and is the documented rewarded-completion callback. Errors and no-fill outcomes do not reward.
- **GameMonetize:** the documented `sdk.showBanner()` ad-break call with `SDK_GAME_PAUSE` / `SDK_GAME_START` pause and resume events. Rewarded requests are unavailable because its public SDK contract does not establish rewarded completion.
- **GameDistribution:** regular and rewarded `showAd()` breaks. Only `SDK_REWARDED_WATCH_COMPLETE` grants a rewarded token; promise fulfillment, `SDK_GAME_START`, and the normal break result do not. Stasis follows the supported one-call-at-a-time flow and associates the untagged global event with the latest issued rewarded request. Proof remains available on that same unreleased handle even if the promise later rejects or `showAd()` later throws, until release/reset/dispose/no-start timeout or a newer rewarded call is issued. The SDK documents no request ID, event ordering guarantee, or maximum delay, so an arbitrarily late completion from an earlier rewarded ad after a newer rewarded `showAd()` is issued can be attributed to the newer handle. This is a vendor-correlation limit, not a guarantee that cross-request events are distinguishable.
- **Poki:** commercial and rewarded breaks. A resolved commercial break may have shown no ad; only `rewardedBreak()` resolving exactly `true` grants the reward.

GameDistribution requires a user activation during `mouseUp`/`touchUp`. The guest request does not call the SDK from a later tick. Instead, the host presents **Watch ad** and **Cancel**; a mouse or touch on **Watch ad** invokes `showAd()` synchronously from that button's pointer-up handler. Keyboard activation explains the mouse/touch requirement and leaves Cancel available. Canceling or waiting 60 seconds fails the pre-issue request without an SDK call.

## Portal submission and placement

The profile only selects the SDK integration. Each portal separately controls submission, account approval, ad eligibility, and placement:

- **CrazyGames:** follow the [HTML5 SDK guide](https://docs.crazygames.com/sdk/intro/) and [ad requirements](https://docs.crazygames.com/requirements/ads/). Basic Launch must remain playable with ads disabled. Do not interrupt a menu, settings screen, shop, or other navigation action with a midgame request; use an appropriate break between active play. The diagnostic click-anywhere sample is not an approved placement.
- **GameMonetize:** use the public game ID from the [GameMonetize SDK page](https://gamemonetize.com/sdk), build the selected profile, and upload its web ZIP through the portal dashboard. The adapter uses the documented `sdk.showBanner()` and pause/start callbacks; it does not infer rewarded support.
- **GameDistribution:** use the game ID and submission flow in the [HTML5 SDK implementation guide](https://github.com/GameDistribution/GD-HTML5/wiki/SDK-Implementation). Follow its mouse/touch release requirement for `showAd()`. The [rewarded-ads guide](https://github.com/GameDistribution/GD-HTML5/wiki/Rewarded-Ads) requires portal-side rewarded configuration and documents the untagged completion event; the adapter supports that event for the latest issued rewarded request with the attribution boundary described above. Portal approval and account settings are outside this build's acceptance.
- **Poki:** follow the [HTML5 SDK guide](https://developers.poki.com/guide/sdk-html5) and [quality requirements](https://developers.poki.com/guide/requirements-quality), including loading/gameplay events and non-disruptive breaks. Rewarded video must be clearly selected and must not gate core progress. SDK wiring does not imply portal approval.

These links describe public SDK and quality contracts; they do not verify a live account, submission approval, ad fill, or commercial placement.

SDK initialization has an 8-second host deadline, and an issued request has a 15-second no-start deadline. A no-start timeout can fail the public request and abandon its reward, but it never resumes audio/gameplay over a subsequently visible ad. If a late actual start or provider pause arrives, the host retains a private orphan call and keeps the physical pause active until an actual terminal event or page reload. That unresolved provider call continues to occupy the single-ad slot; the host does not start an overlapping SDK request.

Use `notify_portal_lifecycle` only at real guest transitions: loading start/finish and gameplay start/stop. Do not derive gameplay events from animation frames, focus changes, or menu redraws. Poki does not support `LoadingStarted`; the adapter ignores that event. Lifecycle notifications sent before SDK readiness are buffered in a bounded queue and replayed after initialization.

## Browser acceptance fixture

`samples/web_ad_lifecycle` is a compilable diagnostic no-audio fixture ([source](../samples/web_ad_lifecycle/src/main.stasis)). Its guest polls and stops simulation on every blocked tick while render and poll counters continue; its status band distinguishes requesting, playing, and terminal states in review captures. `samples/web_ad_lifecycle_audio` adds a real Web Audio tone so browser acceptance can verify the physical pause and restore path. The CDP runner intercepts the selected SDK script at evaluation time and inserts a deterministic test fake; no SDK global is preloaded, and test providers cannot be selected in a package profile. These fixtures test the adapter contract and do not establish live ad fill, portal approval, or acceptable commercial placement. The click-anywhere diagnostic break is not a placement recommendation. Integrations should request breaks at a natural, non-gameplay transition and follow the selected portal's placement rules.

`tools/run_web_ad_browser_acceptance.mjs` serves a freshly built development package, intercepts the selected provider SDK URL, and runs the real package in Chrome through CDP. It records provider script requests, gameplay/poll/render counters, reward consumption, user activation details for GameDistribution, audio-context transitions, PNGs, and a timestamped MP4 interaction capture. It also accepts `--sdk-failure` and `--module-failure` for real-browser SDK-script and dynamic-module failure checks. The touch option uses Chrome's 390x844 mobile touch emulation; it is not a physical-device run. See the command examples in [Web packaging](web_packaging.md#portal-ad-profiles) and the sample manifests for profile/output selection.
