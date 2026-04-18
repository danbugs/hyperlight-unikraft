// Exercise the host-mediated filesystem sandbox via /dev/hcall.
//
// The host is started with `--mount <dir>`; every path we pass to
// fs_read / fs_write / fs_list / fs_stat is resolved relative to that
// directory. Escape attempts (`..`, absolute paths that jump out,
// symlinks pointing outside) are rejected by the host.

#include "hcall.h"

#include <stdio.h>
#include <string.h>

#define BUF 4096

static int call_fs_write(const char *path, const char *text, int append)
{
    char pbuf[256], tbuf[1024], req[2048], resp[BUF];
    if (json_encode_string(path, pbuf, sizeof pbuf) < 0) return -1;
    if (json_encode_string(text, tbuf, sizeof tbuf) < 0) return -1;
    int n = snprintf(req, sizeof req,
        "{\"name\":\"fs_write\",\"args\":{\"path\":%s,\"text\":%s,\"append\":%s}}",
        pbuf, tbuf, append ? "true" : "false");
    if (n < 0 || (size_t)n >= sizeof req) return -1;
    if (hcall_exchange(req, resp, sizeof resp) < 0) return -1;
    if (hcall_has_error(resp)) {
        char err[256];
        json_find_string(resp, "error", err, sizeof err);
        fprintf(stderr, "fs_write(%s): %s\n", path, err);
        return -1;
    }
    long long nb = -1;
    json_find_int(resp, "bytes_written", &nb);
    return (int)nb;
}

static int call_fs_read(const char *path, char *out, size_t cap)
{
    char pbuf[256], req[512], resp[BUF];
    if (json_encode_string(path, pbuf, sizeof pbuf) < 0) return -1;
    int n = snprintf(req, sizeof req,
        "{\"name\":\"fs_read\",\"args\":{\"path\":%s}}", pbuf);
    if (n < 0 || (size_t)n >= sizeof req) return -1;
    if (hcall_exchange(req, resp, sizeof resp) < 0) return -1;
    if (hcall_has_error(resp)) {
        char err[256];
        json_find_string(resp, "error", err, sizeof err);
        fprintf(stderr, "fs_read(%s): %s\n", path, err);
        return -1;
    }
    return json_find_string(resp, "text", out, cap);
}

static int call_fs_stat(const char *path, long long *size, int *is_dir)
{
    char pbuf[256], req[512], resp[BUF];
    if (json_encode_string(path, pbuf, sizeof pbuf) < 0) return -1;
    int n = snprintf(req, sizeof req,
        "{\"name\":\"fs_stat\",\"args\":{\"path\":%s}}", pbuf);
    if (n < 0 || (size_t)n >= sizeof req) return -1;
    if (hcall_exchange(req, resp, sizeof resp) < 0) return -1;
    if (hcall_has_error(resp)) {
        char err[256];
        json_find_string(resp, "error", err, sizeof err);
        fprintf(stderr, "fs_stat(%s): %s\n", path, err);
        return -1;
    }
    if (json_find_int(resp, "size", size) < 0) return -1;
    long long id = 0;
    if (strstr(resp, "\"is_dir\":true")) id = 1;
    *is_dir = (int)id;
    return 0;
}

static void call_fs_list(const char *path)
{
    char pbuf[256], req[512], resp[BUF];
    if (json_encode_string(path, pbuf, sizeof pbuf) < 0) return;
    int n = snprintf(req, sizeof req,
        "{\"name\":\"fs_list\",\"args\":{\"path\":%s}}", pbuf);
    if (n < 0 || (size_t)n >= sizeof req) return;
    if (hcall_exchange(req, resp, sizeof resp) < 0) return;
    if (hcall_has_error(resp)) {
        char err[256];
        json_find_string(resp, "error", err, sizeof err);
        fprintf(stderr, "fs_list(%s): %s\n", path, err);
        return;
    }
    // Minimal list printer: walk "name":"..." pairs in the "entries" array.
    const char *p = resp;
    while ((p = strstr(p, "\"name\":\"")) != NULL) {
        p += strlen("\"name\":\"");
        fputs("  - ", stdout);
        while (*p && *p != '"') {
            if (*p == '\\' && p[1]) p++;
            putchar(*p++);
        }
        putchar('\n');
    }
}

int main(void)
{
    printf("hostfs-c: exercising the host filesystem sandbox\n");

    // 1. Write a file.
    const char *greeting = "Hello from the Unikraft guest!\nSecond line.\n";
    int wrote = call_fs_write("greeting.txt", greeting, 0);
    if (wrote < 0) return 1;
    printf("wrote greeting.txt (%d bytes)\n", wrote);

    // 2. Read it back.
    char buf[1024];
    int n = call_fs_read("greeting.txt", buf, sizeof buf);
    if (n < 0) return 1;
    printf("read greeting.txt (%d bytes):\n---\n%s---\n", n, buf);

    // 3. Stat it.
    long long size = 0;
    int is_dir = 0;
    if (call_fs_stat("greeting.txt", &size, &is_dir) == 0) {
        printf("stat: size=%lld is_dir=%s\n", size, is_dir ? "true" : "false");
    }

    // 4. List the mount root.
    printf("mount root contents:\n");
    call_fs_list("");

    // 5. Prove escape attempts are rejected.
    printf("escape attempts (all should be rejected):\n");
    call_fs_read("../etc/passwd", buf, sizeof buf);
    call_fs_read("/etc/passwd", buf, sizeof buf);
    call_fs_read("subdir/../../outside.txt", buf, sizeof buf);

    printf("done.\n");
    return 0;
}
