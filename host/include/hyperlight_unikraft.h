/* hyperlight-unikraft C API for embedding Unikraft VMs via Hyperlight */
#ifndef HYPERLIGHT_UNIKRAFT_H
#define HYPERLIGHT_UNIKRAFT_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* VM status codes */
#define HL_STATUS_CREATED 0
#define HL_STATUS_RUNNING 1
#define HL_STATUS_STOPPED 2
#define HL_STATUS_ERROR   3

/* Opaque VM handle */
typedef struct HlVm HlVm;

/* VM configuration */
typedef struct HlConfig {
    const char *kernel_path;    /* required: path to unikernel ELF */
    const char *initrd_path;    /* optional: path to CPIO initrd (NULL if none) */
    const char **app_args;      /* optional: application arguments array (NULL if none) */
    int app_args_count;         /* number of app_args entries */
    uint64_t heap_size;         /* heap size in bytes */
    uint64_t stack_size;        /* stack size in bytes */
} HlConfig;

/* Get last error message for the current thread. Returns NULL if no error.
 * Pointer valid until next FFI call on same thread. */
const char *hl_last_error(void);

/* Create a VM handle. Reads initrd, prepares config.
 * Returns NULL on failure (check hl_last_error). */
HlVm *hl_vm_create(const HlConfig *config);

/* Start VM in a background thread. Returns 0 on success, -1 on failure. */
int hl_vm_start(HlVm *vm);

/* Get current VM status (HL_STATUS_*). Returns -1 if vm is NULL. */
int hl_vm_status(const HlVm *vm);

/* Block until VM finishes. Returns 0 on success, -1 on failure. */
int hl_vm_wait(HlVm *vm);

/* Get captured output. Valid after VM stops.
 * Pointer valid until next hl_vm_output or hl_vm_free call on same VM. */
const char *hl_vm_output(const HlVm *vm);

/* Get error message if status is HL_STATUS_ERROR. Returns NULL otherwise. */
const char *hl_vm_error(const HlVm *vm);

/* Free VM handle. Waits for running thread first. */
void hl_vm_free(HlVm *vm);

#ifdef __cplusplus
}
#endif

#endif /* HYPERLIGHT_UNIKRAFT_H */
