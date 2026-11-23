#!/usr/bin/env bash
# Run real service tests under the normal service/client UID.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
if [ "$(id -u)" = 0 ]; then
    echo "Run SDK integration tests as a normal user; the CLI refuses root without an explicit target." >&2
    exit 1
fi
cargo build --locked -p mynofy-cli
exec "${MYNOFY_MVN:-mvn}" -B -ntp -f sdks/java/pom.xml test "$@"
