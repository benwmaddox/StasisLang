#!/usr/bin/env bash
set -euo pipefail

DOTNET_CHANNEL="9.0"
DOTNET_INSTALL_DIR="${DOTNET_INSTALL_DIR:-$HOME/.dotnet}"
DOTNET_INSTALL_SCRIPT_URL="https://dot.net/v1/dotnet-install.sh"
DOTNET_INSTALL_SCRIPT_PATH="${TMPDIR:-/tmp}/dotnet-install.sh"

if command -v dotnet >/dev/null 2>&1; then
    CURRENT_DOTNET_VERSION="$(dotnet --version)"
    if [[ "${CURRENT_DOTNET_VERSION}" == 9.* ]]; then
        echo "dotnet ${CURRENT_DOTNET_VERSION} already installed."
        exit 0
    fi
fi

echo "Installing .NET SDK ${DOTNET_CHANNEL} into ${DOTNET_INSTALL_DIR}"

mkdir -p "${DOTNET_INSTALL_DIR}"

curl -fsSL "${DOTNET_INSTALL_SCRIPT_URL}" -o "${DOTNET_INSTALL_SCRIPT_PATH}"

bash "${DOTNET_INSTALL_SCRIPT_PATH}" \
    --channel "${DOTNET_CHANNEL}" \
    --install-dir "${DOTNET_INSTALL_DIR}"

echo ""
echo "Add .NET to your PATH for this shell:"
echo "  export PATH=\"${DOTNET_INSTALL_DIR}:\$PATH\""

echo ""
echo "Verify the install:"
echo "  dotnet --version"
