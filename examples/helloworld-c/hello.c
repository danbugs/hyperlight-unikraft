#include <unistd.h>

int main() {
    const char msg[] = "Hello from C on Hyperlight!\n";
    write(1, msg, sizeof(msg) - 1);
    return 0;
}
