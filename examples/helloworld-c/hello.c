/* Hello World for Hyperlight dispatch.
 * Uses direct port I/O for output.
 * Returns from _start instead of using exit syscall
 * (the dispatch function handles the halt protocol).
 */

static void hl_putc(char c) {
    __asm__ volatile("outb %0, %1"
        : : "a"((unsigned char)c), "Nd"((unsigned short)103));
}

static void hl_puts(const char *s) {
    while (*s)
        hl_putc(*s++);
}

void _start(void) {
    hl_puts("Hello from C on Hyperlight!\n");
    /* Return to dispatch function — it handles the halt protocol */
}
