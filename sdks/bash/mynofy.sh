# mynofy Bash SDK (Bash >= 3.2). Source this file; caller options/traps stay intact.
# All network, authentication, crypto and Protobuf work belongs to the Rust CLI.

_mynofy_usage() { printf 'mynofy: %s\n' "$*" >&2; return 64; }
_mynofy_quote() { jq -cn --arg value "$1" '$value'; }
_mynofy_name() {
    local LC_ALL=C
    [[ -n $1 && ${#1} -le 256 ]] || _mynofy_usage 'name must contain 1..256 UTF-8 bytes'
}
_mynofy_uint() {
    local LC_ALL=C value=$1 maximum=$2 pattern='^(0|[1-9][0-9]*)$'
    [[ $value =~ $pattern ]] || return 1
    [[ ${#value} -lt ${#maximum} || ( ${#value} -eq ${#maximum} && ( $value == "$maximum" || $value < $maximum ) ) ]]
}
_mynofy_int() {
    if [[ $1 == -* ]]; then
        [[ $1 != -0 ]] && _mynofy_uint "${1#-}" "$3"
    else
        _mynofy_uint "$1" "$2"
    fi
}
_mynofy_dependencies() {
    command -v "${MYNOFY_BIN:-mynofy}" >/dev/null 2>&1 || { _mynofy_usage 'mynofy CLI not found (MYNOFY_BIN)'; return 64; }
    command -v jq >/dev/null 2>&1 || { _mynofy_usage 'jq is required'; return 64; }
}
# Appends to the caller's local array, using Bash dynamic scoping (no eval/nameref).
_mynofy_config() {
    _mynofy_dependencies || return
    [[ -n ${MYNOFY_OWNER:-} && -n ${MYNOFY_KEY:-} ]] || { _mynofy_usage 'set MYNOFY_OWNER and MYNOFY_KEY'; return 64; }
    if [[ -n ${MYNOFY_SOCKET:-} && -z ${MYNOFY_URL:-} ]]; then
        _mynofy_args+=(--socket "$MYNOFY_SOCKET")
        [[ -z ${MYNOFY_CA:-} ]] || { _mynofy_usage 'MYNOFY_CA is only for WSS'; return 64; }
    elif [[ -n ${MYNOFY_URL:-} && -z ${MYNOFY_SOCKET:-} ]]; then
        [[ $MYNOFY_URL == wss://* && -n ${MYNOFY_CA:-} ]] || { _mynofy_usage 'WSS needs wss:// and MYNOFY_CA'; return 64; }
        _mynofy_args+=(--url "$MYNOFY_URL" --ca "$MYNOFY_CA")
    else
        _mynofy_usage 'set exactly one of MYNOFY_SOCKET and MYNOFY_URL'; return 64
    fi
    _mynofy_args+=(--owner "$MYNOFY_OWNER" --key "$MYNOFY_KEY")
    [[ -z ${MYNOFY_CLIENT_ID:-} ]] || _mynofy_args+=(--client-id "$MYNOFY_CLIENT_ID")
    if [[ -n ${MYNOFY_VIA_TOPIC:-} ]]; then
        [[ -n ${MYNOFY_OWNER_CA:-} ]] || { _mynofy_usage 'owner tunnel needs MYNOFY_OWNER_CA'; return 64; }
        _mynofy_args+=(--via-topic "$MYNOFY_VIA_TOPIC" --owner-ca "$MYNOFY_OWNER_CA")
        [[ -z ${MYNOFY_OWNER_NAME:-} ]] || _mynofy_args+=(--owner-name "$MYNOFY_OWNER_NAME")
    elif [[ -n ${MYNOFY_OWNER_CA:-}${MYNOFY_OWNER_NAME:-} ]]; then
        _mynofy_usage 'owner CA/name require MYNOFY_VIA_TOPIC'; return 64
    fi
    return 0
}
# Cleanup belongs only to the operation subshell, never to the sourcing shell.
_mynofy_cleanup() {
    local status=$1
    trap - EXIT HUP INT TERM
    if [[ -n ${_mynofy_pid:-} ]]; then
        kill -TERM "$_mynofy_pid" 2>/dev/null || :
        wait "$_mynofy_pid" 2>/dev/null || :
    fi
    [[ -z ${_mynofy_tmp:-} ]] || rm -rf -- "$_mynofy_tmp"
    return "$status"
}
_mynofy_signal() {
    printf 'mynofy: operation interrupted; mutation outcome may be unknown\n' >&2
    exit "$1"
}
_mynofy_execute() (
    set +u # Bash 3.2 treats an empty array as unset under nounset; scoped to this subshell.
    # mode, response request ID, expected response kind, request JSON, CLI extras
    local mode=$1 request_id=$2 expected=$3 request=$4
    shift 4
    # Bash 3.2 unwinds function locals before an EXIT trap triggered by a signal.
    # These values are globals only inside this isolated operation subshell.
    _mynofy_pid='' _mynofy_tmp=''
    local status
    local -a _mynofy_args=()
    _mynofy_dependencies || return
    if [[ $mode != keygen ]]; then _mynofy_config || return; fi
    umask 077
    _mynofy_tmp=$(mktemp -d "${TMPDIR:-/tmp}/mynofy-bash.XXXXXXXX") || return 20
    trap '_mynofy_cleanup "$?"' EXIT
    trap '_mynofy_signal 129' HUP
    trap '_mynofy_signal 130' INT
    trap '_mynofy_signal 143' TERM
    (
        # Explicit exec/reset also makes $! the CLI PID on older Bash versions.
        trap - EXIT HUP INT TERM
        exec "${MYNOFY_BIN:-mynofy}" "$mode" "${_mynofy_args[@]}" "$@"
    ) <<< "$request" > "$_mynofy_tmp/response" &
    _mynofy_pid=$!
    if wait "$_mynofy_pid"; then status=0; else status=$?; fi
    _mynofy_pid=''
    if [[ $status != 0 ]]; then
        printf 'mynofy: CLI failed; a mutation may already have been accepted; no retry\n' >&2
        return 20
    fi
    if [[ $mode == keygen ]]; then
        jq -es 'length == 1 and (.[0] | type == "object" and .algorithm == "ed25519" and (.client_id | type == "string") and (.public_key | type == "string"))' \
            "$_mynofy_tmp/response" >/dev/null || return 21
    else
        # jq never serializes this document: large integer lexemes remain untouched.
        jq -es --arg id "$request_id" --arg expected "$expected" \
            'length == 1 and (.[0] | type == "object" and .version == 1 and
             ((.request_id == $id) or
              (.type == "error" and .code == "rate_limited" and (has("request_id") | not) and
               (.retry_after_ms | type == "number" and . >= 0 and floor == .))) and
             (.type == $expected or .type == "error"))' \
            "$_mynofy_tmp/response" >/dev/null || { printf 'mynofy: invalid/mismatched response; mutation outcome may be unknown\n' >&2; return 21; }
    fi
    cat "$_mynofy_tmp/response" || return 20
    if [[ $mode != keygen ]] && jq -e '.type == "error"' "$_mynofy_tmp/response" >/dev/null; then return 10; fi
    return 0
)
_mynofy_request() {
    local kind=$1 expected=$2 fields=$3
    _mynofy_execute client bash-sdk "$expected" "{\"version\":1,\"type\":\"$kind\",\"id\":\"bash-sdk\"$fields}"
}
_mynofy_expires() {
    [[ -z $1 ]] || printf ',"expires":{"time":%s}' "$(_mynofy_quote "$1")"
    return 0
}
_mynofy_base64() (
    set -o pipefail
    local encoded
    # Bound the input before encoding; Bash variables never receive raw binary.
    encoded=$(head -c 4097 | base64 | tr -d '\r\n') || return 64
    if [[ ${#encoded} -gt 5464 || ( ${#encoded} -eq 5464 && $encoded != *== ) ]]; then
        _mynofy_usage 'payload exceeds 4096 bytes'; return 64
    fi
    printf '%s' "$encoded"
)
_mynofy_publish_data() {
    local topic=$1 encoded=$2 content_type=$3 expires=$4 fields
    _mynofy_name "$topic" || return
    fields=",\"topic\":$(_mynofy_quote "$topic"),\"payload\":{\"kind\":\"bytes\",\"data\":\"$encoded\""
    if [[ -n $content_type ]]; then fields+=",\"content_type\":$(_mynofy_quote "$content_type")"; fi
    fields+="}$(_mynofy_expires "$expires")"
    _mynofy_request publish result "$fields"
}
mynofy_publish_text() {
    [[ $# -ge 2 && $# -le 3 ]] || { _mynofy_usage 'publish_text TOPIC TEXT [EXPIRES]'; return 64; }
    _mynofy_dependencies || return
    local encoded
    encoded=$(printf '%s' "$2" | _mynofy_base64) || return
    _mynofy_publish_data "$1" "$encoded" 'text/plain; charset=utf-8' "${3:-}"
}
mynofy_publish_file() {
    [[ $# -ge 2 && $# -le 4 ]] || { _mynofy_usage 'publish_file TOPIC FILE [CONTENT_TYPE] [EXPIRES]'; return 64; }
    _mynofy_dependencies || return
    local encoded
    encoded=$(_mynofy_base64 < "$2") || return 64
    _mynofy_publish_data "$1" "$encoded" "${3:-}" "${4:-}"
}
mynofy_publish_event() {
    [[ $# -ge 4 ]] || { _mynofy_usage 'publish_event TOPIC CATEGORY ACTION STATUS [TYPE:VALUE ...] [--expires TIME]'; return 64; }
    _mynofy_dependencies || return
    local topic=$1 category=$2 action=$3 status=$4 parameters='' separator='' item type value count=0 expires='' fields
    local float_pattern='^-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?$'
    shift 4
    _mynofy_name "$topic" || return
    _mynofy_uint "$category" 65535 && _mynofy_uint "$action" 65535 && _mynofy_uint "$status" 65535 || { _mynofy_usage 'event IDs must be unsigned decimal integers'; return 64; }
    while [[ $# -gt 0 ]]; do
        if [[ $1 == --expires ]]; then
            [[ $# == 2 && -n $2 ]] || { _mynofy_usage '--expires must be last, followed by a time'; return 64; }
            expires=$2; shift 2; break
        fi
        item=$1; shift
        [[ $item == *:* && $count -lt 3 ]] || { _mynofy_usage 'at most three TYPE:VALUE parameters'; return 64; }
        type=${item%%:*}; value=${item#*:}
        case $type in
            u32) _mynofy_uint "$value" 4294967295 || { _mynofy_usage 'invalid u32'; return 64; } ;;
            u64) _mynofy_uint "$value" 18446744073709551615 || { _mynofy_usage 'invalid u64'; return 64; } ;;
            i32) _mynofy_int "$value" 2147483647 2147483648 || { _mynofy_usage 'invalid i32'; return 64; } ;;
            i64) _mynofy_int "$value" 9223372036854775807 9223372036854775808 || { _mynofy_usage 'invalid i64'; return 64; } ;;
            bool) [[ $value == true || $value == false ]] || { _mynofy_usage 'invalid bool'; return 64; } ;;
            f32|f64) [[ $value =~ $float_pattern ]] || { _mynofy_usage 'invalid float'; return 64; } ;;
            *) _mynofy_usage 'unknown parameter type'; return 64 ;;
        esac
        parameters+="$separator{\"$type\":$value}"; separator=','; count=$((count+1))
    done
    fields=",\"topic\":$(_mynofy_quote "$topic"),\"payload\":{\"kind\":\"event\",\"catalog\":1,\"catalog_version\":1,\"category\":$category,\"action\":$action,\"status\":$status,\"parameters\":[$parameters]}$(_mynofy_expires "$expires")"
    _mynofy_request publish result "$fields"
}
mynofy_poll() {
    [[ $# -ge 1 && $# -le 3 ]] || { _mynofy_usage 'poll TOPIC [AFTER] [LIMIT]'; return 64; }
    _mynofy_dependencies || return
    _mynofy_name "$1" || return
    local after=${2:-} limit=${3:-20} fields
    _mynofy_uint "$limit" 100 && [[ $limit != 0 ]] || { _mynofy_usage 'limit must be 1..100'; return 64; }
    [[ -z $after || $after =~ ^[0-9A-Za-z]{8}$ ]] || { _mynofy_usage 'invalid cursor'; return 64; }
    fields=",\"topic\":$(_mynofy_quote "$1"),\"limit\":$limit"
    [[ -z $after ]] || fields+=",\"after\":$(_mynofy_quote "$after")"
    _mynofy_request poll messages "$fields"
}


mynofy_list_messages() {
    [[ $# -le 3 ]] || { _mynofy_usage 'list_messages [TOPIC] [CURSOR] [LIMIT]'; return 64; }
    _mynofy_dependencies || return
    local topic=${1:-} cursor=${2:-} limit=${3:-20} fields
    _mynofy_uint "$limit" 100 && [[ $limit != 0 ]] || { _mynofy_usage 'limit must be 1..100'; return 64; }
    fields=",\"limit\":$limit"
    if [[ -n $topic ]]; then _mynofy_name "$topic" || return; fields+=",\"topic\":$(_mynofy_quote "$topic")"; fi
    [[ -z $cursor ]] || fields+=",\"cursor\":$(_mynofy_quote "$cursor")"
    _mynofy_request list_messages messages_page "$fields"
}
mynofy_get_message() {
    [[ $# == 1 ]] || { _mynofy_usage 'get_message MESSAGE_ID'; return 64; }
    _mynofy_dependencies || return
    _mynofy_name "$1" || return
    _mynofy_request get_message message_status ",\"message_id\":$(_mynofy_quote "$1")"
}
mynofy_get_permissions() {
    [[ $# == 1 ]] || { _mynofy_usage 'get_permissions TOPIC'; return 64; }
    _mynofy_dependencies || return
    _mynofy_name "$1" || return
    _mynofy_request get_permissions permissions ",\"topic\":$(_mynofy_quote "$1")"
}
mynofy_get_catalog() {
    [[ $# -le 2 ]] || { _mynofy_usage 'get_catalog [NAME] [VERSION]'; return 64; }
    _mynofy_dependencies || return
    local name=${1:-mynofy} version=${2:-1}
    _mynofy_name "$name" || return
    _mynofy_uint "$version" 4294967295 && [[ $version != 0 ]] || { _mynofy_usage 'invalid catalog version'; return 64; }
    _mynofy_request get_catalog catalog ",\"name\":$(_mynofy_quote "$name"),\"catalog_version\":$version"
}
mynofy_keygen() {
    [[ $# == 1 && -n $1 ]] || { _mynofy_usage 'keygen NEW_KEY_FILE'; return 64; }
    _mynofy_execute keygen '' '' '' --out "$1"
}
mynofy_rotate() {
    [[ $# == 1 && -n $1 ]] || { _mynofy_usage 'rotate NEW_KEY_FILE (must not exist)'; return 64; }
    _mynofy_execute rotate rotation-commit rotation_complete '' --new-key "$1"
}
mynofy_session() (
    [[ $# == 0 ]] || { _mynofy_usage 'session < requests.jsonl'; return 64; }
    local -a _mynofy_args=()
    _mynofy_config || return
    # Low-level JSONL passthrough: CLI exit status and broker error frames unchanged.
    exec "${MYNOFY_BIN:-mynofy}" client "${_mynofy_args[@]}"
)
