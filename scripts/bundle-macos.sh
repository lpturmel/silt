#!/usr/bin/env bash
set -euo pipefail

# Builds an ad-hoc signed macOS Silt.app bundle containing:
#   - Contents/MacOS/silt      (GPUI desktop app)
#   - Contents/MacOS/siltd     (opt-in background collector)
#   - Contents/MacOS/silt-cli  (terminal companion)
#   - Contents/Info.plist

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUNDLE_DIR="${ROOT_DIR}/target/bundle/Silt.app"
MACOS_DIR="${BUNDLE_DIR}/Contents/MacOS"
RES_DIR="${BUNDLE_DIR}/Contents/Resources"

echo "==> Building release workspace binaries..."
cargo build --release --workspace --manifest-path "${ROOT_DIR}/Cargo.toml" "$@"

echo "==> Assembling ${BUNDLE_DIR}..."
rm -rf "${BUNDLE_DIR}"
mkdir -p "${MACOS_DIR}" "${RES_DIR}"

cp "${ROOT_DIR}/target/release/silt" "${MACOS_DIR}/silt"
cp "${ROOT_DIR}/target/release/siltd" "${MACOS_DIR}/siltd"
cp "${ROOT_DIR}/target/release/silt-cli" "${MACOS_DIR}/silt-cli"
cp "${ROOT_DIR}/assets/Info.plist.template" "${BUNDLE_DIR}/Contents/Info.plist"

chmod +x "${MACOS_DIR}/silt" "${MACOS_DIR}/siltd" "${MACOS_DIR}/silt-cli"

if command -v codesign >/dev/null 2>&1; then
    echo "==> Ad-hoc codesigning Silt.app for macOS TCC / Full Disk Access..."
    codesign --force --deep --sign - "${BUNDLE_DIR}"
fi

echo "==> Done! Launch with:"
echo "    open \"${BUNDLE_DIR}\""
echo "    Or copy to /Applications: cp -R \"${BUNDLE_DIR}\" /Applications/"
