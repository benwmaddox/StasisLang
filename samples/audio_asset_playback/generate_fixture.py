#!/usr/bin/env python3
"""Generate the tiny audibly identifiable PCM16 WAV used by this sample."""

import math
import pathlib
import struct
import wave


OUTPUT = pathlib.Path(__file__).parent / "assets" / "tone.wav"
SAMPLE_RATE = 24_000
DURATION_SECONDS = 0.75


def main() -> None:
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    frame_count = int(SAMPLE_RATE * DURATION_SECONDS)
    with wave.open(str(OUTPUT), "wb") as output:
        output.setnchannels(1)
        output.setsampwidth(2)
        output.setframerate(SAMPLE_RATE)
        for frame in range(frame_count):
            time = frame / SAMPLE_RATE
            envelope = min(1.0, frame / 600.0) * min(1.0, (frame_count - frame) / 1200.0)
            sample = 0.18 * envelope * math.sin(2.0 * math.pi * 220.0 * time)
            output.writeframesraw(struct.pack("<h", round(sample * 32767.0)))


if __name__ == "__main__":
    main()
