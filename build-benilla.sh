#!/usr/bin/env bash
# Install build prerequisites and compile every workspace binary.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
profile=release
install_system=1
install_toolchain=1
jobs=()

usage() {
    cat <<'EOF'
Usage: ./build-benilla.sh [options]

Install native dependencies and the repository's pinned Rust toolchain, then build
all workspace libraries and binaries (client, viewer, extractors and DBC tools).
Game data, a game server and GPU drivers are not downloaded.

Options:
  --profile NAME    release (default), dev, play or ship
  --jobs N          Limit parallel compiler jobs (useful on low-memory machines)
  --skip-system     Use existing native dependencies; do not run a package manager
  --skip-toolchain  Use cargo/rustc from PATH; do not install or update Rust
  -h, --help        Show this help without installing or building anything

Native dependency installation supports Debian/Ubuntu, Nobara/Fedora and Arch Linux
(including derivatives), and macOS with Homebrew and Xcode command line tools.
System packages may prompt for sudo. Rust is installed per user, never as root.
EOF
}

fail() {
    printf 'build-benilla: %s\n' "$*" >&2
    exit 1
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --profile)
            [ "$#" -ge 2 ] || fail "--profile needs a value"
            profile="$2"
            case "$profile" in release|dev|play|ship) ;; *) fail "unsupported profile: $profile" ;; esac
            shift 2 ;;
        --jobs)
            [ "$#" -ge 2 ] || fail "--jobs needs a positive integer"
            [[ "$2" =~ ^[1-9][0-9]*$ ]] || fail "--jobs needs a positive integer"
            jobs=(--jobs "$2")
            shift 2 ;;
        --skip-system) install_system=0; shift ;;
        --skip-toolchain) install_toolchain=0; shift ;;
        -h|--help) usage; exit 0 ;;
        *) fail "unknown option: $1 (see --help)" ;;
    esac
done

as_root() {
    if [ "$(id -u)" -eq 0 ]; then
        "$@"
    else
        command -v sudo >/dev/null || fail "sudo is required to install system packages"
        sudo "$@"
    fi
}

if [ "$install_system" -eq 1 ]; then
    case "$(uname -s)" in
        Linux)
            [ -r /etc/os-release ] || fail "cannot identify Linux distribution; use --skip-system"
            # Distribution-provided identifiers, not executable package-manager discovery.
            . /etc/os-release
            case " ${ID:-} ${ID_LIKE:-} " in
                *" debian "*|*" ubuntu "*)
                    as_root apt-get update
                    as_root apt-get install -y build-essential pkg-config libasound2-dev \
                        libudev-dev libclang-dev libwayland-dev libxkbcommon-dev libx11-dev \
                        libxi-dev libxrandr-dev libxcursor-dev libxinerama-dev libvulkan1 \
                        python3 curl ca-certificates git ;;
                *" nobara "*|*" fedora "*|*" rhel "*)
                    as_root dnf install -y gcc gcc-c++ make pkgconf-pkg-config \
                        alsa-lib-devel systemd-devel clang-devel wayland-devel \
                        libxkbcommon-devel libX11-devel libXi-devel libXrandr-devel \
                        libXcursor-devel libXinerama-devel vulkan-loader \
                        python3 curl ca-certificates git ;;
                *" arch "*)
                    as_root pacman -S --needed base-devel pkgconf alsa-lib systemd-libs \
                        clang wayland libxkbcommon libx11 libxi libxrandr libxcursor \
                        libxinerama vulkan-icd-loader python curl ca-certificates git ;;
                *) fail "unsupported Linux distribution: ${ID:-unknown}; install prerequisites and use --skip-system" ;;
            esac ;;
        Darwin)
            xcode-select -p >/dev/null 2>&1 ||
                fail "install Xcode command line tools with 'xcode-select --install', then rerun"
            command -v brew >/dev/null ||
                fail "install Homebrew from https://brew.sh, then rerun (or use --skip-system)"
            brew install pkg-config python ;;
        *) fail "unsupported OS; see docs/CONTRIBUTING.md for manual setup" ;;
    esac
fi

cd "$ROOT"
if [ "$install_toolchain" -eq 1 ]; then
    cargo_home="${CARGO_HOME:-$HOME/.cargo}"
    export PATH="$cargo_home/bin:$PATH"
    if ! command -v rustup >/dev/null; then
        command -v curl >/dev/null || fail "curl is required to download rustup"
        installer="$(mktemp "${TMPDIR:-/tmp}/benilla-rustup.XXXXXX")"
        trap 'rm -f "$installer"' EXIT
        curl --proto '=https' --tlsv1.2 --fail --show-error --location \
            https://sh.rustup.rs --output "$installer"
        sh "$installer" -y --profile minimal --default-toolchain none --no-modify-path
        rm -f "$installer"
        trap - EXIT
    fi
    # With no name, rustup reads the pin and components from rust-toolchain.toml.
    rustup toolchain install
    hash -r
fi

command -v cargo >/dev/null || fail "cargo is missing; rerun without --skip-toolchain"
command -v rustc >/dev/null || fail "rustc is missing; rerun without --skip-toolchain"
command -v cc >/dev/null || fail "a C compiler is missing; rerun without --skip-system"
command -v pkg-config >/dev/null || fail "pkg-config is missing; rerun without --skip-system"
command -v python3 >/dev/null || fail "python3 is missing; rerun without --skip-system"
if [ "$(uname -s)" = Linux ]; then
    pkg-config --exists alsa libudev ||
        fail "ALSA/udev development files are missing; rerun without --skip-system"
fi

rustc --version
cargo --version
cargo build --locked --workspace --bins --profile "$profile" "${jobs[@]}"
printf '\nBuild complete. Run ./run-benilla.sh (release), or use cargo run --profile %s -p benilla.\n' "$profile"
