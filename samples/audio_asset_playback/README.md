# Compressed audio asset playback

This sample starts nonblocking MP3 and PCM WAV loads, checks `ready()` each tick,
then plays through the asset values themselves. It loops a quiet compressed tone,
pauses and resumes it, changes its volume, overlaps a WAV effect, stops the music,
and starts it again. MP3 bytes remain compressed on disk and decode into bounded
host memory on the host worker.

Generate the checked-in fixture after deliberately changing its recipe:

```text
python samples/audio_asset_playback/generate_fixture.py
ffmpeg -i samples/audio_asset_playback/assets/tone.wav -ac 1 -ar 24000 -b:a 128k samples/audio_asset_playback/assets/tone.mp3
```

Run the sample from its project directory:

```text
stasis run
```
