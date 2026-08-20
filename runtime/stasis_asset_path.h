#ifndef STASIS_ASSET_PATH_H
#define STASIS_ASSET_PATH_H

#include <stddef.h>
#include <string.h>

static int stasis_asset_path_is_virtual_root(const char *path) {
    return path != NULL &&
        (strcmp(path, "/assets") == 0 || strncmp(path, "/assets/", 8) == 0);
}

static int stasis_asset_normalize_relative_path(
    const char *path,
    char *out,
    size_t out_size
) {
    const char *cursor = path;
    size_t used = 0;
    if (path == NULL || out == NULL || out_size < 2) return 0;
    int rooted = stasis_asset_path_is_virtual_root(path);
    if (path[0] == '/' && !rooted) return 0;
    if (path[0] == '\\') return 0;
    if (((path[0] >= 'A' && path[0] <= 'Z') ||
         (path[0] >= 'a' && path[0] <= 'z')) &&
        path[1] == ':') {
        return 0;
    }
    if (strstr(path, "://") != NULL) return 0;
    if (rooted) cursor = path + 1;

    while (*cursor != '\0') {
        while (*cursor == '/' || *cursor == '\\') cursor += 1;
        if (*cursor == '\0') break;

        const char *segment = cursor;
        while (*cursor != '\0' && *cursor != '/' && *cursor != '\\') cursor += 1;
        size_t segment_len = (size_t)(cursor - segment);
        if (segment_len == 1 && segment[0] == '.') continue;
        if (segment_len == 2 && segment[0] == '.' && segment[1] == '.') {
            while (used > 0 && out[used - 1] != '/') used -= 1;
            if (used > 0) used -= 1;
            continue;
        }

        size_t separator = used > 0 ? 1 : 0;
        if (used + separator + segment_len + 1 > out_size) return 0;
        if (separator != 0) out[used++] = '/';
        memcpy(out + used, segment, segment_len);
        used += segment_len;
    }

    if (used == 0) return 0;
    out[used] = '\0';
    if (rooted && strcmp(out, "assets") != 0 && strncmp(out, "assets/", 7) != 0) {
        return 0;
    }
    return 1;
}

#endif
