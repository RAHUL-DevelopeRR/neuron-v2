#!/bin/sh
# Neuron installer for Linux and macOS.
#
# Downloads a prebuilt `neuron` binary from GitHub Releases, verifies it
# against the release's SHA-256 file, and installs it into a bin directory.
# Beside it goes `neuron-tui`, the full-screen UI that `neuron` opens on a
# terminal; nothing about the UI fails the install (without it, `neuron` runs
# its line REPL).
#
#   curl -fsSL https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/latest/download/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- --version 7.0.0-alpha.1 --install-dir "$HOME/bin"
#
# Environment:
#   NEURON_VERSION         release to install, e.g. 7.0.0-alpha.1 (default: latest release)
#   NEURON_INSTALL_DIR     install directory (default: $HOME/.local/bin)
#   NEURON_DOWNLOAD_BASE   release download base URL (default: GitHub Releases);
#                          must be HTTPS unless it points at a loopback host
#   NEURON_RELEASES_API    URL of the "latest release" JSON (default: GitHub API)
#   GITHUB_TOKEN           optional token for the GitHub API rate limit
#   NEURON_NO_TUI          1 skips the full-screen UI
#
# Windows: use install.ps1 from PowerShell instead.

set -eu

REPOSITORY="RAHUL-DevelopeRR/neuron-v2"
DEFAULT_DOWNLOAD_BASE="https://github.com/${REPOSITORY}/releases/download"
DEFAULT_RELEASES_API="https://api.github.com/repos/${REPOSITORY}/releases/latest"
SUPPORTED="Linux (x64, arm64), macOS (arm64, x64) and Windows (x64)"

if [ -t 2 ] && [ -z "${NO_COLOR:-}" ]; then
    BOLD="$(printf '\033[1m')"
    RED="$(printf '\033[31m')"
    GREEN="$(printf '\033[32m')"
    YELLOW="$(printf '\033[33m')"
    RESET="$(printf '\033[0m')"
else
    BOLD=""
    RED=""
    GREEN=""
    YELLOW=""
    RESET=""
fi

say() { printf '%s\n' "$*" >&2; }
info() { printf '  %s\n' "$*" >&2; }
warn() { printf '%swarning:%s %s\n' "${YELLOW}" "${RESET}" "$*" >&2; }
die() {
    printf '%serror:%s %s\n' "${RED}" "${RESET}" "$*" >&2
    exit 1
}

usage() {
    cat <<'EOF'
Install the Neuron coding agent from GitHub Releases.

Usage: install.sh [--version VERSION] [--install-dir DIR] [--help]

Options:
  --version VERSION    Release to install, e.g. 7.0.0-alpha.1 (default: latest)
  --install-dir DIR    Directory for the neuron binary (default: ~/.local/bin)
  -h, --help           Show this help

Environment: NEURON_VERSION, NEURON_INSTALL_DIR, NEURON_DOWNLOAD_BASE,
NEURON_RELEASES_API, GITHUB_TOKEN.
EOF
}

VERSION="${NEURON_VERSION:-}"
INSTALL_DIR="${NEURON_INSTALL_DIR:-}"

while [ "$#" -gt 0 ]; do
    case "$1" in
        --version)
            [ "$#" -ge 2 ] || die "--version needs a value"
            VERSION="$2"
            shift 2
            ;;
        --version=*)
            VERSION="${1#--version=}"
            shift
            ;;
        --install-dir)
            [ "$#" -ge 2 ] || die "--install-dir needs a value"
            INSTALL_DIR="$2"
            shift 2
            ;;
        --install-dir=*)
            INSTALL_DIR="${1#--install-dir=}"
            shift
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            usage >&2
            die "unknown argument: $1"
            ;;
    esac
done

if [ -z "${INSTALL_DIR}" ]; then
    [ -n "${HOME:-}" ] || die "HOME is not set; pass --install-dir or set NEURON_INSTALL_DIR"
    INSTALL_DIR="${HOME}/.local/bin"
fi
if [ "${INSTALL_DIR}" != "/" ]; then
    INSTALL_DIR="${INSTALL_DIR%/}"
fi

# --- platform ---------------------------------------------------------------

detect_target() {
    os="$(uname -s 2>/dev/null || echo unknown)"
    arch="$(uname -m 2>/dev/null || echo unknown)"
    case "${os}" in
        Linux) os_part="linux" ;;
        Darwin) os_part="macos" ;;
        MINGW* | MSYS* | CYGWIN* | Windows_NT)
            die "this is a Windows shell (${os}). Install from PowerShell instead:
    irm https://github.com/${REPOSITORY}/releases/latest/download/install.ps1 | iex"
            ;;
        *) die "unsupported operating system: ${os}. Neuron publishes binaries for ${SUPPORTED}." ;;
    esac
    case "${arch}" in
        x86_64 | amd64) arch_part="x64" ;;
        aarch64 | arm64) arch_part="arm64" ;;
        *) die "unsupported CPU architecture: ${arch} on ${os}. Neuron publishes binaries for ${SUPPORTED}." ;;
    esac
    if [ "${os_part}" = "macos" ] && [ "${arch_part}" = "x64" ]; then
        # A shell running under Rosetta reports x86_64 on Apple silicon.
        if [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = "1" ]; then
            arch_part="arm64"
        fi
    fi
    if [ "${os_part}" = "linux" ] && command -v ldd >/dev/null 2>&1; then
        if ldd --version 2>&1 | grep -qi musl; then
            die "musl-based Linux (for example Alpine) is not supported; Neuron's Linux binaries need glibc."
        fi
    fi
    printf '%s-%s\n' "${os_part}" "${arch_part}"
}

# --- downloads --------------------------------------------------------------

# is_allowed_url URL: HTTPS, or plain HTTP to a loopback host (tests and local mirrors).
is_allowed_url() {
    case "$1" in
        https://*) rest="${1#https://}" ;;
        http://*) rest="${1#http://}" ;;
        *) return 1 ;;
    esac
    authority="${rest%%/*}"
    case "${authority}" in
        # userinfo: for http://127.0.0.1:x@evil.example/ curl contacts evil.example
        *@*) return 1 ;;
    esac
    case "$1" in
        https://*) return 0 ;;
    esac
    case "${authority}" in
        \[*) host="${authority%%]*}]" ;;
        *) host="${authority%%:*}" ;;
    esac
    port="${authority#"${host}"}"
    case "${port}" in
        "") ;;
        : | :*[!0-9]*) return 1 ;;
        :*) ;;
        *) return 1 ;;
    esac
    case "${host}" in
        127.0.0.1 | localhost | "[::1]") return 0 ;;
        *) return 1 ;;
    esac
}

if command -v curl >/dev/null 2>&1; then
    DOWNLOADER="curl"
elif command -v wget >/dev/null 2>&1; then
    DOWNLOADER="wget"
else
    die "neither curl nor wget is installed; install one of them and retry."
fi

# fetch URL OUTPUT
fetch() {
    if [ "${DOWNLOADER}" = "curl" ]; then
        curl --fail --silent --show-error --location --retry 3 --connect-timeout 20 -o "$2" "$1"
    else
        wget --quiet --tries=3 --timeout=30 -O "$2" "$1"
    fi
}

# fetch_releases_api URL OUTPUT: GITHUB_TOKEN (for the API rate limit) is sent only to
# api.github.com, and through a curl config on stdin so it never shows up in argv.
fetch_releases_api() {
    case "$1" in
        https://api.github.com/*) ;;
        *)
            fetch "$1" "$2"
            return
            ;;
    esac
    case "${GITHUB_TOKEN:-}" in
        "" | *[!A-Za-z0-9_.-]*)
            fetch "$1" "$2"
            return
            ;;
    esac
    if [ "${DOWNLOADER}" != "curl" ]; then
        # wget would need the token on its command line; the anonymous limit will do.
        fetch "$1" "$2"
        return
    fi
    printf 'header = "Authorization: Bearer %s"\n' "${GITHUB_TOKEN}" |
        curl --fail --silent --show-error --location --retry 3 --connect-timeout 20 --config - -o "$2" "$1"
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        die "neither sha256sum nor shasum is installed; cannot verify the download."
    fi
}

resolve_latest_version() {
    api="${NEURON_RELEASES_API:-${DEFAULT_RELEASES_API}}"
    is_allowed_url "${api}" || die "NEURON_RELEASES_API must be an HTTPS URL (or loopback HTTP): ${api}"
    if ! fetch_releases_api "${api}" "${TMP_DIR}/latest.json"; then
        die "could not look up the latest Neuron release at ${api}.
    Set NEURON_VERSION (for example NEURON_VERSION=7.0.0-alpha.1) to skip the lookup."
    fi
    tag="$(sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "${TMP_DIR}/latest.json" | head -n 1)"
    [ -n "${tag}" ] || die "the latest-release response from ${api} has no tag_name; set NEURON_VERSION."
    printf '%s\n' "${tag#v}"
}

# --- main -------------------------------------------------------------------

TARGET="$(detect_target)"
ASSET="neuron-${TARGET}"

DOWNLOAD_BASE="${NEURON_DOWNLOAD_BASE:-${DEFAULT_DOWNLOAD_BASE}}"
DOWNLOAD_BASE="${DOWNLOAD_BASE%/}"
is_allowed_url "${DOWNLOAD_BASE}" || die "NEURON_DOWNLOAD_BASE must be an HTTPS URL (or loopback HTTP): ${DOWNLOAD_BASE}"

TMP_DIR="$(mktemp -d 2>/dev/null || mktemp -d -t neuron-install)"
STAGED=""
trap 'rm -rf "${TMP_DIR}"; if [ -n "${STAGED}" ]; then rm -f "${STAGED}"; fi' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

if [ -z "${VERSION}" ]; then
    VERSION="$(resolve_latest_version)"
fi
VERSION="${VERSION#v}"
if ! printf '%s\n' "${VERSION}" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$'; then
    die "invalid version: ${VERSION} (expected something like 7.0.0-alpha.1)"
fi

URL="${DOWNLOAD_BASE}/v${VERSION}/${ASSET}"
say "${BOLD}Installing Neuron ${VERSION} (${TARGET})${RESET}"
info "from ${URL}"

# fetch_or_die URL OUTPUT: an HTTP error status (curl 22, wget 8) means the asset is
# missing; anything else is a network problem.
fetch_or_die() {
    status=0
    fetch "$1" "$2" || status=$?
    case "${status}" in
        0) return 0 ;;
        22 | 8)
            die "could not download $1 (HTTP error).
    Neuron ${VERSION} may not be published for ${TARGET}; see https://github.com/${REPOSITORY}/releases"
            ;;
        *)
            die "could not download $1 (${DOWNLOADER} exit ${status}).
    Check your network or proxy settings (HTTPS_PROXY) and retry."
            ;;
    esac
}

fetch_or_die "${URL}.sha256" "${TMP_DIR}/${ASSET}.sha256"
EXPECTED="$(awk 'NR == 1 {print tolower($1)}' "${TMP_DIR}/${ASSET}.sha256")"
if ! printf '%s\n' "${EXPECTED}" | grep -Eq '^[0-9a-f]{64}$'; then
    die "${URL}.sha256 does not contain a SHA-256 checksum."
fi

fetch_or_die "${URL}" "${TMP_DIR}/${ASSET}"
ACTUAL="$(sha256_of "${TMP_DIR}/${ASSET}")"
if [ "${ACTUAL}" != "${EXPECTED}" ]; then
    die "checksum mismatch for ${ASSET}: expected ${EXPECTED}, got ${ACTUAL}. Nothing was installed."
fi
info "verified SHA-256 ${ACTUAL}"

# Stage and self-check inside the install directory, not the temporary directory:
# /tmp is often mounted noexec, and the final rename stays on one filesystem.
mkdir -p "${INSTALL_DIR}" 2>/dev/null || die "cannot create ${INSTALL_DIR}; choose another directory with --install-dir."
DESTINATION="${INSTALL_DIR}/neuron"
STAGED="${INSTALL_DIR}/.neuron.$$.tmp"
if ! cp "${TMP_DIR}/${ASSET}" "${STAGED}" 2>/dev/null; then
    die "cannot write to ${INSTALL_DIR}; choose another directory with --install-dir."
fi
chmod 755 "${STAGED}"
# </dev/null: under `curl | sh` stdin is the rest of this script.
if ! VERSION_OUTPUT="$("${STAGED}" --version </dev/null 2>&1)"; then
    printf '%s\n' "${VERSION_OUTPUT}" >&2
    die "the downloaded binary does not run on this system (or ${INSTALL_DIR} is on a noexec mount). Nothing was installed."
fi
mv -f "${STAGED}" "${DESTINATION}"
STAGED=""

# The full-screen UI, verified and self-checked like `neuron`; returns 1 (and
# installs nothing) on any failure.
install_ui() {
    UI_ASSET="neuron-tui-${TARGET}"
    UI_URL="${DOWNLOAD_BASE}/v${VERSION}/${UI_ASSET}"
    fetch "${UI_URL}.sha256" "${TMP_DIR}/${UI_ASSET}.sha256" || return 1
    UI_EXPECTED="$(awk 'NR == 1 {print tolower($1)}' "${TMP_DIR}/${UI_ASSET}.sha256")"
    printf '%s\n' "${UI_EXPECTED}" | grep -Eq '^[0-9a-f]{64}$' || return 1
    fetch "${UI_URL}" "${TMP_DIR}/${UI_ASSET}" || return 1
    if [ "$(sha256_of "${TMP_DIR}/${UI_ASSET}")" != "${UI_EXPECTED}" ]; then
        warn "checksum mismatch for ${UI_ASSET}; it was not installed."
        return 1
    fi
    STAGED="${INSTALL_DIR}/.neuron-tui.$$.tmp"
    cp "${TMP_DIR}/${UI_ASSET}" "${STAGED}" 2>/dev/null || return 1
    chmod 755 "${STAGED}"
    if ! "${STAGED}" --version </dev/null >/dev/null 2>&1; then
        rm -f "${STAGED}"
        STAGED=""
        return 1
    fi
    mv -f "${STAGED}" "${INSTALL_DIR}/neuron-tui"
    STAGED=""
}

case "$(printf '%s' "${NEURON_NO_TUI:-}" | tr '[:upper:]' '[:lower:]')" in
    "" | 0 | false | no)
        info "installing the full-screen UI (neuron-tui) ..."
        if install_ui; then
            info "installed ${INSTALL_DIR}/neuron-tui"
        else
            warn "the full-screen UI was not installed; neuron uses its line REPL. Rerun the installer to try again."
        fi
        ;;
    *) info "NEURON_NO_TUI is set; skipped the full-screen UI." ;;
esac

# The release binary prints a report whose second line is "  Version  X.Y.Z".
REPORTED="$(printf '%s\n' "${VERSION_OUTPUT}" | sed -n 's/^[[:space:]]*Version[[:space:]][[:space:]]*\([^[:space:]][^[:space:]]*\).*/\1/p' | head -n 1)"
say "${GREEN}Installed${RESET} ${DESTINATION}"
if [ -n "${REPORTED}" ]; then
    info "neuron ${REPORTED}"
else
    info "$(printf '%s\n' "${VERSION_OUTPUT}" | head -n 1)"
fi

case ":${PATH:-}:" in
    *":${INSTALL_DIR}:"* | *":${INSTALL_DIR}/:"*)
        RESOLVED="$(command -v neuron 2>/dev/null || true)"
        if [ -n "${RESOLVED}" ] && [ "${RESOLVED}" != "${DESTINATION}" ]; then
            warn "another neuron at ${RESOLVED} comes first on PATH and runs instead of ${DESTINATION}."
            info "Remove it (for example: pip uninstall neuroncli, or npm uninstall -g @zero-x.live/neuron) or reorder PATH."
        fi
        RUN="neuron"
        ;;
    *)
        warn "${INSTALL_DIR} is not on your PATH."
        case "$(basename "${SHELL:-sh}")" in
            zsh) PROFILE="\$HOME/.zshrc" ;;
            bash) PROFILE="\$HOME/.bashrc" ;;
            fish) PROFILE="" ;;
            *) PROFILE="\$HOME/.profile" ;;
        esac
        if [ -n "${PROFILE}" ]; then
            info "Add this line to ${PROFILE}, then open a new terminal:"
            info "  export PATH=\"${INSTALL_DIR}:\$PATH\""
        else
            info "Add it with: fish_add_path ${INSTALL_DIR}"
        fi
        RUN="${DESTINATION}"
        ;;
esac

cat >&2 <<EOF

Next steps:
  ${RUN} auth login     # sign in to Zero-X (a free plan is available)
  ${RUN}                # start a session in your project directory

Docs: https://github.com/${REPOSITORY}#readme
EOF
