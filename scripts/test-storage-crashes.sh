#!/usr/bin/env bash
# Real child-process SIGKILLs and deterministic ENOSPC/EIO in cfg(test)-only code.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
exec cargo test --locked -p mynofy-server storage_fault_ -- --nocapture "$@"
