# aris — build commands
# Usage: just <recipe>

set shell := ["bash", "-c"]
set windows-shell := ["bash.exe", "-c"]
set unstable
set lists
# python_cmd is defined both below (fresh-clone fallback) and in the staged
# .just/celestia-devtools.just, so duplicates must be allowed. Verified with
# just 1.55.1: with allow-duplicate-variables the MAIN justfile's definition
# always wins over an imported one, regardless of position — so the fallback
# below MUST mirror the devtools expression, or it would silently override it.
set allow-duplicate-variables

# Shared celestia-devtools recipes — NOT in git. This justfile references shared
# variables, so the import is REQUIRED. Bootstrap once: celestia-devtools init
# (or `just fetch` if already staged). Refresh after upgrades.
#
# Fallback for fresh clones before .just/ is staged. Mirrors the expression in
# .just/celestia-devtools.just: on Windows prefer `python` — `python3` is often
# the broken WindowsApps Store stub that exits 49; on Unix prefer `python3`.
# Keep in sync when refreshing .just/ (see the note above).
python_cmd := if os_family() == "windows" {
    if which("python") != "" { "python" } else { "python3" }
} else {
    if which("python3") != "" { "python3" } else { "python" }
}
import? "./.just/git-bash-interop.just"
import? "./.just/celestia-devtools.just"

# Stage shared celestia-devtools recipes into .just/ (gitignored).
# Source order: explicit URL arg → local pip bundle (offline) → GitHub raw.
# curl honors HTTP_PROXY/HTTPS_PROXY/ALL_PROXY env vars automatically.
[script('bash')]
fetch URL='':
    #!/usr/bin/env bash
    set -euo pipefail
    out=.just/celestia-devtools.just
    mkdir -p .just
    if [ -n "{{URL}}" ]; then
      echo "[fetch] {{URL}} -> $out"
      curl -fsSL "{{URL}}" -o "$out"
    elif command -v celestia-devtools >/dev/null 2>&1; then
      src=$(celestia-devtools include-path)
      echo "[fetch] local bundle ($src) -> $out"
      cp "$src" "$out"
    else
      echo "[fetch] github raw -> $out"
      curl -fsSL "https://raw.githubusercontent.com/celestia-island/celestia-devtools/dev/src/celestia_devtools/common.just" -o "$out"
    fi
    echo "[fetch] wrote $out"

default: build

# ── Environment ─────────────────────────────────────────────

# Inspect the build environment: host kind, WSL2 distros (on Windows),
# selected distro, and container backend. Pre-flight check before build.
env-check:
    {{python_cmd}} scripts/check_env.py

# Build/fetch the HMI browser engine (webkitgtk | servo | cef) per [display]
# config. Renders the gateway dashboard on an attached screen.
build-browser BOARD="nanopi-r3s":
    {{python_cmd}} scripts/build_browser.py {{BOARD}}

# ── Development ────────────────────────────────────────────

check:
    cargo check --workspace

lint:
    cargo clippy --workspace -- -D warnings

test:
    cargo test --workspace

# Format Rust + Markdown docs
fmt:
    cargo fmt --all
    just fmt-markdown

# Check formatting without modifying
fmt-check:
    cargo fmt --all -- --check
    just fmt-markdown --check

# ── Cross-compilation Setup ────────────────────────────────

setup-cross:
    {{python_cmd}} scripts/setup_cross.py

# ── Build ──────────────────────────────────────────────────

# Build firmware with kei kernel (default, Phase 2)
build:
    just cache-guard
    {{python_cmd}} scripts/build.py nanopi-r3s --kernel-source kei

# Build firmware with Linux kernel (Phase 1)
build-linux:
    just cache-guard
    {{python_cmd}} scripts/build.py nanopi-r3s --kernel-source linux

build-board BOARD:
    {{python_cmd}} scripts/build.py {{BOARD}} --kernel-source kei

# ── Flash ──────────────────────────────────────────────────

flash-sd DEVICE="/dev/sdb":
    {{python_cmd}} scripts/flash_sd.py {{DEVICE}}

flash-board BOARD DEVICE="/dev/sdb":
    {{python_cmd}} scripts/flash_sd.py -b {{BOARD}} {{DEVICE}}

# ── Testing ────────────────────────────────────────────────

# First ignition test: evernight-server + Modbus sim + sensor-poll (host, no QEMU)
ignition-test:
    {{python_cmd}} scripts/ignition_test.py

# QEMU ignition test with Linux kernel backend (baseline)
qemu-ignition-linux:
    {{python_cmd}} scripts/qemu_ignition.py --kernel linux

# QEMU ignition test with kei kernel backend (experimental)
qemu-ignition-kei:
    {{python_cmd}} scripts/qemu_ignition.py --kernel kei

# QEMU ignition test with official asterinas backend
qemu-ignition-asterinas:
    {{python_cmd}} scripts/qemu_ignition.py --kernel asterinas

qemu-test:
    {{python_cmd}} scripts/qemu_test.py nanopi-r3s

# QEMU desktop test with HMI display (webkitgtk/servo kiosk browser)
qemu-desktop BOARD="qemu-hmi":
    {{python_cmd}} scripts/qemu_desktop.py {{BOARD}}

# QEMU desktop test with kei kernel backend (experimental)
qemu-desktop-kei BOARD="qemu-hmi":
    {{python_cmd}} scripts/qemu_desktop.py {{BOARD}} --kernel-source kei

hw-test:
    cargo test --test hardware -- --test-threads=1

# ── Utilities ──────────────────────────────────────────────

# ── Testing ────────────────────────────────────────────────

# Run all USB gadget tests
test-gadget:
    python3 tests/run_all.py

# Quick test run (skip image build and QEMU)
test-quick:
    python3 tests/run_all.py --quick

# Build the USB mass-storage installer image (exposed to hosts via USB-C)
build-installer-image OUTPUT="output/installer.img" EVERNIGHT_DIR="output/evernight-binaries":
    bash scripts/package/build_installer_image.sh {{OUTPUT}} {{EVERNIGHT_DIR}}

# Create fixture binaries for testing
create-fixtures:
    bash tests/fixtures/create_fixtures.sh

# ── Windows Testing ──────────────────────────────────────

# Test Windows installer via Wine (fast, no VM needed)
test-windows-wine:
    python3 tests/installer/test_windows_wine.py

# Install Windows DLLs via winetricks for better Wine compatibility
wine-setup:
    export WINEPREFIX="${WINEPREFIX:-$$HOME/.wine-aris}"
    export WINE=/usr/lib/wine/wine64
    /tmp/winetricks corefonts vcrun2019

# Run a Windows batch file through Wine
wine-bat BAT:
    export WINEPREFIX="${WINEPREFIX:-$$HOME/.wine-aris}"
    export WINE=/usr/lib/wine/wine64
    /usr/lib/wine/wine64 cmd /c "Z:$(realpath {{BAT}} | tr / '\')"

# QEMU Windows VM: check status
windows-status:
    python3 tests/windows/setup_vm.py --status

# QEMU Windows VM: auto-download Win11 eval ISO + setup VM (one-time, ~6.6GB)
windows-setup:
    python3 tests/windows/setup_vm.py --auto-download

# QEMU Windows VM: boot and run USB gadget test (requires --download first)
windows-test:
    python3 tests/windows/setup_vm.py --test

# QEMU Windows VM: boot interactively (VNC on localhost:5900)
windows-interactive:
    python3 tests/windows/setup_vm.py --interactive

dev-shell:
    {{python_cmd}} scripts/dev_shell.py

# ── Development ────────────────────────────────────────────

# Launch the aris browser in a winit desktop window (with JS + networking).
dev:
    cargo run -p aris-render --features "desktop winit js" --bin aris_browser

dev-html FILE:
    cargo run -p aris-render --features "desktop winit js" --bin aris_browser -- {{FILE}}

dev-render:
    cargo run -p aris-render --bin render_lagrange -- tests/fixtures/lagrange_index.html

dev-wasm:
    cargo run -p aris-wasm --bin render_wasm -- tests/fixtures/tairitsu_website.wasm

clean:
    rm -rf output/ target/ build/
    cargo clean

# ── Conformance ────────────────────────────────────────────

# Run the W3C-style conformance suite and regenerate the report.
conformance:
    RUST_LOG="" cargo run -p aris-render --features "desktop winit js" --bin conformance_test > /tmp/aris_conformance.json
    python scripts/conformance/report.py /tmp/aris_conformance.json > docs/guides/conformance-report.md
    @echo "Report written to docs/guides/conformance-report.md"

# Run real W3C web-platform-tests (DOM subset) and regenerate the report.
wpt-dom DIR="tests/wpt/wpt-master/dom":
    RUST_LOG="" cargo run -p aris-render --features "desktop winit js" --bin wpt_runner -- {{DIR}} > /tmp/aris_wpt.json
    python scripts/conformance/wpt_report.py /tmp/aris_wpt.json > docs/guides/wpt-report.md
    @echo "WPT report written to docs/guides/wpt-report.md"
    head -5 docs/guides/wpt-report.md
