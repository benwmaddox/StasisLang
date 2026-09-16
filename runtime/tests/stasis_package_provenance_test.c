#include "stasis_package_provenance_reader.h"

#include <stdio.h>
#include <stdlib.h>

#define CHECK(condition) do { \
    if (!(condition)) { \
        fprintf(stderr, "check failed: %s (%s:%d)\n", #condition, __FILE__, __LINE__); \
        return 1; \
    } \
} while (0)

static FILE* provenance_file(size_t count, int append_nul) {
    FILE* file = tmpfile();
    if (!file) return NULL;
    for (size_t index = 0; index < count; ++index) {
        if (fputc('x', file) == EOF) {
            fclose(file);
            return NULL;
        }
    }
    if (append_nul && fputc('\0', file) == EOF) {
        fclose(file);
        return NULL;
    }
    rewind(file);
    return file;
}

int main(void) {
    const size_t capacity = 16;
    char buffer[16];
    size_t count = 0;

    FILE* exact = provenance_file(capacity, 0);
    CHECK(exact != NULL);
    CHECK(stasis_read_package_provenance(exact, buffer, capacity, &count) ==
        STASIS_PACKAGE_PROVENANCE_READ_OK);
    CHECK(count == capacity);
    fclose(exact);

    FILE* oversized = provenance_file(capacity + 1, 0);
    CHECK(oversized != NULL);
    CHECK(stasis_read_package_provenance(oversized, buffer, capacity, &count) ==
        STASIS_PACKAGE_PROVENANCE_READ_TOO_LARGE);
    fclose(oversized);

    FILE* embedded_nul = provenance_file(capacity - 1, 1);
    CHECK(embedded_nul != NULL);
    CHECK(stasis_read_package_provenance(embedded_nul, buffer, capacity, &count) ==
        STASIS_PACKAGE_PROVENANCE_READ_EMBEDDED_NUL);
    fclose(embedded_nul);

    CHECK(STASIS_PACKAGE_PROVENANCE_MAX_BYTES == 1024U * 1024U);
    return 0;
}
