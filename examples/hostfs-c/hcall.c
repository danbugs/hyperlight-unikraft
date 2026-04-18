#include "hcall.h"

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int hcall_exchange(const char *request, char *out, size_t out_cap)
{
    int fd = open("/dev/hcall", O_RDWR);
    if (fd < 0) {
        fprintf(stderr, "open(/dev/hcall): %s\n", strerror(errno));
        return -1;
    }
    size_t req_len = strlen(request);
    ssize_t wrote = write(fd, request, req_len);
    if (wrote < 0 || (size_t)wrote != req_len) {
        fprintf(stderr, "write(/dev/hcall): %s\n", strerror(errno));
        close(fd);
        return -1;
    }
    ssize_t n = read(fd, out, out_cap - 1);
    close(fd);
    if (n < 0) {
        fprintf(stderr, "read(/dev/hcall): %s\n", strerror(errno));
        return -1;
    }
    out[n] = '\0';
    return (int)n;
}

int json_encode_string(const char *in, char *out, size_t cap)
{
    size_t p = 0;
    if (p >= cap) return -1;
    out[p++] = '"';
    for (const char *s = in; *s; s++) {
        const char *esc = NULL;
        char buf[3];
        switch (*s) {
            case '"':  esc = "\\\""; break;
            case '\\': esc = "\\\\"; break;
            case '\n': esc = "\\n";  break;
            case '\r': esc = "\\r";  break;
            case '\t': esc = "\\t";  break;
            default:
                if ((unsigned char)*s < 0x20) {
                    // Control chars — skip for MVP.
                    continue;
                }
                buf[0] = *s; buf[1] = '\0';
                esc = buf;
                break;
        }
        size_t n = strlen(esc);
        if (p + n >= cap) return -1;
        memcpy(out + p, esc, n);
        p += n;
    }
    if (p + 2 > cap) return -1;
    out[p++] = '"';
    out[p] = '\0';
    return (int)p;
}

// Locate a quoted string value following "key" in a JSON object.
// Returns pointer to the opening `"` of the value, or NULL if not found.
static const char *find_key_value_quote(const char *json, const char *key)
{
    char needle[128];
    int n = snprintf(needle, sizeof(needle), "\"%s\"", key);
    if (n < 0 || (size_t)n >= sizeof(needle)) return NULL;
    const char *p = strstr(json, needle);
    if (!p) return NULL;
    p += n;
    while (*p && (*p == ' ' || *p == '\t' || *p == ':')) p++;
    return (*p == '"') ? p : NULL;
}

int json_find_string(const char *json, const char *key, char *out, size_t out_cap)
{
    const char *q = find_key_value_quote(json, key);
    if (!q) return -1;
    q++; // past opening quote
    size_t p = 0;
    while (*q && *q != '"') {
        char c;
        if (*q == '\\' && q[1]) {
            switch (q[1]) {
                case 'n': c = '\n'; break;
                case 'r': c = '\r'; break;
                case 't': c = '\t'; break;
                case '"': c = '"';  break;
                case '\\': c = '\\'; break;
                default: c = q[1]; break;
            }
            q += 2;
        } else {
            c = *q++;
        }
        if (p + 1 >= out_cap) return -1;
        out[p++] = c;
    }
    out[p] = '\0';
    return (int)p;
}

int json_find_int(const char *json, const char *key, long long *out)
{
    char needle[128];
    int n = snprintf(needle, sizeof(needle), "\"%s\"", key);
    if (n < 0 || (size_t)n >= sizeof(needle)) return -1;
    const char *p = strstr(json, needle);
    if (!p) return -1;
    p += n;
    while (*p && (*p == ' ' || *p == '\t' || *p == ':')) p++;
    char *end;
    long long v = strtoll(p, &end, 10);
    if (end == p) return -1;
    *out = v;
    return 0;
}

int hcall_has_error(const char *json)
{
    return strstr(json, "\"error\"") != NULL;
}
