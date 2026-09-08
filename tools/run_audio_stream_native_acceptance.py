"""Record the public PCM fixture with a freshly built native compiler/runtime."""

import argparse
import array
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stasis", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--output", type=Path, default=Path("target/audio-stream-native"))
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    compiler, runtime, output = args.stasis.resolve(), args.runtime.resolve(), args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    recording = output / "audio.mp3"
    pcm_path = output / "audio.f32le"
    env = dict(os.environ, STASIS_RUNTIME_LIBRARY_PATH=str(runtime),
               STASIS_RUNTIME_DLL_PATH="", SDL_VIDEODRIVER="dummy", SDL_AUDIODRIVER="dummy")
    subprocess.run([
        str(compiler), "record", "--workspace", str(root / "samples/audio_stream_pcm"),
        "--output", str(recording), "--width", "320", "--height", "180",
        "--fps", "60", "--frames", "120", "--json",
    ], cwd=root, env=env, check=True, timeout=120)
    subprocess.run([
        "ffmpeg", "-nostdin", "-n", "-v", "error", "-i", str(recording),
        "-f", "f32le", "-acodec", "pcm_f32le", "-ar", "48000", "-ac", "2", str(pcm_path),
    ], check=True, timeout=30)
    pcm = array.array("f", pcm_path.read_bytes())
    if sys.byteorder != "little":
        pcm.byteswap()
    assert len(pcm) == 96000 * 2, f"expected two seconds of stereo PCM, got {len(pcm)} samples"
    # Exclude codec edges; MP3 introduces ringing, so compare RMS and frequency.
    left, right = pcm[0::2][4800:-4800], pcm[1::2][4800:-4800]
    rms = (sum(x * x for x in left) / len(left)) ** 0.5
    ratio_error = (sum((r - l * 0.5) ** 2 for l, r in zip(left, right)) / len(left)) ** 0.5
    crossings = sum(a * b < 0 for a, b in zip(left, left[1:]))
    frequency = crossings * 48000 / (2 * len(left))
    assert 0.43 < rms < 0.53, f"incorrect signal RMS: {rms}"
    assert ratio_error < 0.01, f"incorrect stereo ratio: {ratio_error}"
    assert abs(frequency - 480) < 1, f"incorrect frequency: {frequency}"
    inputs = {
        "compiler": compiler, "runtime": runtime, "recording": recording, "pcm": pcm_path,
        "fixture": root / "samples/audio_stream_pcm/main.stasis",
    }
    receipt = {
        "frames": 96000, "sample_rate": 48000, "channels": 2, "rms": rms,
        "stereo_ratio_rms_error": ratio_error, "frequency_hz": frequency,
        "capture": "Native deterministic recording, MP3 decoded to f32le; no physical output",
        "sha256": {name: hashlib.sha256(path.read_bytes()).hexdigest() for name, path in inputs.items()},
    }
    (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(receipt, indent=2))


if __name__ == "__main__":
    main()
