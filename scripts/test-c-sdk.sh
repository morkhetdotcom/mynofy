#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
if [ "$(id -u)" = 0 ]; then
    echo 'Run SDK service integration tests as a normal user.' >&2
    exit 1
fi
cargo build --locked -p mynofy-cli -p mynofy-client-c
cargo test --locked -p mynofy-client-c
cargo test --locked -p mynofy-protocol --features crypto sdk_conformance
mkdir -p target/c-sdk-tests
case "$(uname -s)" in
    Darwin) library_path=DYLD_LIBRARY_PATH ;;
    *) library_path=LD_LIBRARY_PATH ;;
esac
"${CC:-cc}" -std=c11 -Wall -Wextra -Werror -pedantic -I sdks/c/include \
    sdks/c/tests/native.c -L target/debug -lmynofy_c -o target/c-sdk-tests/native
"${CC:-cc}" -std=c11 -Wall -Wextra -Werror -pedantic -I sdks/c/include \
    sdks/c/examples/local.c -L target/debug -lmynofy_c -o target/c-sdk-tests/local
# Preserve the Python runtime and any caller-supplied native library paths.
existing_library_path=${!library_path:-}
exec env "$library_path=$PWD/target/debug${existing_library_path:+:$existing_library_path}" "${MYNOFY_PYTHON:-python3}" -m unittest discover -s sdks/c/tests -v "$@"
