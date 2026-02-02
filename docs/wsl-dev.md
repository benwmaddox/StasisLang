# WSL dev loop (Brickout Revenge)

This repo supports developing on Linux/WSL. For hot-swap latency, the biggest win is avoiding Windows filesystem/AV overhead by running from the WSL filesystem (ext4 inside the distro), not from `/mnt/c` or `/mnt/f`.

## 1) One-time setup (Ubuntu)

In your WSL distro:

```bash
sudo apt-get update
sudo apt-get install -y build-essential cmake pkg-config clang lld git \
  libsdl2-dev libglew-dev
```

Install toolchains if you don't already have them.

### .NET SDK (Ubuntu)

This repo targets .NET 9.x. On Ubuntu in WSL, the most reliable approach is to install the Microsoft package feed for your Ubuntu version and then install `dotnet-sdk-9.0`.

First, check your Ubuntu version (pick the matching install block below):

```bash
cat /etc/os-release
# Look for: VERSION_ID="24.04" (or "22.04", etc.)
```

#### Ubuntu 24.04 (VERSION_ID="24.04")

```bash
sudo apt-get update
sudo apt-get install -y ca-certificates wget apt-transport-https

wget "https://packages.microsoft.com/config/ubuntu/24.04/packages-microsoft-prod.deb" \
  -O /tmp/packages-microsoft-prod.deb
sudo dpkg -i /tmp/packages-microsoft-prod.deb
rm -f /tmp/packages-microsoft-prod.deb

sudo apt-get update
sudo apt-get install -y dotnet-sdk-9.0

dotnet --info
```

#### Ubuntu 22.04 (VERSION_ID="22.04")

```bash
sudo apt-get update
sudo apt-get install -y ca-certificates wget apt-transport-https

wget "https://packages.microsoft.com/config/ubuntu/22.04/packages-microsoft-prod.deb" \
  -O /tmp/packages-microsoft-prod.deb
sudo dpkg -i /tmp/packages-microsoft-prod.deb
rm -f /tmp/packages-microsoft-prod.deb

sudo apt-get update
sudo apt-get install -y dotnet-sdk-9.0

dotnet --info
```

#### Auto-detect (works if Microsoft publishes a config for your VERSION_ID)

```bash
sudo apt-get update
sudo apt-get install -y ca-certificates wget apt-transport-https

source /etc/os-release
wget "https://packages.microsoft.com/config/ubuntu/$VERSION_ID/packages-microsoft-prod.deb" \
  -O /tmp/packages-microsoft-prod.deb
sudo dpkg -i /tmp/packages-microsoft-prod.deb
rm -f /tmp/packages-microsoft-prod.deb

sudo apt-get update
sudo apt-get install -y dotnet-sdk-9.0

dotnet --info
```

#### If apt can't find `dotnet-sdk-9.0` (common on new Ubuntu releases)

If you see `E: Unable to locate package dotnet-sdk-9.0`, first sanity-check what packages the repo is exposing:

```bash
grep -R "packages.microsoft.com" /etc/apt/sources.list /etc/apt/sources.list.d || true
sudo apt-get update
apt-cache search dotnet-sdk | head -n 20
```

If `dotnet-sdk-9.0` still doesn't appear, use the official installer script (installs into your home dir; no apt repo required):

```bash
sudo apt-get update
sudo apt-get install -y ca-certificates curl

curl -fsSL https://dot.net/v1/dotnet-install.sh -o /tmp/dotnet-install.sh
chmod +x /tmp/dotnet-install.sh

/tmp/dotnet-install.sh --channel 9.0 --install-dir "$HOME/.dotnet"

echo 'export DOTNET_ROOT="$HOME/.dotnet"' >> ~/.bashrc
echo 'export PATH="$HOME/.dotnet:$PATH"' >> ~/.bashrc
source ~/.bashrc

dotnet --info
dotnet --list-sdks
```

If you prefer to manage multiple SDKs, install `dotnet` via `asdf` or `dotnet-install.sh`, but the apt-based install above tends to be the least surprising for WSL dev.

### Rust (rustup + stable toolchain)

Install `rustup`, then install a stable Rust toolchain:

```bash
sudo apt-get update
sudo apt-get install -y curl

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Make cargo/rustup available in this shell
source "$HOME/.cargo/env"

rustup default stable
rustc --version
cargo --version
```

If you already have Rust from `apt`, prefer rustup for predictable toolchain pinning and modern `cargo` behavior.

### GitHub CLI (`gh`) + auth (WSL)

If you want a smooth GitHub auth + clone experience inside WSL, install GitHub CLI and authenticate in the WSL environment (WSL has its own home directory and credentials).

Install `gh`:

```bash
type -p curl >/dev/null || (sudo apt-get update && sudo apt-get install -y curl)

curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
  | sudo dd of=/usr/share/keyrings/githubcli-archive-keyring.gpg
sudo chmod go+r /usr/share/keyrings/githubcli-archive-keyring.gpg

echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
  | sudo tee /etc/apt/sources.list.d/github-cli.list >/dev/null

sudo apt-get update
sudo apt-get install -y gh
gh --version
```

Authenticate (recommended: HTTPS + device flow). This is the most reliable flow in WSL because it does not require launching a Linux browser:

```bash
gh auth login
gh auth status

# Optional: configure git to use gh for HTTPS credentials
gh auth setup-git
```

Notes on auth in WSL:

- If `gh auth login` shows a URL + one-time code, open the URL in your normal Windows browser and paste the code to complete auth.
- If you previously authenticated `gh` on Windows, that does not automatically apply to WSL; run `gh auth login` inside WSL too.

## 2) Clone into the WSL filesystem (important)

If you installed and authenticated `gh`, cloning is straightforward:

```bash
mkdir -p ~/src
cd ~/src
gh repo clone benwmaddox/StasisLang
cd StasisLang
```

You can also use plain git:

```bash
mkdir -p ~/src
cd ~/src
git clone https://github.com/benwmaddox/StasisLang.git
cd StasisLang
```

If you prefer to keep a single working copy, open `\\\\wsl$\\<distro>\\home\\<user>\\src\\StasisLang` in your editor (or use VS Code Remote - WSL).

### Optional: SSH clone instead of HTTPS

If you prefer SSH (no HTTPS tokens), create an SSH key in WSL and add it to GitHub:

```bash
ssh-keygen -t ed25519 -C "you@example.com"
eval "$(ssh-agent -s)"
ssh-add "$HOME/.ssh/id_ed25519"

# Add the public key to your GitHub account
gh ssh-key add "$HOME/.ssh/id_ed25519.pub" --title "wsl"

ssh -T git@github.com
```

Then clone via SSH:

```bash
git clone git@github.com:benwmaddox/StasisLang.git
```

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

### Troubleshooting: `Permission denied` running `.sh`

If you see `bash: ./dev_brickout_revenge_wsl.sh: Permission denied`, the script likely isn't marked executable in your working copy.

Fix it:

```bash
chmod +x ./dev_brickout_revenge_wsl.sh ./dev_brickout_revenge_v1_wsl.sh ./build.sh ./stasis.sh
./dev_brickout_revenge_wsl.sh
```

One-off workaround (doesn't require the executable bit):

```bash
bash ./dev_brickout_revenge_wsl.sh
```

### Diskless hot-swap (optional)

This avoids writing/loading hot-swap DLL/SO artifacts, which can reduce hot-swap latency:

```bash
cd tools/cranelift-jit-runner && cargo build --release
STASIS_CRANELIFT_JIT_RUNNER=1 ./dev_brickout_revenge_wsl.sh
```

## 5) Measuring hot-swap latency

By default, the `--watch` loop prints `HOTRELOAD phases(ms): ...` and `HOTSWAP latency(ms): ...` after an edit triggers a rebuild/swap.

If you want a single-line summary per swap (and to suppress the `HOTRELOAD phases(ms): ...` line), set:

```bash
STASIS_HOTSWAP_ONE_LINE=1
```

In that mode you'll get one line like:

- AOT hot-swap: `HOTSWAP(ms): total=... latency=... load=...`
- JIT hot-swap: `HOTSWAP(ms): total=... latency=...`

For apples-to-apples timing, use a repo-local workspace under WSL (not `/mnt/*`), make a single small edit, and compare the printed `HOTSWAP latency(ms)` between:

- Windows native (`dev_brickout_revenge*.bat`)
- WSL (`dev_brickout_revenge*_wsl.sh`)
