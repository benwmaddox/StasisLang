#include "stasis_platform_storage.h"

#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

int main(void) {
    char root[256];
    char corrupt_path[320];
    FILE *corrupt;
    snprintf(root, sizeof(root), "/tmp/stasis_platform_storage_%ld", (long)getpid());
    assert(mkdir(root, 0700) == 0);
    assert(stasis_storage_set_root(root) == 1);
    assert(stasis_storage_load_i32("game", "tier", 1) == 1);
    assert(stasis_storage_save_i32("game", "tier", 4) == 1);
    assert(stasis_storage_load_i32("game", "tier", 1) == 4);
    assert(stasis_storage_save_i32("../game", "tier", 5) == 0);

    snprintf(corrupt_path, sizeof(corrupt_path), "%s/game/tier.i32", root);
    corrupt = fopen(corrupt_path, "wb");
    assert(corrupt != NULL);
    assert(fputs("surprise\n", corrupt) >= 0);
    assert(fclose(corrupt) == 0);
    assert(stasis_storage_load_i32("game", "tier", 2) == 2);

    assert(remove(corrupt_path) == 0);
    snprintf(corrupt_path, sizeof(corrupt_path), "%s/game", root);
    assert(rmdir(corrupt_path) == 0);
    assert(rmdir(root) == 0);
    return 0;
}
