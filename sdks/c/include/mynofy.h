#ifndef MYNOFY_H
#define MYNOFY_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

#define MYNOFY_ABI_VERSION 2
#define MYNOFY_MAX_BYTES 4096

typedef struct mynofy_key mynofy_key;
typedef struct mynofy_client mynofy_client;
typedef struct mynofy_response mynofy_response;

/* Borrowed views; strings are UTF-8, NOT NUL terminated. Empty views have len=0.
 * Response views live until response_free; key views until key_free; info views
 * until client_free. Do not modify or free view data. */
typedef struct { const char *data; size_t len; } mynofy_str;
typedef struct { const uint8_t *data; size_t len; } mynofy_bytes;

enum mynofy_status {
    MYNOFY_OK=0, MYNOFY_INVALID=1, MYNOFY_BUSY=2, MYNOFY_CLOSED=3,
    MYNOFY_TRANSPORT=4, MYNOFY_BROKER=5, MYNOFY_INTERNAL=6,
    MYNOFY_CANCELLED=7, MYNOFY_TIMEOUT=8
};
typedef struct {
    int32_t code;
    uint32_t outcome_unknown;
    uint32_t has_retry_after_ms;
    uint64_t retry_after_ms;
    char broker_code[64];
    char message[256];
} mynofy_error;

typedef struct {
    size_t struct_size;
    uint32_t abi_version;
    uint32_t timeout_ms; /* 0 = 10000; whole connect/auth or operation budget. */
    const char *url; /* wss:// only; exactly one of url/socket_path. */
    const char *socket_path;
    const char *ca_file; /* Required for WSS. */
    const char *owner;
    const char *client_id; /* NULL = identity from first public key. */
    const mynofy_key *key;
    const char *via_topic; /* Optional owner tunnel. */
    const char *owner_ca_file;
    const char *owner_name; /* NULL = owner. */
} mynofy_config;
#define MYNOFY_CONFIG_INIT {sizeof(mynofy_config),MYNOFY_ABI_VERSION,0,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL}

typedef struct { mynofy_str owner, client_id; uint32_t restart_counter; } mynofy_connection_info;
enum mynofy_parameter_type {
    MYNOFY_U32=1, MYNOFY_U64=2, MYNOFY_I32=3, MYNOFY_I64=4,
    MYNOFY_F32=5, MYNOFY_F64=6, MYNOFY_BOOL=7
};
typedef union {
    uint32_t u32_value; uint64_t u64_value;
    int32_t i32_value; int64_t i64_value;
    float f32_value; double f64_value; uint32_t bool_value; /* exactly 0/1 */
} mynofy_parameter_value;
typedef struct { uint32_t type; mynofy_parameter_value value; } mynofy_parameter;
/* Initialize the entire event with {0}, then set IDs and selected union members. */
typedef struct {
    uint32_t catalog, catalog_version, category, action, status, parameter_count;
    mynofy_parameter parameters[3];
} mynofy_event;

enum mynofy_payload_kind { MYNOFY_NO_PAYLOAD=0, MYNOFY_BYTES=1, MYNOFY_EVENT=2 };
enum mynofy_response_kind {
    MYNOFY_RESULT=1, MYNOFY_MESSAGES=2, MYNOFY_MESSAGES_PAGE=3,
    MYNOFY_MESSAGE_STATUS=4, MYNOFY_PERMISSIONS=5, MYNOFY_CATALOG=6,
    MYNOFY_ROTATION_COMPLETE=7
};
typedef struct {
    mynofy_str id, topic, expires, created_at;
    uint32_t payload_kind;
    mynofy_bytes data;
    mynofy_str content_type;
    mynofy_event event;
} mynofy_message;
typedef struct {
    mynofy_str rotation_id, client_id, token_id, valid_until;
} mynofy_rotation_info;

/* Inputs: valid live handles, UTF-8 NUL-terminated strings, and readable buffers
 * of the given lengths. NULL optional strings mean absent. NULL data is allowed
 * only with len=0. Outputs must be writable; handle outputs must initially be NULL.
 * Outputs must not overlap inputs, views or handles.
 * Error output is optional. Errors are caller-owned and always initialized.
 * No function unwinds a Rust panic through C. Invalid non-NULL pointers or
 * use-after-free cannot be detected and violate the API contract. */
int32_t mynofy_key_generate(mynofy_key **out, mynofy_error *error);
int32_t mynofy_key_parse(const uint8_t *der, size_t len, mynofy_key **out, mynofy_error *error);
mynofy_bytes mynofy_key_pkcs8(const mynofy_key *key);
mynofy_bytes mynofy_key_public(const mynofy_key *key);
mynofy_str mynofy_key_token_id(const mynofy_key *key);
mynofy_str mynofy_key_client_id(const mynofy_key *key);
void mynofy_key_free(mynofy_key *key);

int32_t mynofy_connect(const mynofy_config *config, mynofy_client **out, mynofy_error *error);
int32_t mynofy_client_info(const mynofy_client *client, mynofy_connection_info *out, mynofy_error *error);
/* close may run concurrently with an operation: permanent cancellation, no retry.
 * free requires all calls on that handle to have returned. It also closes it. */
void mynofy_client_close(const mynofy_client *client);
void mynofy_client_free(mynofy_client *client);

/* One operation per client: concurrent attempts return BUSY without queuing.
 * Transport/protocol failure, timeout or cancellation closes the connection.
 * outcome_unknown can be set for publish/rotation: reconcile before retry.
 * The application retains cursor/business state and decides when to advance its cursor. */
int32_t mynofy_publish_bytes(const mynofy_client *, const char *topic,
    const uint8_t *data, size_t len, const char *content_type, const char *expires,
    mynofy_response **out, mynofy_error *error);
int32_t mynofy_publish_event(const mynofy_client *, const char *topic,
    const mynofy_event *event, const char *expires, mynofy_response **out, mynofy_error *error);
int32_t mynofy_poll(const mynofy_client *, const char *topic, const char *after,
    uint32_t limit, mynofy_response **out, mynofy_error *error);
int32_t mynofy_list_messages(const mynofy_client *, const char *topic, const char *cursor,
    uint32_t limit, mynofy_response **out, mynofy_error *error);
int32_t mynofy_get_message(const mynofy_client *, const char *id, mynofy_response **out, mynofy_error *error);
int32_t mynofy_get_permissions(const mynofy_client *, const char *topic, mynofy_response **out, mynofy_error *error);
int32_t mynofy_get_catalog(const mynofy_client *, const char *name, uint32_t version,
    mynofy_response **out, mynofy_error *error);
/* Persist new_key BEFORE calling. Success closes the old session. After an
 * uncertain commit retain both keys. Reconnect with the original stable ID. */
int32_t mynofy_rotate(const mynofy_client *, const mynofy_key *new_key, mynofy_response **out, mynofy_error *error);

void mynofy_response_free(mynofy_response *response);
uint32_t mynofy_response_type(const mynofy_response *response);
size_t mynofy_response_message_count(const mynofy_response *response);
int32_t mynofy_response_message_at(const mynofy_response *, size_t index, mynofy_message *out, mynofy_error *error);
/* Poll/get_message only; empty view for summaries, invalid index or NULL response.
 * Owner-stamped stable identity; borrowed until response_free. */
mynofy_str mynofy_response_message_producer_id(const mynofy_response *, size_t index);
mynofy_str mynofy_response_message_id(const mynofy_response *);
mynofy_str mynofy_response_expires(const mynofy_response *);
mynofy_str mynofy_response_next_cursor(const mynofy_response *);
uint32_t mynofy_response_has_more(const mynofy_response *);
mynofy_bytes mynofy_response_catalog(const mynofy_response *);
size_t mynofy_response_permission_count(const mynofy_response *);
mynofy_str mynofy_response_permission_at(const mynofy_response *, size_t index);
mynofy_str mynofy_response_owner(const mynofy_response *);
mynofy_str mynofy_response_acl_revision(const mynofy_response *);
int32_t mynofy_response_rotation(const mynofy_response *, mynofy_rotation_info *out, mynofy_error *error);
#ifdef __cplusplus
}
#endif
#endif
