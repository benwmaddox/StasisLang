# Low-bitrate Opus music experiment

This standalone prototype implements task 497's recommended first run. It does
not change Stasis playback or choose a production bitrate without listening data.

1. Install Python 3 and FFmpeg with `libopus`; put `ffmpeg` and `ffprobe` on PATH.
2. Put three highest-quality AI-generated instrumental masters (60-90 seconds)
   in `tools/opus_music/music/source/`. WAV/FLAC is preferred; MP3/M4A is accepted
   only when that is the generator's highest-quality download. Keep originals.
3. From the repository root run:
   `python tools/opus_music/build_test.py tools/opus_music/music/source`
4. Run `python -m http.server 8765 --bind 127.0.0.1 --directory tools/opus_music`.
   Open `http://127.0.0.1:8765` in a browser supporting Opus (Chrome or Firefox).
5. Select Reference or a blind version, press Play, and switch versions while
   listening. Playback resumes at approximately the previous timestamp after
   loading; this is not a gapless crossfade. Native controls provide pause/seek.
6. Score every version, save to reveal, and repeat in gameplay-background mode.
   This mode lowers music 15 dB; it does not simulate a running game or effects.
7. Download `ratings.json` and `summary.html` into a local `results/` folder.
   Allow multiple downloads if prompted. Ratings and blind order also persist
   in browser local storage for this experiment and origin. Export before
   changing browsers, clearing storage, rebuilding, or changing server ports.

No music-generation account or masters are bundled. Use any provider with
appropriate download rights; record provider, prompt, model/date and license
alongside each source. Suggested first three prompts:

- Cozy management game: warm playful simple melody, light percussion, no vocals.
- Upbeat arcade game: clear percussion, fast melodic lead, no vocals.
- Dense cinematic-lite background: multiple instruments, broad frequency range,
  restrained dynamics, no vocals.

Expand to five with clean electronic strategy music and warm plucked acoustic
music. Use `--extended` for 48/32/24/20/16/12 kbps instead of the MVP's
32/24/20/16. Prompt-pair generation, stereo, artifact tags and foreground sound
effects are deferred until the first listening experiment is promising.

The builder measures loudness after downmix, uses two-pass FFmpeg loudnorm
(-18 LUFS, -2 dB true peak, linear when possible), and writes a 48 kHz 24-bit
mono reference. FFmpeg may use dynamic normalization when linear targets cannot
be met. All VBR encodes come directly from that reference; originals are never
modified. No manual bandwidth restriction is applied. Stop the server while
rebuilding; metadata is published only after all encodes succeed. Generated
audio, metadata and results are ignored by Git. Rebuilding replaces outputs.

The report uses measured container bytes and durations, decimal KB/MB, and
duration-weighted storage rates. Recommendations require all tracks (at least
three) to have gameplay scores, mean >=4/5 and >=80% Yes (Maybe counts against
acceptance). Optional percentages, if supplied, must average >=80%; absent
percentages cannot prove 80% perceived fidelity. Isolated ratings are retained
in JSON but gameplay scores drive the decision. This is one listener's result,
not a population estimate. No result is claimed before Ben listens.

Validation: `python -m unittest discover -s tools/opus_music -p "test_*.py"`
and `node --test tools/opus_music/report.test.mjs`.
