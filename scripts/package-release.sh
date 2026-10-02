#!/usr/bin/env bash
# Compatibility entrypoint; the packaging implementation is cross-platform Python.
set -euo pipefail
exec python3 "$(dirname "$0")/package-release.py" "$@"
