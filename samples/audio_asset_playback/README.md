# WAV asset playback

This sample exercises the SDL-backed, Brickout-compatible WAV helpers. It loops
a quiet tone, pauses and resumes it, changes its volume, overlaps an effect,
stops the music, and starts it again.

Generate the checked-in fixture after deliberately changing its recipe:

```text
python samples/audio_asset_playback/generate_fixture.py
```

Run the sample from its project directory:

```text
stasis run
```
