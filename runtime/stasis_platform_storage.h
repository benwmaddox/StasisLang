#ifndef STASIS_PLATFORM_STORAGE_H
#define STASIS_PLATFORM_STORAGE_H

int stasis_storage_set_root(const char *root);
int stasis_storage_load_i32(const char *scope, const char *key, int fallback);
int stasis_storage_save_i32(const char *scope, const char *key, int value);

#endif
