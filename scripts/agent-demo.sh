#!/usr/bin/env bash
# Local operator + two workers demo. No Python packages or TLS setup required.
set -euo pipefail
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
if [ "${1:-}" = setup ]; then
    (cd "$project_dir" && cargo build --locked -p mynofy-cli)
fi
exec python3 "$project_dir/examples/agent-demo/demo.py" "$@"
