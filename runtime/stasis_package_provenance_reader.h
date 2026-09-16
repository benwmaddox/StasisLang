#ifndef STASIS_PACKAGE_PROVENANCE_READER_H
#define STASIS_PACKAGE_PROVENANCE_READER_H

#include <limits.h>
#include <stddef.h>
#include <stdio.h>
#include <string.h>

#define STASIS_PACKAGE_PROVENANCE_MAX_BYTES (1024U * 1024U)

#if STASIS_PACKAGE_PROVENANCE_MAX_BYTES > INT_MAX
#error "package provenance log bound must fit the printf precision argument"
#endif

typedef enum StasisPackageProvenanceReadResult {
    STASIS_PACKAGE_PROVENANCE_READ_OK = 0,
    STASIS_PACKAGE_PROVENANCE_READ_TOO_LARGE = 1,
    STASIS_PACKAGE_PROVENANCE_READ_ERROR = 2,
    STASIS_PACKAGE_PROVENANCE_READ_EMBEDDED_NUL = 3,
} StasisPackageProvenanceReadResult;

static inline StasisPackageProvenanceReadResult stasis_read_package_provenance(
    FILE* file,
    char* buffer,
    size_t capacity,
    size_t* out_count) {
    if (!file || !buffer || capacity == 0 || !out_count) {
        return STASIS_PACKAGE_PROVENANCE_READ_ERROR;
    }
    size_t count = fread(buffer, 1, capacity, file);
    if (ferror(file)) {
        return STASIS_PACKAGE_PROVENANCE_READ_ERROR;
    }
    if (count == capacity && fgetc(file) != EOF) {
        return STASIS_PACKAGE_PROVENANCE_READ_TOO_LARGE;
    }
    if (ferror(file)) {
        return STASIS_PACKAGE_PROVENANCE_READ_ERROR;
    }
    if (memchr(buffer, '\0', count) != NULL) {
        return STASIS_PACKAGE_PROVENANCE_READ_EMBEDDED_NUL;
    }
    *out_count = count;
    return STASIS_PACKAGE_PROVENANCE_READ_OK;
}

#endif
