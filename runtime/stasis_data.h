/*
 * stasis_data.h - Data hot-reload system for Stasis
 */

#ifndef STASIS_DATA_H
#define STASIS_DATA_H

#ifdef _WIN32
#include <windows.h>
#endif

#ifdef __cplusplus
extern "C" {
#endif

/* Initialize the data binding system */
void stasis_data_init(void);

/* Set the current DLL handle for symbol lookup (must be called after each hot-swap) */
#ifdef _WIN32
void stasis_data_set_dll(HMODULE dll);
#else
void stasis_data_set_dll(void* dll);
#endif

/* Register a data binding. Returns handle (1-based) or 0 on failure. */
int stasis_data_bind(const char* json_file_path, const char* struct_meta_path);

/* Check for data file changes and reload if needed. Returns 1 if reloaded. */
int stasis_data_poll(int handle);

/* Poll all active bindings. Returns number of bindings reloaded. */
int stasis_data_poll_all(void);

/* Check if a binding has an error */
int stasis_data_has_error(int handle);

/* Get error message for a binding */
const char* stasis_data_get_error(int handle);

/* Unbind a data binding */
void stasis_data_unbind(int handle);

/* Cleanup all bindings */
void stasis_data_cleanup(void);

#ifdef __cplusplus
}
#endif

#endif /* STASIS_DATA_H */
