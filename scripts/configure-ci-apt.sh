#!/usr/bin/env bash
set -euo pipefail

if [[ "${GITHUB_ACTIONS:-}" != true || "${RUNNER_OS:-}" != Linux ]]; then
  echo 'This script only configures disposable Linux GitHub Actions runners.' >&2
  exit 1
fi

# The hosted image's Azure HTTP mirror can stall even while the HTTPS fallback
# works. Keep dependency installation, using the official Ubuntu HTTPS mirror.
if [[ -f /etc/apt/apt-mirrors.txt ]]; then
  case "$(uname -m)" in
    x86_64) mirror=https://archive.ubuntu.com/ubuntu ;;
    aarch64) mirror=https://ports.ubuntu.com/ubuntu-ports ;;
    *) echo 'Unsupported native Linux runner architecture' >&2; exit 1 ;;
  esac
  printf '%s\n' "$mirror" | sudo tee /etc/apt/apt-mirrors.txt >/dev/null
fi
sudo tee /etc/apt/apt.conf.d/80-observer-ci-timeouts >/dev/null <<'APT'
Acquire::Retries "1";
Acquire::http::Timeout "30";
Acquire::https::Timeout "30";
APT
