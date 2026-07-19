cmake_minimum_required(VERSION 3.16)
project(stasis_graphics C)

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
    "Build SDL_Renderer-only runtime (no OpenGL/GLEW backend)"
    ${STASIS_MOBILE_PLATFORM}
)

# Prefer static CRT when possible (for single-exe bundling)
if (MSVC)
    set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")
endif()

# Find graphics deps only when building graphics targets.
if(STASIS_GRAPHICS_BUILD_SHARED OR STASIS_GRAPHICS_BUILD_STATIC OR STASIS_BUILD_MOBILE_RUNTIME)
    # Find SDL2 (via vcpkg or system)
    if(NOT TARGET SDL2::SDL2 AND NOT TARGET SDL2::SDL2-static)
        find_package(SDL2 CONFIG REQUIRED)
    endif()
    if(NOT TARGET SDL2_image::SDL2_image AND NOT TARGET SDL2_image::SDL2_image-static)
        find_package(SDL2_image CONFIG REQUIRED)
    endif()

    if((STASIS_GRAPHICS_BUILD_SHARED OR STASIS_GRAPHICS_BUILD_STATIC) AND
       NOT STASIS_GRAPHICS_SDL_ONLY)
        # Find OpenGL and GLEW (desktop backend)
        find_package(OpenGL REQUIRED)
        find_package(GLEW REQUIRED)
    endif()
endif()

function(configure_stasis_target target is_static sdl_only link_sdl_main)
    target_sources(${target} PRIVATE stasis_graphics.c)
    if(link_sdl_main)
        target_link_libraries(${target} PRIVATE $<TARGET_NAME_IF_EXISTS:SDL2::SDL2main>)
    endif()
    target_link_libraries(${target} PRIVATE
        $<IF:$<TARGET_EXISTS:SDL2::SDL2>,SDL2::SDL2,SDL2::SDL2-static>
        $<IF:$<TARGET_EXISTS:SDL2_image::SDL2_image>,SDL2_image::SDL2_image,SDL2_image::SDL2_image-static>
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
    configure_stasis_target(stasis_graphics OFF ${STASIS_GRAPHICS_SDL_ONLY} ON)

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
    configure_stasis_target(stasis_graphics_static ON ${STASIS_GRAPHICS_SDL_ONLY} ON)
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
    configure_stasis_target(stasis_mobile_runtime ON TRUE OFF)
    target_include_directories(stasis_mobile_runtime PUBLIC "${CMAKE_CURRENT_SOURCE_DIR}")
    set_target_properties(stasis_mobile_runtime PROPERTIES
        POSITION_INDEPENDENT_CODE ON
        ARCHIVE_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/mobile"
    )
    install(TARGETS stasis_mobile_runtime ARCHIVE DESTINATION lib)
    install(
        FILES stasis_mobile_runtime.h stasis_mobile_aot_runtime.h
        DESTINATION include
    )
endif()

if(STASIS_MOBILE_RUNTIME_BUILD_TESTS)
    enable_testing()
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
        target_compile_definitions(stasis_runner PRIVATE _CRT_SECURE_NO_WARNINGS)
    else()
        target_link_libraries(stasis_runner PRIVATE dl)
    endif()
    set_target_properties(stasis_runner PROPERTIES
        RUNTIME_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/bin"
        RUNTIME_OUTPUT_DIRECTORY_RELEASE "${CMAKE_BINARY_DIR}/bin/Release"
        RUNTIME_OUTPUT_DIRECTORY_DEBUG "${CMAKE_BINARY_DIR}/bin/Debug"
    )
endif()
