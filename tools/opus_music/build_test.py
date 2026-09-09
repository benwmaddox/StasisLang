"""Build a local blind Opus listening experiment from highest-quality masters."""
import argparse
import hashlib
import json
import math
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parent
BITRATES = (32, 24, 20, 16)


def run(*args):
    return subprocess.run(args, check=True, capture_output=True, text=True, timeout=300)


def measure(path):
    duration = float(run('ffprobe', '-v', 'error', '-show_entries',
                         'format=duration', '-of', 'default=nw=1:nk=1', str(path)).stdout)
    if not math.isfinite(duration) or duration <= 0:
        raise ValueError(f'Invalid duration: {path}')
    size = path.stat().st_size
    return dict(durationSeconds=duration, sizeBytes=size,
                kbPerMinute=size / duration * 60 / 1000,
                effectiveKbps=size * 8 / duration / 1000)


def build(source, output, bitrates=BITRATES):
    files = sorted(p for p in source.iterdir()
                   if p.is_file() and p.suffix.lower() in {'.wav', '.flac', '.aiff', '.aif', '.mp3', '.m4a'})
    if not files:
        raise ValueError('No supported masters found. Add WAV, FLAC, AIFF, MP3 or M4A files.')
    tracks = []
    for original in files:
        digest = hashlib.sha256(original.read_bytes()).hexdigest()
        track_id = hashlib.sha256(original.name.encode()).hexdigest()[:12]
        normalized = output / 'music' / 'normalized' / f'{track_id}.wav'
        encoded = output / 'music' / 'encoded' / track_id
        normalized.parent.mkdir(parents=True, exist_ok=True)
        encoded.mkdir(parents=True, exist_ok=True)
        # Measure after downmix so reference and variants share the same loudness and channels.
        base = ('ffmpeg', '-hide_banner', '-nostdin', '-y', '-i', str(original), '-vn')
        first = run(*base, '-af', 'aformat=channel_layouts=mono,loudnorm=I=-18:TP=-2:LRA=11:print_format=json',
                    '-f', 'null', '-')
        stats, _ = json.JSONDecoder().raw_decode(first.stderr[first.stderr.rfind('{'):])
        values = [float(stats[k]) for k in ('input_i', 'input_tp', 'input_lra', 'input_thresh', 'target_offset')]
        if not all(math.isfinite(v) for v in values):
            raise ValueError(f'Master is silent or cannot be normalized: {original}')
        i, tp, lra, threshold, offset = values
        filt = (f'aformat=channel_layouts=mono,loudnorm=I=-18:TP=-2:LRA=11:'
                f'measured_I={i}:measured_TP={tp}:measured_LRA={lra}:'
                f'measured_thresh={threshold}:offset={offset}:linear=true')
        run(*base, '-af', filt, '-ar', '48000', '-c:a', 'pcm_s24le', str(normalized))
        variants = []
        for bitrate in bitrates:
            path = encoded / f'{bitrate}k.opus'
            run('ffmpeg', '-hide_banner', '-nostdin', '-y', '-i', str(normalized),
                '-c:a', 'libopus', '-b:a', f'{bitrate}k', '-vbr', 'on', '-ac', '1', str(path))
            variants.append(dict(bitrateKbps=bitrate, path=path.relative_to(output).as_posix(), **measure(path)))
        tracks.append(dict(id=track_id, name=original.stem, sourceSha256=digest,
                           reference=normalized.relative_to(output).as_posix(),
                           **measure(normalized), variants=variants))
    data = dict(schemaVersion=1, tracks=tracks)
    data['experimentId'] = hashlib.sha256(json.dumps(data, sort_keys=True).encode()).hexdigest()
    (output / 'metadata.json').write_text(json.dumps(data, indent=2) + '\n', encoding='utf-8')
    return data


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('--extended', action='store_true', help='Include 48 and 12 kbps')
    args = parser.parse_args()
    build(args.source.resolve(), ROOT, (48, 32, 24, 20, 16, 12) if args.extended else BITRATES)
    print('Ready: python -m http.server 8765 --bind 127.0.0.1 --directory tools/opus_music')
