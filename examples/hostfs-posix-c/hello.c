/* Full POSIX smoke test — no opendir for now (known-broken). */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static void die(const char *op, const char *path)
{
	fprintf(stderr, "%s %s: %s\n", op, path, strerror(errno));
}

int main(void)
{
	puts("hostfs-posix-c: unmodified POSIX against the sandboxed host mount");

	int fd = open("/host/greeting.txt", O_WRONLY | O_CREAT | O_TRUNC, 0666);
	if (fd < 0) { die("open", "/host/greeting.txt"); return 1; }
	const char *msg = "Hello from Unikraft via transparent POSIX!\n"
			  "No hcall helpers — just open + write.\n";
	ssize_t n = write(fd, msg, strlen(msg));
	printf("wrote /host/greeting.txt (%zd bytes)\n", n);
	close(fd);

	fd = open("/host/greeting.txt", O_RDONLY);
	if (fd < 0) { die("open", "/host/greeting.txt"); return 1; }
	char buf[1024];
	n = read(fd, buf, sizeof(buf) - 1);
	close(fd);
	if (n < 0) { die("read", "/host/greeting.txt"); return 1; }
	buf[n] = '\0';
	printf("read (%zd bytes):\n---\n%s---\n", n, buf);

	if (mkdir("/host/logs", 0777) < 0 && errno != EEXIST) {
		die("mkdir", "/host/logs");
		return 1;
	}
	fd = open("/host/logs/app.log", O_WRONLY | O_CREAT | O_APPEND, 0666);
	if (fd < 0) { die("open append", "/host/logs/app.log"); return 1; }
	write(fd, "line 1\n", 7);
	write(fd, "line 2\n", 7);
	close(fd);
	puts("appended to /host/logs/app.log");

	struct stat st;
	if (stat("/host/greeting.txt", &st) == 0)
		printf("stat: size=%lld\n", (long long)st.st_size);

	puts("done.");
	return 0;
}
