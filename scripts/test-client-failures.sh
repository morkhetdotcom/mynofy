#!/usr/bin/env bash
# Deterministic real client/transport failures; no LLM or live demo required.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
exec cargo test --locked -p mynofy-cli --test failure_clients --test mcp -- "$@"
