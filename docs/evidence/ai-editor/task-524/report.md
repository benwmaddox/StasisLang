# Task 524 live acceptance

Passed on 2026-09-10 using two real OpenRouter tasks in one disposable Asset
Breakout session. Task 1 changed the render accent; task 2 received a verified
934 x 525 live PNG and added a white ball outline. Both required explicit
Accept and Apply, compiled the actual candidate, and passed 8/8 tests.

| Measurement | Accent task | Image/ball task |
| --- | ---: | ---: |
| OpenRouter model | google/gemini-2.5-flash | openai/gpt-5.6-luna |
| Resolved provider | Google | OpenAI |
| First streamed action token | 5.426 s | 4.774 s |
| Usable reviewed proposal | 11.841 s | 17.144 s |
| Provider total | 10.917 s | 16.272 s |
| Apply through passing receipt | 1.349 s | 1.359 s |
| Staged child compile/tests | 755.178 ms | 759.111 ms |
| Validated apply total | 776.806 ms | 780.794 ms |
| Live watcher compile | 83 ms | 82 ms |
| Patch packaging | 0 ms | 0 ms |
| Between-tick commit | 171 ms | 172 ms |
| Prompt / completion tokens | 16,329 / 2,457 | 16,374 / 1,140 |
| Provider cost | $0.0110412 | $0.0054612 |
| Tests | 8 passed | 8 passed |

Active flow: **44.342 s**, including evidence encoding, excluding a deliberate
60-second capture setup hold. Playable warmup was 5.034 s before the flow.
Successful-run provider cost: **$0.0165024**. Both tasks took two provider turns;
no retry occurred in this successful run. Earlier failed attempts are retained
separately in `provider-failed-attempt*.json` and are not included in that cost.
Raw usage, compiler receipts, plans, and timing definitions are in `report.json`.

The same session advanced generation 1 -> 2 -> 3 while paused at tick 19.
Paddle x remained 0, bricks remained 35, assets stayed ready, and the loader
failure flag stayed false after both commits. On resume the ball moved and
continued through normal game logic. Each edit recompiled one function and
reused 42. Failing-candidate and parent-runtime isolation regressions are
recorded in `../task524/validation.md`.

## Visual evidence and provenance

- `native-window-flow.png` and `native-window-flow.mp4`: inspected labeled
  PrintWindow captures of the two actual HWND surfaces, with native chrome.
  The editor is graphical, independent of the SDL game, and tiled left/right
  with an 8-pixel gap. The 15.25-second video shows the image request through
  applied/passing state beside the continuing game. It is a surface composite,
  not a desktop screenshot; Windows was locked. `native-window-flow.json`
  records the exact process, handles, rectangles, and successful capture stop.
- `editor-flow.mp4`: inspected complete 35.5-second sampled editor progression
  through both requests, review/accept/apply, image attachment, and passing
  state. Capture changed from 620 x 950 to 934 x 1034 after its first 0.5-second
  frame. The encoded video's first frame was padded and subsequent frames
  restored to their known native aspect ratio; content and sequence were not
  replaced. Some text retains the original encoding's downsampling blur.
- `game-motion.mp4`: inspected decoded frames showing the outlined ball moving
  and the round continuing after resume. `game-frame.png` is the verified
  playable frame actually sent to the image model.
- `first-proposal.png`, `second-proposal.png`, and `final-editor.png`: original
  native editor renders. The timeline's captured scroll position obscures part
  of the expanded changes; supplemental recorded-plan review PNGs show the
  exact hunks without claiming a second provider execution.

## Reference comparison

Compared with `../task-flow-reference.jpg`, the implementation retains task
navigation, current-task state, chronological evidence, persistent reply
composer, semantic diff, test status, and a separate game window. Intentional
differences are equal-half tiling, Windows native chrome, the Asset Breakout
sample and its 16:9 letterboxing, text-first cards, actual provider/cost/context
metrics, and explicit acceptance. Mockup avatars, decorative tags, forest art,
and generated-asset examples are not reproduced. Compact navigation replaces
the wide project rail; the compact/wide/fractional-scale fixtures are documented
in `../task524/layout-validation.md`.

Foreground keyboard focus under the locked desktop and a physical mixed-DPI
monitor transition were not observed. Native restore/geometry checks and
focused keyboard/layout tests passed; these hardware limits are not presented
as live visual validation.

Theory gained: tests activate process-global JIT state. Staging and validating
in a child before publishing source preserves the live game, and future fixture
types must participate in the same staging and fingerprint boundary.
