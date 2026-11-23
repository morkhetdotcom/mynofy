#!/usr/bin/env bash
# Real service integration tests require a normal service/client UID.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
if [ "$(id -u)" = 0 ]; then
    echo "Run SDK integration tests as a normal user; the CLI refuses root without an explicit target." >&2
    exit 1
fi
cargo build --locked -p mynofy-cli
cd sdks/go
exec "${MYNOFY_GO:-go}" test -race ./... "$@"
