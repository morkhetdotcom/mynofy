#!/usr/bin/env bash
# Extended reproducible functional/model/mutation tests; no benchmark or new dependency.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
export MYNOFY_MODEL_SEEDS="${MYNOFY_MODEL_SEEDS:-64}"
export MYNOFY_MODEL_STEPS="${MYNOFY_MODEL_STEPS:-1000}"
export MYNOFY_FUZZ_CASES="${MYNOFY_FUZZ_CASES:-16384}"
exec cargo test --locked -p mynofy-server functional -- --nocapture "$@"
