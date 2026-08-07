cmake_minimum_required(VERSION 3.16)
project(stasis_graphics C)

set(STASIS_RELEASE_ID "development" CACHE STRING "Immutable Stasis toolchain release identity")

set(STASIS_MOBILE_PLATFORM OFF)
if(ANDROID OR CMAKE_SYSTEM_NAME STREQUAL "iOS")
    set(STASIS_MOBILE_PLATFORM ON)
endif()

set(STASIS_DESKTOP_TARGET_DEFAULT ON)
if(STASIS_MOBILE_PLATFORM)
    set(STASIS_DESKTOP_TARGET_DEFAULT OFF)
endif()

option(
    STASIS_GRAPHICS_BUILD_SHARED
    "Build the graphics runtime as a DLL"
    ${STASIS_DESKTOP_TARGET_DEFAULT}
)
option(
    STASIS_GRAPHICS_BUILD_STATIC
    "Build the desktop graphics static library"
    ${STASIS_DESKTOP_TARGET_DEFAULT}
)
option(
    STASIS_GRAPHICS_BUNDLE_SDL
    "Build pinned SDL3 and SDL3_image into the graphics runtime"
    OFF
)
option(STASIS_BUILD_RUNNER "Build the stasis DLL runner executable" ${STASIS_DESKTOP_TARGET_DEFAULT})
option(STASIS_BUILD_SYS "Build the sys runtime (file/argv/exec)" ${STASIS_DESKTOP_TARGET_DEFAULT})
option(
    STASIS_BUILD_MOBILE_RUNTIME
    "Build the shared SDL-only mobile runtime core"
    ${STASIS_MOBILE_PLATFORM}
)
option(STASIS_MOBILE_RUNTIME_BUILD_TESTS "Build mobile runtime lifecycle tests" OFF)

# Android and other restricted platforms do not support the OpenGL 2.1 + GLEW path.
option(
    STASIS_GRAPHICS_SDL_ONLY
    "Build the canonical SDL_Renderer runtime (disable only for legacy GL conformance)"
    ON
)

# Prefer static CRT when possible (for single-exe bundling)
if (MSVC)
    set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")
endif()

# Find graphics deps only when building graphics targets.
if(STASIS_GRAPHICS_BUILD_SHARED OR STASIS_GRAPHICS_BUILD_STATIC OR STASIS_BUILD_MOBILE_RUNTIME)
    if(STASIS_GRAPHICS_BUNDLE_SDL)
        include(FetchContent)
        set(CMAKE_POSITION_INDEPENDENT_CODE ON)
        set(SDL_SHARED OFF CACHE BOOL "" FORCE)
        set(SDL_STATIC ON CACHE BOOL "" FORCE)
        set(SDL_TEST OFF CACHE BOOL "" FORCE)
        set(SDL_TEST_LIBRARY OFF CACHE BOOL "" FORCE)
        set(SDL_TESTS OFF CACHE BOOL "" FORCE)
        set(SDL_DISABLE_INSTALL ON CACHE BOOL "" FORCE)
        FetchContent_Declare(
            stasis_sdl3
            URL https://github.com/libsdl-org/SDL/releases/download/release-3.4.10/SDL3-3.4.10.tar.gz
            URL_HASH SHA256=12b34280415ec8418c864408b93d008a20a6530687ee613d60bfbd20411f2785
            DOWNLOAD_EXTRACT_TIMESTAMP TRUE
        )
        FetchContent_MakeAvailable(stasis_sdl3)

        set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
        set(SDLIMAGE_INSTALL OFF CACHE BOOL "" FORCE)
        set(SDLIMAGE_SAMPLES OFF CACHE BOOL "" FORCE)
        set(SDLIMAGE_TESTS OFF CACHE BOOL "" FORCE)
        set(SDLIMAGE_VENDORED OFF CACHE BOOL "" FORCE)
        set(SDLIMAGE_BACKEND_STB ON CACHE BOOL "" FORCE)
        set(SDLIMAGE_BACKEND_IMAGEIO OFF CACHE BOOL "" FORCE)
        set(SDLIMAGE_AVIF OFF CACHE BOOL "" FORCE)
        set(SDLIMAGE_JXL OFF CACHE BOOL "" FORCE)
        set(SDLIMAGE_TIF OFF CACHE BOOL "" FORCE)
        set(SDLIMAGE_WEBP OFF CACHE BOOL "" FORCE)
        set(SDLIMAGE_PNG_LIBPNG OFF CACHE BOOL "" FORCE)
        FetchContent_Declare(
            stasis_sdl3_image
            URL https://github.com/libsdl-org/SDL_image/releases/download/release-3.4.4/SDL3_image-3.4.4.tar.gz
            URL_HASH SHA256=29751304a13d25ac513f24305fa25b06a6edd9607718c90129b8350d35fc5573
            DOWNLOAD_EXTRACT_TIMESTAMP TRUE
        )
        FetchContent_MakeAvailable(stasis_sdl3_image)
    endif()

    # Find SDL3 (via vcpkg, system package, or the pinned bundled family).
    if(NOT TARGET SDL3::SDL3 AND NOT TARGET SDL3::SDL3-static)
        find_package(SDL3 CONFIG REQUIRED)
    endif()
    if(NOT TARGET SDL3_image::SDL3_image AND NOT TARGET SDL3_image::SDL3_image-static)
        find_package(SDL3_image CONFIG REQUIRED)
    endif()

    if((STASIS_GRAPHICS_BUILD_SHARED OR STASIS_GRAPHICS_BUILD_STATIC) AND
       NOT STASIS_GRAPHICS_SDL_ONLY)
        # Find OpenGL and GLEW (desktop backend)
        find_package(OpenGL REQUIRED)
        find_package(GLEW REQUIRED)
    endif()
endif()

function(configure_stasis_target target is_static sdl_only)
    target_sources(${target} PRIVATE stasis_graphics.c)
    target_compile_definitions(${target} PRIVATE STASIS_RELEASE_ID="${STASIS_RELEASE_ID}")
    target_link_libraries(${target} PRIVATE
        $<IF:$<TARGET_EXISTS:SDL3::SDL3>,SDL3::SDL3,SDL3::SDL3-static>
        $<IF:$<TARGET_EXISTS:SDL3_image::SDL3_image>,SDL3_image::SDL3_image,SDL3_image::SDL3_image-static>
    )

    if(NOT sdl_only)
        target_link_libraries(${target} PRIVATE
            OpenGL::GL
            GLEW::GLEW
        )
    else()
        target_compile_definitions(${target} PRIVATE STASIS_GRAPHICS_SDL_ONLY)
    endif()

    if(NOT WIN32)
        target_link_libraries(${target} PRIVATE m)
    endif()

    if(WIN32)
        target_compile_definitions(${target} PRIVATE _CRT_SECURE_NO_WARNINGS)
    endif()

    if(NOT WIN32 AND (CMAKE_SYSTEM_PROCESSOR MATCHES "x86_64|AMD64|i[3-6]86"))
        # Defensive stack realignment for callers that don't maintain 16-byte alignment (e.g., some JIT/AOT runtimes).
        target_compile_options(${target} PRIVATE -mstackrealign)
    endif()

    if(${is_static})
        if(NOT sdl_only)
            target_compile_definitions(${target} PRIVATE STASIS_GRAPHICS_STATIC GLEW_STATIC)
        else()
            target_compile_definitions(${target} PRIVATE STASIS_GRAPHICS_STATIC)
        endif()
    endif()
endfunction()

if(STASIS_GRAPHICS_BUILD_SHARED)
    add_library(stasis_graphics SHARED)
    configure_stasis_target(stasis_graphics OFF ${STASIS_GRAPHICS_SDL_ONLY})

    # On Windows, use .def file for explicit exports
    if(WIN32)
        target_sources(stasis_graphics PRIVATE stasis_graphics.def)
    endif()

    set_target_properties(stasis_graphics PROPERTIES
        RUNTIME_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/bin"
        LIBRARY_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/bin"
        ARCHIVE_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/bin"
    )
    install(TARGETS stasis_graphics
        RUNTIME DESTINATION bin
        LIBRARY DESTINATION lib
    )
endif()

if(STASIS_GRAPHICS_BUILD_STATIC)
    add_library(stasis_graphics_static STATIC)
    configure_stasis_target(stasis_graphics_static ON ${STASIS_GRAPHICS_SDL_ONLY})
    set_target_properties(stasis_graphics_static PROPERTIES
        ARCHIVE_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/Release"
        ARCHIVE_OUTPUT_DIRECTORY_RELEASE "${CMAKE_BINARY_DIR}/Release"
        ARCHIVE_OUTPUT_DIRECTORY_DEBUG "${CMAKE_BINARY_DIR}/Debug"
    )
    install(TARGETS stasis_graphics_static
        ARCHIVE DESTINATION lib
    )
endif()

if(STASIS_BUILD_MOBILE_RUNTIME)
    add_library(
        stasis_mobile_runtime STATIC
        stasis_mobile_runtime.c
        stasis_mobile_aot_runtime.c
    )
    configure_stasis_target(stasis_mobile_runtime ON TRUE)
    target_include_directories(stasis_mobile_runtime PUBLIC "${CMAKE_CURRENT_SOURCE_DIR}")
    set_target_properties(stasis_mobile_runtime PROPERTIES
        POSITION_INDEPENDENT_CODE ON
        ARCHIVE_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/mobile"
    )
    install(TARGETS stasis_mobile_runtime ARCHIVE DESTINATION lib)
    install(
        FILES stasis_asset_path.h stasis_mobile_runtime.h stasis_mobile_aot_runtime.h
        DESTINATION include
    )
endif()

if(STASIS_MOBILE_RUNTIME_BUILD_TESTS)
    enable_testing()
    add_executable(
        stasis_render_contract_test
        tests/stasis_render_contract_test.c
    )
    target_include_directories(stasis_render_contract_test PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}")
    add_test(NAME stasis_render_command_contract COMMAND stasis_render_contract_test)
    add_executable(
        stasis_display_scale_test
        tests/stasis_display_scale_test.c
    )
    target_include_directories(stasis_display_scale_test PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}")
    if(NOT WIN32)
        target_link_libraries(stasis_display_scale_test PRIVATE m)
    endif()
    add_test(NAME stasis_display_scale_contract COMMAND stasis_display_scale_test)
    add_executable(
        stasis_renderer_lifecycle_test
        tests/stasis_renderer_lifecycle_test.c
    )
    target_include_directories(stasis_renderer_lifecycle_test PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}")
    add_test(NAME stasis_renderer_lifecycle_contract COMMAND stasis_renderer_lifecycle_test)
    if(TARGET stasis_graphics_static)
        add_executable(
            stasis_font_cache_test
            tests/stasis_font_cache_test.c
        )
        target_link_libraries(stasis_font_cache_test PRIVATE stasis_graphics_static)
        target_compile_definitions(
            stasis_font_cache_test
            PRIVATE STASIS_TEST_FONT_PATH="${CMAKE_SOURCE_DIR}/../apps/stasis/assets/gauntlet-font/Basic-Regular.ttf"
        )
        add_test(NAME stasis_font_cache_contract COMMAND stasis_font_cache_test)
        set_tests_properties(
            stasis_font_cache_contract
            PROPERTIES ENVIRONMENT "SDL_VIDEODRIVER=dummy;SDL_RENDER_DRIVER=software"
        )
    endif()
    add_executable(
        stasis_asset_path_test
        tests/stasis_asset_path_test.c
    )
    target_include_directories(stasis_asset_path_test PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}")
    add_test(NAME stasis_asset_path_contract COMMAND stasis_asset_path_test)
    add_executable(
        stasis_mobile_runtime_test
        tests/stasis_mobile_runtime_test.c
        stasis_mobile_runtime.c
        stasis_mobile_aot_runtime.c
    )
    target_include_directories(stasis_mobile_runtime_test PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}")
    add_test(NAME stasis_mobile_runtime_lifecycle COMMAND stasis_mobile_runtime_test)
    add_executable(
        stasis_mobile_aot_runtime_test
        tests/stasis_mobile_aot_runtime_test.c
        stasis_mobile_aot_runtime.c
    )
    target_include_directories(stasis_mobile_aot_runtime_test PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}")
    if(NOT WIN32)
        target_link_libraries(stasis_mobile_runtime_test PRIVATE m)
        target_link_libraries(stasis_mobile_aot_runtime_test PRIVATE m)
    endif()
    add_test(NAME stasis_mobile_aot_runtime_contract COMMAND stasis_mobile_aot_runtime_test)
    if(UNIX)
        add_executable(
            stasis_platform_storage_test
            tests/stasis_platform_storage_test.c
            stasis_platform_storage.c
        )
        target_include_directories(stasis_platform_storage_test PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}")
        add_test(NAME stasis_platform_storage_contract COMMAND stasis_platform_storage_test)
    endif()
endif()

if(STASIS_BUILD_SYS)
    add_library(stasis_sys_static STATIC stasis_sys.c)
    if(NOT WIN32)
        set_target_properties(stasis_sys_static PROPERTIES POSITION_INDEPENDENT_CODE ON)
    endif()
    if(WIN32)
        target_compile_definitions(stasis_sys_static PRIVATE _CRT_SECURE_NO_WARNINGS)
    endif()
    set_target_properties(stasis_sys_static PROPERTIES
        ARCHIVE_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/Release"
        ARCHIVE_OUTPUT_DIRECTORY_RELEASE "${CMAKE_BINARY_DIR}/Release"
        ARCHIVE_OUTPUT_DIRECTORY_DEBUG "${CMAKE_BINARY_DIR}/Debug"
    )
endif()

if(STASIS_BUILD_RUNNER)
    add_executable(stasis_runner stasis_runner.c stasis_data.c cJSON.c)
    if(WIN32)
        target_sources(stasis_runner PRIVATE stasis_runner.manifest)
        target_compile_definitions(stasis_runner PRIVATE _CRT_SECURE_NO_WARNINGS)
    else()
        target_link_libraries(stasis_runner PRIVATE dl)
    endif()
    if(APPLE AND NOT CMAKE_SYSTEM_NAME STREQUAL "iOS")
        set_target_properties(stasis_runner PROPERTIES
            MACOSX_BUNDLE TRUE
            MACOSX_BUNDLE_INFO_PLIST "${CMAKE_CURRENT_SOURCE_DIR}/stasis_runner_macos.plist.in"
            MACOSX_BUNDLE_BUNDLE_NAME "Stasis Runner"
            MACOSX_BUNDLE_GUI_IDENTIFIER "org.stasislang.runner"
            MACOSX_BUNDLE_BUNDLE_VERSION "1"
            MACOSX_BUNDLE_SHORT_VERSION_STRING "1.0"
        )
    endif()
    set_target_properties(stasis_runner PROPERTIES
        RUNTIME_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/bin"
        RUNTIME_OUTPUT_DIRECTORY_RELEASE "${CMAKE_BINARY_DIR}/bin/Release"
        RUNTIME_OUTPUT_DIRECTORY_DEBUG "${CMAKE_BINARY_DIR}/bin/Debug"
    )
    if(UNIX AND STASIS_MOBILE_RUNTIME_BUILD_TESTS)
        add_library(stasis_runner_package_graphics SHARED tests/stasis_runner_package_graphics.c)
        set_target_properties(stasis_runner_package_graphics PROPERTIES OUTPUT_NAME stasis_graphics)
        add_library(stasis_runner_package_game SHARED tests/stasis_runner_package_game.c)
        target_link_libraries(stasis_runner_package_game PRIVATE m)
        set_target_properties(stasis_runner_package_game PROPERTIES PREFIX "")
        add_test(
            NAME stasis_runner_packaged_unix_contract
            COMMAND ${CMAKE_COMMAND}
                -DTEST_ROOT=${CMAKE_CURRENT_BINARY_DIR}/runner-package-test
                -DRUNNER=$<TARGET_FILE:stasis_runner>
                -DGRAPHICS=$<TARGET_FILE:stasis_runner_package_graphics>
                -DGAME=$<TARGET_FILE:stasis_runner_package_game>
                -P ${CMAKE_CURRENT_SOURCE_DIR}/tests/stasis_runner_package_test.cmake
        )
    endif()
endif()
