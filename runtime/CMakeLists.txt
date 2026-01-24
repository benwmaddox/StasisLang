cmake_minimum_required(VERSION 3.16)
project(stasis_graphics C)

option(STASIS_GRAPHICS_BUILD_SHARED "Build the graphics runtime as a DLL" ON)
option(STASIS_GRAPHICS_BUILD_STATIC "Build the graphics runtime as a static library" ON)
option(STASIS_BUILD_RUNNER "Build the stasis DLL runner executable" ON)
option(STASIS_BUILD_SYS "Build the sys runtime (file/argv/exec)" ON)

# Android and other restricted platforms do not support the OpenGL 2.1 + GLEW path.
set(STASIS_GRAPHICS_SDL_ONLY_DEFAULT OFF)
if(ANDROID)
    set(STASIS_GRAPHICS_SDL_ONLY_DEFAULT ON)
endif()
option(STASIS_GRAPHICS_SDL_ONLY "Build SDL_Renderer-only runtime (no OpenGL/GLEW backend)" ${STASIS_GRAPHICS_SDL_ONLY_DEFAULT})

# Prefer static CRT when possible (for single-exe bundling)
if (MSVC)
    set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")
endif()

# Find graphics deps only when building graphics targets.
if(STASIS_GRAPHICS_BUILD_SHARED OR STASIS_GRAPHICS_BUILD_STATIC)
    # Find SDL2 (via vcpkg or system)
    find_package(SDL2 CONFIG REQUIRED)
    find_package(SDL2_image CONFIG REQUIRED)

    if(NOT STASIS_GRAPHICS_SDL_ONLY)
        # Find OpenGL and GLEW (desktop backend)
        find_package(OpenGL REQUIRED)
        find_package(GLEW REQUIRED)
    endif()
endif()

function(configure_stasis_target target is_static)
    target_sources(${target} PRIVATE stasis_graphics.c)
    target_link_libraries(${target} PRIVATE
        $<TARGET_NAME_IF_EXISTS:SDL2::SDL2main>
        $<IF:$<TARGET_EXISTS:SDL2::SDL2>,SDL2::SDL2,SDL2::SDL2-static>
        $<IF:$<TARGET_EXISTS:SDL2_image::SDL2_image>,SDL2_image::SDL2_image,SDL2_image::SDL2_image-static>
    )

    if(NOT STASIS_GRAPHICS_SDL_ONLY)
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
        if(NOT STASIS_GRAPHICS_SDL_ONLY)
            target_compile_definitions(${target} PRIVATE STASIS_GRAPHICS_STATIC GLEW_STATIC)
        else()
            target_compile_definitions(${target} PRIVATE STASIS_GRAPHICS_STATIC)
        endif()
    endif()
endfunction()

if(STASIS_GRAPHICS_BUILD_SHARED)
    add_library(stasis_graphics SHARED)
    configure_stasis_target(stasis_graphics OFF)

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
    configure_stasis_target(stasis_graphics_static ON)
    set_target_properties(stasis_graphics_static PROPERTIES
        ARCHIVE_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/Release"
        ARCHIVE_OUTPUT_DIRECTORY_RELEASE "${CMAKE_BINARY_DIR}/Release"
        ARCHIVE_OUTPUT_DIRECTORY_DEBUG "${CMAKE_BINARY_DIR}/Debug"
    )
    install(TARGETS stasis_graphics_static
        ARCHIVE DESTINATION lib
    )
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
