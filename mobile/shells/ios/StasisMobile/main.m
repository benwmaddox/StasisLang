#include <SDL_main.h>

#ifdef main
#undef main
#endif

int main(int argc, char **argv) {
    return SDL_UIKitRunApp(argc, argv, SDL_main);
}
