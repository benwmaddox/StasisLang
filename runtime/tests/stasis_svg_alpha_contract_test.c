#include <stdio.h>

int stasis_test_svg_bake_alpha_contract(const char* path, int* partial_count);

int main(int argc, char** argv) {
    if (argc != 2) return 1;
    int partial_count = 0;
    if (!stasis_test_svg_bake_alpha_contract(argv[1], &partial_count)) {
        fprintf(stderr, "SVG bake did not produce premultiplied RGBA\n");
        return 2;
    }
    if (partial_count <= 0) {
        fprintf(stderr, "SVG fixture did not exercise a partial-alpha edge\n");
        return 3;
    }
    return 0;
}
