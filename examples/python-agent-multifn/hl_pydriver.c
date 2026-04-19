/*
 * hl_pydriver — Python 3.12 embedding driver for Hyperlight multi-function
 * dispatch. Replaces /usr/local/bin/python3 as the ELF the Unikraft
 * guest loads, and exposes two named guest functions:
 *
 *   init()       - one-time Py_Initialize() + warm-up imports
 *   run(code)    - PyRun_SimpleString(code)
 *
 * Called from the host as:
 *
 *   sandbox.call_named("init", ())                 (once)
 *   sandbox.snapshot_now()                          (capture warm state)
 *   loop:
 *       sandbox.restore()
 *       sandbox.call_named("run", "<user code>")
 *
 * main() runs on every dispatch; the static `py_initialized` flag plus
 * the fact that snapshot/restore preserves global state means the heavy
 * import cost is paid exactly once per VM lifetime, in the one init()
 * call — every subsequent run() starts with numpy/pandas already loaded
 * in `sys.modules`.
 *
 * The current in-flight FunctionCall bytes reach us via two slot
 * pointers app-elfloader injected into envp at boot:
 *   HL_FC_BYTES_PTR=0x...   (const uint8_t ** — bytes slot)
 *   HL_FC_LEN_PTR=0x...     (size_t *         — length slot)
 */

#define PY_SSIZE_T_CLEAN
#include <Python.h>
#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

/* Minimal FlatBuffer reader for the Hyperlight FunctionCall shape.
 * Kept in-line so we don't need to link against or include any kernel
 * headers — the format is stable and documented in the Hyperlight
 * schema (src/schema/function_*.fbs).
 */
static inline uint32_t fb_u32(const uint8_t *b, size_t o)
{
	return b[o] | ((uint32_t)b[o+1] << 8) |
	       ((uint32_t)b[o+2] << 16) | ((uint32_t)b[o+3] << 24);
}
static inline uint16_t fb_u16(const uint8_t *b, size_t o)
{
	return b[o] | ((uint16_t)b[o+1] << 8);
}
static inline int32_t fb_i32(const uint8_t *b, size_t o)
{
	return (int32_t)fb_u32(b, o);
}
static inline size_t fb_vtable(const uint8_t *b, size_t tbl)
{
	return tbl - fb_i32(b, tbl);
}
static inline uint16_t fb_field(const uint8_t *b, size_t tbl, uint16_t vt)
{
	size_t v = fb_vtable(b, tbl);
	uint16_t vs = fb_u16(b, v);
	return vt >= vs ? 0 : fb_u16(b, v + vt);
}
static inline size_t fb_follow(const uint8_t *b, size_t tbl, uint16_t vt)
{
	uint16_t f = fb_field(b, tbl, vt);
	if (!f) return 0;
	size_t p = tbl + f;
	return p + fb_u32(b, p);
}

static int parse_fc_name(const uint8_t *b, size_t len,
			 const char **out, size_t *out_len)
{
	if (len < 8) return -1;
	size_t fc = 4 + fb_u32(b, 4);
	if (fc >= len) return -1;
	size_t p = fb_follow(b, fc, 4);
	if (!p || p + 4 > len) return -1;
	uint32_t nlen = fb_u32(b, p);
	if (p + 4 + nlen > len) return -1;
	*out = (const char *)(b + p + 4);
	*out_len = nlen;
	return 0;
}

/* First parameter as hlstring. value_type=7 = hlstring in the
 * ParameterValue union (src/schema/function_types.fbs).
 */
static const char *parse_fc_arg0_string(const uint8_t *b, size_t len,
					size_t *out_len)
{
	if (len < 8) return NULL;
	size_t fc = 4 + fb_u32(b, 4);
	size_t params = fb_follow(b, fc, 6);
	if (!params) return NULL;
	uint32_t plen = fb_u32(b, params);
	if (plen == 0) return NULL;
	size_t p0_pos = params + 4;
	size_t p0 = p0_pos + fb_u32(b, p0_pos);
	uint16_t tf = fb_field(b, p0, 4);
	if (!tf) return NULL;
	uint8_t vt = b[p0 + tf];
	if (vt != 7) return NULL; /* not hlstring */
	size_t hs = fb_follow(b, p0, 6);
	if (!hs) return NULL;
	size_t s = fb_follow(b, hs, 4);
	if (!s || s + 4 > len) return NULL;
	uint32_t slen = fb_u32(b, s);
	if (s + 4 + slen > len) return NULL;
	*out_len = slen;
	return (const char *)(b + s + 4);
}

/* One-time Python init + warm-up imports. The snapshot the host takes
 * after this call captures sys.modules["numpy"]/["pandas"]/... so
 * subsequent run() calls get them for free.
 */
static void py_init_once(void)
{
	Py_Initialize();

	/* sys.argv so scripts that read it don't crash. */
	PyRun_SimpleString(
		"import sys\n"
		"sys.argv = ['hl_pydriver']\n");

	/* Warm up heavy deps used by python-agent. Best-effort — any
	 * failure gets logged but doesn't stop init; the run() call
	 * that actually needs the module will raise its own traceback.
	 */
	PyRun_SimpleString(
		"import sys, importlib\n"
		"for _mod in ("
		"    'numpy', 'pandas', 'pydantic', 'yaml', 'jinja2',"
		"    'bs4', 'tabulate', 'click', 'tenacity', 'tqdm',"
		"    'openpyxl', 'pypdf', 'markdown_it', 'PIL', 'lxml',"
		"    'cryptography', 'dateutil', 'dotenv'):\n"
		"  try:\n"
		"    importlib.import_module(_mod)\n"
		"  except Exception as e:\n"
		"    sys.stderr.write(f'warn: preload {_mod} failed: {e}\\n')\n");
}

int main(int argc, char **argv, char **envp)
{
	static const uint8_t **fc_bytes_slot;
	static size_t *fc_len_slot;
	static int py_initialized;

	/* Resolve slot addresses once; subsequent dispatches reuse them. */
	if (!fc_bytes_slot) {
		for (char **p = envp; p && *p; p++) {
			if (!strncmp(*p, "HL_FC_BYTES_PTR=", 16)) {
				unsigned long v =
					strtoul(*p + 16, NULL, 16);
				fc_bytes_slot =
					(const uint8_t **)(uintptr_t)v;
			} else if (!strncmp(*p, "HL_FC_LEN_PTR=", 14)) {
				unsigned long v =
					strtoul(*p + 14, NULL, 16);
				fc_len_slot = (size_t *)(uintptr_t)v;
			}
		}
		if (!fc_bytes_slot || !fc_len_slot) {
			fprintf(stderr,
				"hl_pydriver: HL_FC_*_PTR env vars missing\n");
			return 1;
		}
	}

	const uint8_t *fc = *fc_bytes_slot;
	size_t fc_len = *fc_len_slot;
	if (!fc || fc_len == 0) {
		fprintf(stderr, "hl_pydriver: no current FC bytes\n");
		return 1;
	}

	const char *name = NULL;
	size_t name_len = 0;
	if (parse_fc_name(fc, fc_len, &name, &name_len) < 0) {
		fprintf(stderr, "hl_pydriver: FC parse failed\n");
		return 1;
	}

	if (name_len == 4 && !memcmp(name, "init", 4)) {
		if (!py_initialized) {
			py_init_once();
			py_initialized = 1;
		}
	} else if (name_len == 3 && !memcmp(name, "run", 3)) {
		if (!py_initialized || !Py_IsInitialized()) {
			py_init_once();
			py_initialized = 1;
		}
		size_t arg_len = 0;
		const char *arg = parse_fc_arg0_string(fc, fc_len, &arg_len);
		if (!arg) {
			fprintf(stderr,
				"hl_pydriver: run() requires a string arg\n");
			return 1;
		}
		char *code = malloc(arg_len + 1);
		if (!code) {
			fprintf(stderr, "hl_pydriver: OOM\n");
			return 1;
		}
		memcpy(code, arg, arg_len);
		code[arg_len] = '\0';
		PyRun_SimpleString(code);
		free(code);
	} else {
		fprintf(stderr, "hl_pydriver: unknown fn '%.*s'\n",
			(int)name_len, name);
		return 1;
	}

	fflush(stdout);
	fflush(stderr);
	return 0;
}
