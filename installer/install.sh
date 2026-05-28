#!/bin/sh
# Marg one-line installer.
#
#   curl -fsSL https://github.com/SarthiAI/Marg/releases/latest/download/install.sh | sh
#
# What it does: detects the host OS and architecture, downloads the matching
# release archive from GitHub, verifies the SHA-256 against the published
# SHA256SUMS, installs the marg binary under /usr/local/bin, then runs
# `marg init --auto` to write a default config and mint the bootstrap admin
# token. On Linux as root with systemctl present, also installs and enables
# the bundled systemd unit.
#
# Configuration knobs (env vars):
#   MARG_VERSION   release tag to install (default: latest)
#   MARG_REPO      override the source repo (default: SarthiAI/Marg)
#   MARG_PREFIX    binary install prefix (default: /usr/local/bin)
#   MARG_DEST_DIR  config prefix passed to `marg init --config-dir` (default:
#                  /etc/marg if root else $HOME/.marg)
#   MARG_FORCE     1 to overwrite an existing /usr/local/bin/marg
#   MARG_NO_INIT   1 to skip `marg init --auto`
#   MARG_NO_SYSTEMD 1 to skip the systemd unit install

set -eu

if [ -n "${BASH_VERSION:-}" ]; then
    # The script runs under /bin/sh, but if invoked as `bash -c ...` we keep
    # a strict pipeline.
    set -o pipefail
fi

MARG_REPO="${MARG_REPO:-SarthiAI/Marg}"
MARG_VERSION="${MARG_VERSION:-latest}"
MARG_PREFIX="${MARG_PREFIX:-/usr/local/bin}"
MARG_FORCE="${MARG_FORCE:-0}"
MARG_NO_INIT="${MARG_NO_INIT:-0}"
MARG_NO_SYSTEMD="${MARG_NO_SYSTEMD:-0}"

# ---------- helpers --------------------------------------------------------

log() { printf '%s\n' "marg-install: $*" >&2; }
die() { log "error: $*"; exit 1; }

need_cmd() {
    command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

http_get() {
    # $1 url, $2 out path or - for stdout
    if command -v curl >/dev/null 2>&1; then
        if [ "$2" = "-" ]; then
            curl -fsSL "$1"
        else
            curl -fsSL "$1" -o "$2"
        fi
    elif command -v wget >/dev/null 2>&1; then
        if [ "$2" = "-" ]; then
            wget -q -O - "$1"
        else
            wget -q -O "$2" "$1"
        fi
    else
        die "neither curl nor wget is available"
    fi
}

sha256_check() {
    # $1 file $2 expected hex
    actual=""
    if command -v sha256sum >/dev/null 2>&1; then
        actual="$(sha256sum "$1" | awk '{print $1}')"
    elif command -v shasum >/dev/null 2>&1; then
        actual="$(shasum -a 256 "$1" | awk '{print $1}')"
    else
        die "no sha256 tool (need sha256sum or shasum)"
    fi
    [ "$actual" = "$2" ] || die "SHA256 mismatch for $1 (got $actual, expected $2)"
}

resolve_uid() {
    if command -v id >/dev/null 2>&1; then
        id -u 2>/dev/null || echo 1000
    else
        echo 1000
    fi
}

sudo_prefix() {
    if [ "$(resolve_uid)" = "0" ]; then
        printf ''
    elif command -v sudo >/dev/null 2>&1; then
        printf 'sudo '
    else
        die "no sudo available; re-run as root or set MARG_PREFIX to a user-writable dir"
    fi
}

detect_target() {
    os="$(uname -s 2>/dev/null || echo unknown)"
    arch="$(uname -m 2>/dev/null || echo unknown)"
    case "$os" in
        Linux)
            case "$arch" in
                x86_64|amd64)   target=linux-x64 ;;
                aarch64|arm64)  target=linux-arm64 ;;
                *) die "unsupported linux architecture: $arch" ;;
            esac
            ;;
        Darwin)
            case "$arch" in
                arm64|aarch64) target=macos-arm64 ;;
                *) die "unsupported macOS architecture: $arch (only arm64 / Apple Silicon ships)" ;;
            esac
            ;;
        *) die "unsupported operating system: $os" ;;
    esac
    printf '%s' "$target"
}

resolve_version() {
    if [ "$MARG_VERSION" = "latest" ]; then
        url="https://api.github.com/repos/${MARG_REPO}/releases/latest"
        tag="$(http_get "$url" - | grep -E '"tag_name"' | head -n 1 | sed -E 's/.*"tag_name": "([^"]+)".*/\1/')"
        [ -n "$tag" ] || die "could not resolve latest release tag from $url"
        printf '%s' "$tag"
    else
        # Accept either v0.1.0 or 0.1.0
        case "$MARG_VERSION" in
            v*) printf '%s' "$MARG_VERSION" ;;
            *)  printf 'v%s' "$MARG_VERSION" ;;
        esac
    fi
}

# ---------- main -----------------------------------------------------------

need_cmd uname
need_cmd tar
need_cmd awk

target="$(detect_target)"
tag="$(resolve_version)"
version="${tag#v}"

base="https://github.com/${MARG_REPO}/releases/download/${tag}"
archive="marg-${version}-${target}.tar.gz"
sums="SHA256SUMS"

log "host: $(uname -s)/$(uname -m), target: $target, version: $tag"
log "downloading $archive"

work="$(mktemp -d -t marg-install.XXXXXX)"
trap 'rm -rf "$work"' EXIT

http_get "${base}/${archive}" "${work}/${archive}"
http_get "${base}/${sums}" "${work}/${sums}"

# Pull the expected SHA for this archive out of SHA256SUMS.
expected="$(awk -v n="$archive" '$2 == n { print $1; exit }' "${work}/${sums}")"
[ -n "$expected" ] || die "no SHA256 entry for $archive in ${sums}"
sha256_check "${work}/${archive}" "$expected"

# Extract the archive (one folder named marg-<version>-<target>).
( cd "$work" && tar -xzf "$archive" )
src_dir="${work}/marg-${version}-${target}"
[ -x "${src_dir}/marg" ] || die "extracted archive missing marg binary at ${src_dir}/marg"

# Refuse to clobber an existing binary unless MARG_FORCE=1.
target_bin="${MARG_PREFIX}/marg"
if [ -e "$target_bin" ] && [ "$MARG_FORCE" != "1" ]; then
    die "$target_bin already exists. Re-run with MARG_FORCE=1 to overwrite."
fi

sudo="$(sudo_prefix)"
log "installing binary to $target_bin"
${sudo}install -m 0755 "${src_dir}/marg" "$target_bin"

# Verify the binary boots.
"$target_bin" --version >/dev/null || die "installed binary failed --version probe"

if [ "$MARG_NO_INIT" = "1" ]; then
    log "MARG_NO_INIT=1 set; skipping marg init"
    log "done. start with: $target_bin start --config <path>"
    exit 0
fi

init_args="--auto"
if [ -n "${MARG_DEST_DIR:-}" ]; then
    init_args="$init_args --config-dir ${MARG_DEST_DIR}"
fi

if [ "$MARG_NO_SYSTEMD" != "1" ] && [ "$(uname -s)" = "Linux" ] \
    && [ "$(resolve_uid)" = "0" ] && command -v systemctl >/dev/null 2>&1; then
    init_args="$init_args --systemd"
fi

log "running: ${target_bin} init ${init_args}"
${sudo}${target_bin} init ${init_args}

log "marg is installed. See the summary above for the admin token and URLs."
