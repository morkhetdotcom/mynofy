#!/usr/bin/env bash
# Protocol negotiation demo; shares transport/setup code with demo 1.
set -euo pipefail
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
export MYNOFY_DEMO_VARIANT=2
exec "$project_dir/scripts/agent-demo.sh" "$@"
