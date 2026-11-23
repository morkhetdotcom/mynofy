#!/usr/bin/env bash
# Run from a venv containing sdks/python, under the normal service/client UID.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
if [ "$(id -u)" = 0 ]; then
    echo "Run SDK integration tests as a normal user; the CLI refuses root without an explicit target." >&2
    exit 1
fi
cargo build --locked -p mynofy-cli
exec "${MYNOFY_PYTHON:-python3}" -m unittest discover -s sdks/python/tests -v "$@"
