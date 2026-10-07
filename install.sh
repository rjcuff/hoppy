#!/bin/sh
# shellcheck shell=dash
# shellcheck disable=SC3043 # Assume `local` extension

# The hoppy installer.
#
# Downloads the latest release binary for this machine and puts it on disk.
# Runs on POSIX shells that support `local` (sh, bash, dash, zsh, ksh).

HOPPY_REPO="${HOPPY_REPO:-rjcuff/hoppy}"

main() {
    set -u

    parse_args "$@"

    local _target
    _target="${_HOPPY_TARGET:-$(get_target)}" || exit $?
    assert_nz "${_target}" "target"
    echo "Detected platform: ${_target}"

    local _bin_name
    case "${_target}" in
    *windows*) _bin_name="hoppy.exe" ;;
    *) _bin_name="hoppy" ;;
    esac

    local _tmp_dir
    _tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/hoppy_XXXXXX")" || err "mktemp: could not create temporary directory"
    # shellcheck disable=SC2064 # Expand now: the variable is local.
    trap "rm -rf -- '${_tmp_dir}'" EXIT
    cd "${_tmp_dir}" || err "cd: failed to enter directory: ${_tmp_dir}"

    local _package
    _package="$(download_hoppy "${_target}")" || exit $?
    assert_nz "${_package}" "package"
    echo "Downloaded package: ${_package}"
    case "${_package}" in
    *.tar.gz)
        need_cmd tar
        ensure tar -xzf "${_package}"
        ;;
    *.zip)
        need_cmd unzip
        ensure unzip -oq "${_package}"
        ;;
    *)
        err "unsupported package format: ${_package}"
        ;;
    esac
    [ -f "${_bin_name}" ] || err "the package did not contain ${_bin_name}"

    ensure try_sudo mkdir -p -- "${_HOPPY_BIN_DIR}"
    ensure try_sudo cp -- "${_bin_name}" "${_HOPPY_BIN_DIR}/${_bin_name}"
    ensure try_sudo chmod +x "${_HOPPY_BIN_DIR}/${_bin_name}"
    echo "Installed hoppy to ${_HOPPY_BIN_DIR}"

    echo ""
    echo "hoppy is installed! Run \`hoppy\` to see your network."
    if ! echo ":${PATH}:" | grep -Fq ":${_HOPPY_BIN_DIR}:"; then
        echo "Note: ${_HOPPY_BIN_DIR} is not on your \$PATH. Add it, or hoppy will not be found."
    fi
}

parse_args() {
    _HOPPY_BIN_DIR_DEFAULT="${HOME}/.local/bin"
    _HOPPY_SUDO_DEFAULT="sudo"

    _HOPPY_BIN_DIR="${_HOPPY_BIN_DIR_DEFAULT}"
    _HOPPY_SUDO="${_HOPPY_SUDO_DEFAULT}"

    while [ "$#" -gt 0 ]; do
        case "$1" in
        --target) need_value "$@" && _HOPPY_TARGET="$2" && shift 2 ;;
        --target=*) _HOPPY_TARGET="${1#*=}" && shift 1 ;;
        --bin-dir) need_value "$@" && _HOPPY_BIN_DIR="$2" && shift 2 ;;
        --bin-dir=*) _HOPPY_BIN_DIR="${1#*=}" && shift 1 ;;
        --sudo) need_value "$@" && _HOPPY_SUDO="$2" && shift 2 ;;
        --sudo=*) _HOPPY_SUDO="${1#*=}" && shift 1 ;;
        -h | --help) usage && exit 0 ;;
        *) err "unknown option: $1 (try --help)" ;;
        esac
    done
}

need_value() {
    if [ "$#" -lt 2 ]; then err "option $1 needs a value"; fi
}

usage() {
    local _target
    _target="$(get_target 2>/dev/null || echo unknown)"

    echo "\
hoppy installer
https://github.com/${HOPPY_REPO}

Fetches and installs hoppy. If hoppy is already installed, it is updated to the
latest release.

Usage:
  install.sh [OPTIONS]

Options:
      --target   Override the detected platform [current: ${_target}]
      --bin-dir  Override the installation directory [default: ${_HOPPY_BIN_DIR_DEFAULT}]
      --sudo     Override the command used to elevate privileges [default: ${_HOPPY_SUDO_DEFAULT}]
  -h, --help     Print help"
}

# Download the release asset for a target into the current directory and
# print its file name.
download_hoppy() {
    local _target="$1"
    local _dld

    if check_cmd curl; then
        _dld=curl
    elif check_cmd wget; then
        _dld=wget
    else
        err "need 'curl' or 'wget' (command not found)"
    fi
    need_cmd grep
    need_cmd cut

    local _releases_url="https://api.github.com/repos/${HOPPY_REPO}/releases/latest"
    local _releases
    case "${_dld}" in
    curl) _releases="$(curl -sSfL "${_releases_url}")" ||
        err "could not read ${_releases_url}. Is there a published release yet?" ;;
    wget) _releases="$(wget -qO- "${_releases_url}")" ||
        err "could not read ${_releases_url}. Is there a published release yet?" ;;
    esac
    if echo "${_releases}" | grep -q 'API rate limit exceeded'; then
        err "GitHub's API rate limit was hit. Try again later, or install with: cargo install --git https://github.com/${HOPPY_REPO}"
    fi

    local _package_url
    _package_url="$(echo "${_releases}" | grep "browser_download_url" | cut -d '"' -f 4 | grep -- "${_target}" | head -n 1)"
    if [ -z "${_package_url}" ]; then
        err "there is no hoppy build for ${_target} yet. Install with: cargo install --git https://github.com/${HOPPY_REPO}"
    fi

    local _ext
    case "${_package_url}" in
    *.tar.gz) _ext="tar.gz" ;;
    *.zip) _ext="zip" ;;
    *) err "unsupported package format: ${_package_url}" ;;
    esac

    local _package="hoppy.${_ext}"
    case "${_dld}" in
    curl) curl -sSfLo "${_package}" "${_package_url}" || err "curl: failed to download ${_package_url}" ;;
    wget) wget -qO "${_package}" "${_package_url}" || err "wget: failed to download ${_package_url}" ;;
    esac

    echo "${_package}"
}

# Map `uname` output to the Rust target triple used in release asset names.
get_target() {
    local _os _cpu
    _os="$(uname -s)"
    _cpu="$(uname -m)"

    case "${_cpu}" in
    x86_64 | x86-64 | x64 | amd64) _cpu=x86_64 ;;
    aarch64 | arm64) _cpu=aarch64 ;;
    *) err "unsupported CPU: ${_cpu}. Install with cargo instead: cargo install --git https://github.com/${HOPPY_REPO}" ;;
    esac

    case "${_os}" in
    Linux) _os=unknown-linux-musl ;;
    Darwin)
        # Under Rosetta, `uname -m` reports x86_64 on Apple silicon.
        if [ "${_cpu}" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
            _cpu=aarch64
        fi
        _os=apple-darwin
        ;;
    MINGW* | MSYS* | CYGWIN* | Windows_NT) _os=pc-windows-msvc ;;
    *) err "unsupported OS: ${_os}. Install with cargo instead: cargo install --git https://github.com/${HOPPY_REPO}" ;;
    esac

    echo "${_cpu}-${_os}"
}

# Run a command as-is, and retry with sudo only if that fails.
try_sudo() {
    if "$@" >/dev/null 2>&1; then
        return 0
    fi

    need_sudo
    "${_HOPPY_SUDO}" "$@"
}

need_sudo() {
    if ! check_cmd "${_HOPPY_SUDO}"; then
        err "\
could not find \`${_HOPPY_SUDO}\`, needed to write to ${_HOPPY_BIN_DIR}.

Pick a directory you own with --bin-dir, or rerun as root / Administrator."
    fi

    if ! "${_HOPPY_SUDO}" -v; then
        err "sudo permissions not granted, aborting installation"
    fi
}

need_cmd() {
    if ! check_cmd "$1"; then
        err "need '$1' (command not found)"
    fi
}

check_cmd() {
    command -v -- "$1" >/dev/null 2>&1
}

ensure() {
    if ! "$@"; then err "command failed: $*"; fi
}

assert_nz() {
    if [ -z "$1" ]; then err "found empty string: $2"; fi
}

err() {
    echo "Error: $1" >&2
    exit 1
}

# Braces make sure nothing runs until the whole script has downloaded.
{
    main "$@" || exit 1
}
