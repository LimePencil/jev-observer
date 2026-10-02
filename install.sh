#!/bin/sh
# Install a verified release without sudo, shell-profile changes, or autostart.
# Keep execution in main(), called at EOF, so a truncated download cannot install.

usage() {
    cat <<'EOF'
Usage: sh install.sh [--version VERSION] [--install-dir PATH]

Install the latest Jev Observer release to ~/.local/bin.

  --version VERSION   Pin a release, such as 0.1.1 or v0.1.1
  --install-dir PATH  Choose the directory for the jev-observer executable
  --help              Show this help

Linux x86_64/arm64 and macOS Intel/Apple Silicon are supported when the
corresponding release asset is published. No compiler or Node.js is needed.

For a private repository, authenticate GitHub CLI with `gh auth login`, then:
  gh api --hostname github.com repos/LimePencil/jev-observer/contents/install.sh \
    -H 'Accept: application/vnd.github.raw+json' | sh

Authenticated GitHub CLI is preferred for the official repository. HTTPS
downloads otherwise use curl or GNU wget. SHA-256 verification is mandatory.
JEV_OBSERVER_RELEASE_BASE_URL can point to an HTTPS release mirror with the
same latest/download and download/vVERSION layout. HTTP is disabled unless
JEV_OBSERVER_ALLOW_INSECURE_HTTP=1 is explicitly set for local fixture tests.

Build instructions: https://github.com/LimePencil/jev-observer/blob/main/README.md
EOF
}

fail() {
    printf 'jev-observer installer: %s\n' "$1" >&2
    printf 'Build instructions: %s\n' "$build_docs" >&2
    exit 1
}

cleanup() {
    cleanup_status=$?
    trap - 0 1 2 15
    if [ -n "$stage_dir" ]; then rm -rf "$stage_dir"; fi
    if [ -n "$work_dir" ]; then rm -rf "$work_dir"; fi
    exit "$cleanup_status"
}

valid_version() {
    printf '%s\n' "$1" | awk '
        /^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?(\+[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$/ { good = 1 }
        END { exit !(NR == 1 && good) }
    '
}

shell_quote() {
    # Append a sentinel so even a path ending in a newline remains intact.
    printf '%s.' "$1" | awk '
        { path = path (NR > 1 ? "\n" : "") $0 }
        END {
            printf "%c", 39
            for (i = 1; i < length(path); i++) {
                character = substr(path, i, 1)
                if (character == sprintf("%c", 39)) printf "%c%c%c%c", 39, 92, 39, 39
                else printf "%s", character
            }
            printf "%c", 39
        }
    '
}

allowed_url() {
    case $1 in
        https://*) ;;
        http://*) [ "$allow_http" = 1 ] || return 1 ;;
        *) return 1 ;;
    esac
    url_authority=${1#*://}
    url_authority=${url_authority%%/*}
    case $url_authority in ''|*@*) return 1 ;; esac
    # URLs containing whitespace are not valid download destinations.
    printf '%s\n' "$1" | awk 'NR != 1 || /[[:space:]]/ { bad = 1 } END { exit bad }'
}

# Wget's --https-only applies to recursive links, not redirects. Follow each
# redirect explicitly so an HTTPS download cannot silently switch to HTTP.
wget_download() {
    fetch_url=$1
    fetch_output=$2
    redirects=0
    while [ "$redirects" -le 10 ]; do
        allowed_url "$fetch_url" || return 1
        if wget --no-config --no-netrc --server-response --max-redirect=0 \
            --timeout=30 --tries=2 --output-document="$fetch_output" \
            "$fetch_url" >"$work_dir/download.log" 2>&1; then
            return 0
        fi
        redirect=$(awk '
            /^[[:space:]]*HTTP\/[0-9.]+[[:space:]]+[0-9][0-9][0-9]/ {
                status = $2; location = ""
            }
            tolower($1) == "location:" {
                line = $0
                sub(/^[[:space:]]*[^:]+:[[:space:]]*/, "", line)
                sub(/[[:space:]]+\[following\]$/, "", line)
                sub(/[[:space:]]+$/, "", line)
                location = line
            }
            END { if (status ~ /^(301|302|303|307|308)$/) print location }
        ' "$work_dir/download.log")
        [ -n "$redirect" ] || return 1
        origin=${fetch_url#*://}
        origin=${origin%%/*}
        scheme=${fetch_url%%:*}
        case $redirect in
            https://*|http://*) fetch_url=$redirect ;;
            //*) fetch_url=$scheme:$redirect ;;
            /*) fetch_url=$scheme://$origin$redirect ;;
            \?*) fetch_url=${fetch_url%%\?*}$redirect ;;
            *:*) return 1 ;;
            *)
                fetch_url=${fetch_url%%\?*}
                fetch_url=${fetch_url%/*}/$redirect
                ;;
        esac
        redirects=$((redirects + 1))
    done
    return 1
}

download_asset() {
    download_tag=$1
    download_name=$2
    if [ "$downloader" = gh ]; then
        if [ "$download_tag" = latest ]; then
            gh release download --repo github.com/LimePencil/jev-observer \
                --pattern "$download_name" --dir "$work_dir" \
                >"$work_dir/download.log" 2>&1 && return 0
        else
            gh release download "$download_tag" --repo github.com/LimePencil/jev-observer \
                --pattern "$download_name" --dir "$work_dir" \
                >"$work_dir/download.log" 2>&1 && return 0
        fi
    else
        if [ "$download_tag" = latest ]; then
            download_url=$release_base/latest/download/$download_name
        else
            download_url=$release_base/download/$download_tag/$download_name
        fi
        if [ "$downloader" = curl ]; then
            curl -q --fail --silent --show-error --location \
                --proto "$protocols" --proto-redir "$protocols" \
                --connect-timeout 20 --max-time 300 --retry 2 \
                --output "$work_dir/$download_name" "$download_url" \
                >"$work_dir/download.log" 2>&1 && return 0
        else
            wget_download "$download_url" "$work_dir/$download_name" && return 0
        fi
    fi
    # Do not echo downloader logs: authenticated URLs can contain credentials.
    if [ "$release_base" = "$official_base" ]; then
        fail "Could not download $download_name from release $download_tag. The release or platform asset may not be published. Private repository access requires GitHub CLI: run gh auth login, then rerun this installer with sh."
    fi
    fail "Could not download $download_name from release $download_tag. Check the mirror and whether this release or platform asset is published."
}

main() {
    set -eu
    LC_ALL=C
    export LC_ALL
    # User tar defaults must not alter the archive validation or extraction.
    unset TAR_OPTIONS
    umask 077
    build_docs=https://github.com/LimePencil/jev-observer/blob/main/README.md
    official_base=https://github.com/LimePencil/jev-observer/releases
    requested_version=
    version_given=0
    install_dir=
    directory_given=0
    work_dir=
    stage_dir=
    trap cleanup 0
    trap 'exit 129' 1
    trap 'exit 130' 2
    trap 'exit 143' 15

    while [ "$#" -gt 0 ]; do
        case $1 in
            --help|-h) usage; return 0 ;;
            --version)
                [ "$#" -ge 2 ] || fail '--version requires a value'
                requested_version=$2; version_given=1; shift 2 ;;
            --version=*) requested_version=${1#*=}; version_given=1; shift ;;
            --install-dir)
                [ "$#" -ge 2 ] || fail '--install-dir requires a path'
                install_dir=$2; directory_given=1; shift 2 ;;
            --install-dir=*) install_dir=${1#*=}; directory_given=1; shift ;;
            *) fail "Unknown argument: $1. Use --help for usage." ;;
        esac
    done
    for utility in uname awk tar mktemp mkdir chmod mv rm cmp wc cat; do
        command -v "$utility" >/dev/null 2>&1 || fail "Required command is missing: $utility"
    done
    if [ "$version_given" = 1 ]; then
        requested_version=${requested_version#v}
        valid_version "$requested_version" || fail 'Version must be MAJOR.MINOR.PATCH, optionally with a prerelease/build suffix and leading v.'
    fi
    if [ "$directory_given" = 0 ]; then
        [ -n "${HOME:-}" ] || fail 'HOME is unset; provide --install-dir PATH'
        install_dir=$HOME/.local/bin
    fi
    [ -n "$install_dir" ] || fail '--install-dir must not be empty'
    case $install_dir in /*) ;; *) install_dir=$(pwd -P)/$install_dir ;; esac
    destination=$install_dir/jev-observer
    [ ! -d "$destination" ] || fail 'The install target jev-observer is a directory (or a symlink to one). Choose another --install-dir.'

    os=$(uname -s) || fail 'Could not identify the operating system'
    arch=$(uname -m) || fail 'Could not identify the CPU architecture'
    case $os:$arch in
        Linux:x86_64|Linux:amd64) target=x86_64-unknown-linux-musl ;;
        Linux:aarch64|Linux:arm64) target=aarch64-unknown-linux-musl ;;
        Darwin:x86_64|Darwin:amd64) target=x86_64-apple-darwin ;;
        Darwin:arm64|Darwin:aarch64) target=aarch64-apple-darwin ;;
        *) fail "No prebuilt release supports this platform: $os $arch. Build from source instead." ;;
    esac
    if command -v sha256sum >/dev/null 2>&1; then
        checksum_tool=sha256sum
    elif command -v shasum >/dev/null 2>&1; then
        checksum_tool=shasum
    else
        fail 'SHA-256 verification requires sha256sum or shasum; installation cannot continue without it.'
    fi

    allow_http=${JEV_OBSERVER_ALLOW_INSECURE_HTTP:-0}
    release_base=${JEV_OBSERVER_RELEASE_BASE_URL:-$official_base}
    while [ "${release_base%/}" != "$release_base" ]; do release_base=${release_base%/}; done
    case $release_base in *\?*|*\#*) fail 'The release base URL must not contain a query or fragment.' ;; esac
    allowed_url "$release_base" || fail 'The release base URL must be HTTPS without credentials. HTTP requires explicit JEV_OBSERVER_ALLOW_INSECURE_HTTP=1 for fixture tests.'
    protocols='=https'
    if [ "$allow_http" = 1 ]; then protocols='=https,http'; fi
    if [ "$release_base" = "$official_base" ] \
        && command -v gh >/dev/null 2>&1 \
        && gh auth status --active --hostname github.com >/dev/null 2>&1; then
        downloader=gh
    elif command -v curl >/dev/null 2>&1; then
        downloader=curl
    elif command -v wget >/dev/null 2>&1; then
        downloader=wget
    else
        fail 'Install curl, GNU wget, or an authenticated GitHub CLI to download a release.'
    fi

    temp_root=$(CDPATH='' cd -P "${TMPDIR:-/tmp}" && pwd -P) || fail 'The temporary directory is unavailable'
    work_dir=$(mktemp -d "$temp_root/jev-observer.XXXXXXXX") || fail 'Could not create a private temporary directory'
    tag=latest
    if [ "$version_given" = 1 ]; then tag=v$requested_version; fi
    printf 'Downloading Jev Observer (%s, %s)…\n' "$tag" "$target"
    download_asset "$tag" SHA256SUMS
    manifest=$work_dir/SHA256SUMS
    [ -f "$manifest" ] && [ ! -L "$manifest" ] || fail 'The checksum manifest was not downloaded as a regular file'
    manifest_bytes=$(wc -c <"$manifest")
    [ "$manifest_bytes" -le 1048576 ] || fail 'The checksum manifest is unexpectedly large'
    if ! awk -v target="$target" '
        BEGIN { prefix = "jev-observer-v"; suffix = "-" target ".tar.gz" }
        {
            name = $2
            sub(/^\*/, "", name)
            if (substr(name, 1, length(prefix)) == prefix &&
                substr(name, length(name) - length(suffix) + 1) == suffix) {
                matches++
                if (NF != 2 || length($1) != 64 || $1 !~ /^[0-9A-Fa-f]+$/) bad = 1
                print tolower($1), name
            }
        }
        END { exit !(matches == 1 && !bad) }
    ' "$manifest" >"$work_dir/selected"; then
        fail "SHA256SUMS has no single valid archive for $target. A prebuilt asset for this platform may not be published."
    fi
    IFS=' ' read -r expected_hash asset <"$work_dir/selected"
    version=${asset#jev-observer-v}
    version=${version%-"$target".tar.gz}
    valid_version "$version" || fail 'The checksum manifest contains an invalid release version'
    if [ "$version_given" = 1 ] && [ "$version" != "$requested_version" ]; then
        fail 'The checksum manifest does not match the requested release version'
    fi
    # Pin the archive even when SHA256SUMS came from latest, avoiding a release race.
    download_asset "v$version" "$asset"
    archive=$work_dir/$asset
    [ -f "$archive" ] && [ ! -L "$archive" ] || fail 'The release archive was not downloaded as a regular file'
    if [ "$checksum_tool" = sha256sum ]; then
        sha256sum <"$archive" >"$work_dir/computed" || fail 'Could not calculate the archive checksum'
    else
        shasum -a 256 <"$archive" >"$work_dir/computed" || fail 'Could not calculate the archive checksum'
    fi
    actual_hash=$(awk 'NR == 1 { print tolower($1) }' "$work_dir/computed")
    [ "$actual_hash" = "$expected_hash" ] || fail 'SHA-256 checksum mismatch; the existing installation has not been changed'

    tar -tzf "$archive" >"$work_dir/members" 2>"$work_dir/tar.log" || fail 'The release archive is invalid'
    printf 'jev-observer\n' >"$work_dir/expected-members"
    cmp -s "$work_dir/members" "$work_dir/expected-members" || fail 'The archive must contain exactly one root file named jev-observer'
    tar -tvzf "$archive" >"$work_dir/member-details" 2>"$work_dir/tar.log" || fail 'Could not inspect the release archive'
    if ! awk '
        NR != 1 { bad = 1 }
        NR == 1 {
            if ($0 !~ /^-[r-][w-]x[r-][w-][x-][r-][w-][x-][[:space:]]/ ||
                $NF != "jev-observer" || $0 ~ / link to | -> /) bad = 1
        }
        END { exit !(NR == 1 && !bad) }
    ' "$work_dir/member-details"; then
        fail 'The archived jev-observer must be a regular executable, not a link or directory'
    fi

    mkdir -p "$install_dir" || fail 'Could not create the install directory; choose a writable --install-dir'
    install_dir=$(CDPATH='' cd -P "$install_dir" && pwd -P) || fail 'Could not access the install directory'
    destination=$install_dir/jev-observer
    stage_dir=$(mktemp -d "$install_dir/.jev-observer-install.XXXXXXXX") || fail 'Could not stage a binary in the install directory; choose a writable --install-dir'
    # Write only the validated member's bytes, ignoring archive ownership,
    # paths and extended attributes. Staging here also works with a noexec /tmp.
    tar -xOzf "$archive" jev-observer >"$stage_dir/jev-observer" 2>"$work_dir/tar.log" || fail 'Could not extract the verified executable'
    [ -s "$stage_dir/jev-observer" ] || fail 'The verified executable is empty'
    chmod 755 "$stage_dir/jev-observer" || fail 'Could not make the verified binary executable'
    if ! "$stage_dir/jev-observer" --version </dev/null >"$work_dir/version" 2>"$work_dir/version-error"; then
        fail 'The verified binary could not run on this machine; the existing installation has not been changed'
    fi
    printf 'jev-observer %s\n' "$version" >"$work_dir/expected-version"
    cmp -s "$work_dir/version" "$work_dir/expected-version" || fail 'The binary version does not match the release; the existing installation has not been changed'
    [ ! -d "$destination" ] || fail 'The install target became a directory; the existing installation has not been changed'
    # The staging file shares the destination filesystem: replacement is atomic.
    mv -f "$stage_dir/jev-observer" "$destination" || fail 'Could not replace the installed binary'
    printf 'Installed jev-observer %s to %s\n' "$version" "$destination"
    case :${PATH:-}: in
        *:"$install_dir":*) ;;
        *)
            # Print the caller's literal $PATH for the command they will run.
            # shellcheck disable=SC2016
            printf 'For this shell, add the install directory to PATH:\n  export PATH=%s:"$PATH"\n' "$(shell_quote "$install_dir")"
            ;;
    esac
    printf 'Explore the local sample dashboard:\n  %s --demo\n' "$(shell_quote "$destination")"
}

main "$@"
