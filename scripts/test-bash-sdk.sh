#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
if [ "$(id -u)" = 0 ]; then
    echo 'Run SDK integration tests as a normal user.' >&2
    exit 1
fi
cargo build --locked -p mynofy-cli
cargo test --locked -p mynofy-protocol --features crypto sdk_conformance
exec "${MYNOFY_PYTHON:-python3}" -m unittest discover -s sdks/bash/tests -v "$@"
