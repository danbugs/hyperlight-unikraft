// Tiny /dev/hcall helper — open the device, exchange a JSON request/response.
//
// Phase A of the filesystem sandbox: the guest calls the host by name
// (fs_read, fs_write, ...) with a JSON payload. Phase B will replace this
// with a transparent Unikraft VFS driver that routes POSIX ops through the
// same host handlers.
//
// Only handles ASCII-safe text (quotes/backslashes are escaped on write,
// \n/\t/\"/\\ are decoded on read). Production use would want full JSON
// escaping + unicode support.

#ifndef HCALL_H
#define HCALL_H

#include <stddef.h>

// Build a JSON request and exchange it with the host.
// `out` / `out_cap` is a caller-provided buffer the response is written to.
// Returns the number of bytes written to `out` (excluding null terminator),
// or -1 on error. `out` is always null-terminated on success.
int hcall_exchange(const char *request, char *out, size_t out_cap);

// Escape a plain string into a JSON string value (adds surrounding quotes).
// Writes up to `cap - 1` bytes + null terminator. Returns the number of
// bytes written, or -1 if the buffer is too small.
int json_encode_string(const char *in, char *out, size_t cap);

// Extract a string value for `key` from a JSON object. Writes up to
// `out_cap - 1` bytes + null terminator. Unescapes \n, \t, \", \\.
// Returns the number of bytes written, or -1 if the key is missing.
int json_find_string(const char *json, const char *key, char *out, size_t out_cap);

// Extract an integer value for `key`. Returns 0 on success, -1 if missing.
int json_find_int(const char *json, const char *key, long long *out);

// Returns 1 if the response has an "error" field, else 0.
int hcall_has_error(const char *json);

#endif // HCALL_H
