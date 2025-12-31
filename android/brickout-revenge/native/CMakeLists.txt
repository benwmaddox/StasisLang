cmake_minimum_required(VERSION 3.20)

project(stasis_brickout_revenge_android C)

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)

if(NOT DEFINED STASIS_REPO_ROOT)
    message(FATAL_ERROR "STASIS_REPO_ROOT is required (repo root path)")
endif()

if(NOT DEFINED STASIS_GAME_LL)
    message(FATAL_ERROR "STASIS_GAME_LL is required (path to generated .ll)")
endif()

get_filename_component(STASIS_REPO_ROOT_ABS "${STASIS_REPO_ROOT}" ABSOLUTE)
get_filename_component(STASIS_GAME_LL_ABS "${STASIS_GAME_LL}" ABSOLUTE)

if(NOT EXISTS "${STASIS_GAME_LL_ABS}")
    message(FATAL_ERROR "STASIS_GAME_LL not found: ${STASIS_GAME_LL_ABS}")
endif()

# Build the Stasis game LLVM IR into an object file with the active toolchain compiler.
string(TOUPPER "${CMAKE_BUILD_TYPE}" STASIS_BUILD_TYPE_UPPER)
set(STASIS_GAME_CFLAGS "${CMAKE_C_FLAGS} ${CMAKE_C_FLAGS_${STASIS_BUILD_TYPE_UPPER}}")
separate_arguments(STASIS_GAME_CFLAGS)
set(STASIS_GAME_TARGET_FLAGS "")
if(CMAKE_C_COMPILER_TARGET)
    list(APPEND STASIS_GAME_TARGET_FLAGS "--target=${CMAKE_C_COMPILER_TARGET}")
elseif(CMAKE_ANDROID_ARCH_ABI AND CMAKE_ANDROID_API)
    if(CMAKE_ANDROID_ARCH_ABI STREQUAL "arm64-v8a")
        list(APPEND STASIS_GAME_TARGET_FLAGS "--target=aarch64-none-linux-android${CMAKE_ANDROID_API}")
    endif()
endif()
if(CMAKE_SYSROOT)
    list(APPEND STASIS_GAME_TARGET_FLAGS "--sysroot=${CMAKE_SYSROOT}")
endif()
set(GAME_OBJ "${CMAKE_CURRENT_BINARY_DIR}/brickout_revenge.o")
add_custom_command(
    OUTPUT "${GAME_OBJ}"
    COMMAND "${CMAKE_C_COMPILER}" ${STASIS_GAME_TARGET_FLAGS} ${STASIS_GAME_CFLAGS} -c "${STASIS_GAME_LL_ABS}" -o "${GAME_OBJ}"
    DEPENDS "${STASIS_GAME_LL_ABS}"
    VERBATIM
)
add_custom_target(stasis_game_obj DEPENDS "${GAME_OBJ}")

find_package(SDL2 CONFIG REQUIRED)

add_library(main SHARED
    stasis_android_main.c
    stasis_android_shims.c
    "${STASIS_REPO_ROOT_ABS}/runtime/stasis_graphics.c"
    "${STASIS_REPO_ROOT_ABS}/runtime/stasis_data.c"
    "${STASIS_REPO_ROOT_ABS}/runtime/cJSON.c"
    "${GAME_OBJ}"
)
add_dependencies(main stasis_game_obj)

target_include_directories(main PRIVATE
    "${STASIS_REPO_ROOT_ABS}/runtime"
)

target_compile_definitions(main PRIVATE
    STASIS_GRAPHICS_SDL_ONLY=1
)

target_link_libraries(main PRIVATE
    SDL2::SDL2
    c
)

if(DEFINED VCPKG_INSTALLED_DIR AND DEFINED VCPKG_TARGET_TRIPLET)
    set(STASIS_ICONV_HINTS "")
    if(CMAKE_BUILD_TYPE STREQUAL "Debug")
        list(APPEND STASIS_ICONV_HINTS "${VCPKG_INSTALLED_DIR}/${VCPKG_TARGET_TRIPLET}/debug/lib")
    endif()
    list(APPEND STASIS_ICONV_HINTS "${VCPKG_INSTALLED_DIR}/${VCPKG_TARGET_TRIPLET}/lib")
    find_library(STASIS_ICONV_LIB NAMES iconv libiconv HINTS ${STASIS_ICONV_HINTS})
    if(STASIS_ICONV_LIB)
        target_link_libraries(main PRIVATE "${STASIS_ICONV_LIB}")
    endif()
endif()
