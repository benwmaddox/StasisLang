# WSL dev loop (Brickout Revenge)

This repo supports developing on Linux/WSL. For hot-swap latency, the biggest win is avoiding Windows filesystem/AV overhead by running from the WSL filesystem (ext4 inside the distro), not from `/mnt/c` or `/mnt/f`.

## 1) One-time setup (Ubuntu)

In your WSL distro:

```bash
sudo apt-get update
sudo apt-get install -y build-essential cmake pkg-config clang lld git \
  libsdl2-dev libglew-dev
```

Install toolchains if you don't already have them:

- .NET SDK 9.x
- Rust (stable) via rustup

## 2) Clone into the WSL filesystem (important)

```bash
mkdir -p ~/src
cd ~/src
git clone https://github.com/benwmaddox/StasisLang.git
cd StasisLang
```

If you prefer to keep a single working copy, open `\\\\wsl$\\<distro>\\home\\<user>\\src\\StasisLang` in your editor (or use VS Code Remote - WSL).

## 3) Build once

```bash
./build.sh
```

## 4) Run Brickout Revenge with watch + hot-swap

WSLg (Win11) should open the window automatically.

```bash
./dev_brickout_revenge_wsl.sh
```

For v1:

```bash
./dev_brickout_revenge_v1_wsl.sh
```

### Diskless hot-swap (optional)

This avoids writing/loading hot-swap DLL/SO artifacts, which can reduce hot-swap latency:

```bash
cd tools/cranelift-jit-runner && cargo build --release
STASIS_CRANELIFT_JIT_RUNNER=1 ./dev_brickout_revenge_wsl.sh
```

## 5) Measuring hot-swap latency

The `--watch` loop prints `HOTRELOAD phases(ms): ...` and `HOTSWAP latency(ms): ...` after an edit triggers a rebuild/swap.

For apples-to-apples timing, use a repo-local workspace under WSL (not `/mnt/*`), make a single small edit, and compare the printed `HOTSWAP latency(ms)` between:

- Windows native (`dev_brickout_revenge*.bat`)
- WSL (`dev_brickout_revenge*_wsl.sh`)
