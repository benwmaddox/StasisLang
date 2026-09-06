cmake_minimum_required(VERSION 3.16)
project(stasis_graphics C CXX)

# Prefer static CRT when possible (for single-exe bundling).
if(MSVC)
    set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")
endif()

set(STASIS_RELEASE_ID "development" CACHE STRING "Immutable Stasis toolchain release identity")
set(STASIS_BUILD_FINGERPRINT "" CACHE STRING "Exact source/toolchain fingerprint shared with the CLI")

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
option(STASIS_BUILD_MONOLITH "Build one direct-bound production AOT executable" OFF)
set(STASIS_MONOLITH_AOT_DIR "" CACHE PATH "Generated direct-bound AOT bundle")
set(STASIS_MONOLITH_MAIN_SOURCE "" CACHE FILEPATH "Generated production host source")
set(STASIS_MONOLITH_OUTPUT_DIR "" CACHE PATH "Production executable output directory")
set(STASIS_MONOLITH_OUTPUT_NAME "stasis_game" CACHE STRING "Production executable file name")
set(STASIS_MONOLITH_NETWORK_LIBRARY "" CACHE FILEPATH "stasis_network static library")
set(STASIS_MONOLITH_NETWORK_INCLUDE_DIR "" CACHE PATH "stasis_network header directory")
option(
    STASIS_BUILD_MOBILE_RUNTIME
    "Build the shared SDL renderer mobile runtime core"
    ${STASIS_MOBILE_PLATFORM}
)
option(STASIS_MOBILE_RUNTIME_BUILD_TESTS "Build mobile runtime lifecycle tests" OFF)
option(STASIS_SVG_BUILD_TESTS "Build the ThorVG SVG bridge smoke tests" OFF)
option(STASIS_GRAPHICS_BUILD_TESTS "Build native renderer contract tests" OFF)

if(STASIS_BUILD_MONOLITH)
    set(STASIS_GRAPHICS_BUNDLE_SDL ON CACHE BOOL "" FORCE)
    set(STASIS_GRAPHICS_BUILD_SHARED OFF CACHE BOOL "" FORCE)
    set(STASIS_GRAPHICS_BUILD_STATIC OFF CACHE BOOL "" FORCE)
    set(STASIS_BUILD_RUNNER OFF CACHE BOOL "" FORCE)
    set(STASIS_BUILD_SYS OFF CACHE BOOL "" FORCE)
    set(STASIS_BUILD_MOBILE_RUNTIME ON CACHE BOOL "" FORCE)
endif()

if(STASIS_GRAPHICS_BUILD_SHARED OR STASIS_GRAPHICS_BUILD_STATIC OR
   STASIS_BUILD_MOBILE_RUNTIME OR STASIS_SVG_BUILD_TESTS)
    add_subdirectory(third_party/thorvg)
endif()

if(STASIS_SVG_BUILD_TESTS)
    enable_testing()
    add_executable(stasis_svg_smoke tests/stasis_svg_smoke.cpp stasis_svg.cpp)
    target_link_libraries(stasis_svg_smoke PRIVATE stasis_thorvg)
    add_test(
        NAME stasis_svg_smoke
        COMMAND stasis_svg_smoke "${CMAKE_CURRENT_LIST_DIR}/tests/fixtures/viewport-clip.svg"
    )
endif()

# Find graphics deps only when building graphics targets.
if(STASIS_GRAPHICS_BUILD_SHARED OR STASIS_GRAPHICS_BUILD_STATIC OR STASIS_BUILD_MOBILE_RUNTIME)
    if(STASIS_GRAPHICS_BUNDLE_SDL)
        include(FetchContent)
        set(CMAKE_POSITION_INDEPENDENT_CODE ON)
        set(SDL_SHARED OFF CACHE BOOL "" FORCE)
        set(SDL_STATIC ON CACHE BOOL "" FORCE)
        set(SDL_GPU OFF CACHE BOOL "" FORCE)
        set(SDL_CAMERA OFF CACHE BOOL "" FORCE)
        set(SDL_JOYSTICK OFF CACHE BOOL "" FORCE)
        set(SDL_HAPTIC OFF CACHE BOOL "" FORCE)
        set(SDL_HIDAPI OFF CACHE BOOL "" FORCE)
        set(SDL_POWER OFF CACHE BOOL "" FORCE)
        set(SDL_SENSOR OFF CACHE BOOL "" FORCE)
        set(SDL_DIALOG OFF CACHE BOOL "" FORCE)
        set(SDL_TRAY OFF CACHE BOOL "" FORCE)
        set(SDL_VULKAN OFF CACHE BOOL "" FORCE)
        set(SDL_RENDER_VULKAN OFF CACHE BOOL "" FORCE)
        set(SDL_RENDER_GPU OFF CACHE BOOL "" FORCE)
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
        set(SDLIMAGE_ANI OFF CACHE BOOL "" FORCE)
        set(SDLIMAGE_AVIF OFF CACHE BOOL "" FORCE)
        set(SDLIMAGE_BMP OFF CACHE BOOL "" FORCE)
        set(SDLIMAGE_GIF OFF CACHE BOOL "" FORCE)
        set(SDLIMAGE_JPG OFF CACHE BOOL "" FORCE)
        set(SDLIMAGE_JXL OFF CACHE BOOL "" FORCE)
        set(SDLIMAGE_LBM OFF CACHE BOOL "" FORCE)
        set(SDLIMAGE_PCX OFF CACHE BOOL "" FORCE)
        set(SDLIMAGE_PNG ON CACHE BOOL "" FORCE)
        set(SDLIMAGE_PNM OFF CACHE BOOL "" FORCE)
        set(SDLIMAGE_QOI OFF CACHE BOOL "" FORCE)
        set(SDLIMAGE_SVG OFF CACHE BOOL "" FORCE)
        set(SDLIMAGE_TGA OFF CACHE BOOL "" FORCE)
        set(SDLIMAGE_TIF OFF CACHE BOOL "" FORCE)
        set(SDLIMAGE_WEBP OFF CACHE BOOL "" FORCE)
        set(SDLIMAGE_XCF OFF CACHE BOOL "" FORCE)
        set(SDLIMAGE_XPM OFF CACHE BOOL "" FORCE)
        set(SDLIMAGE_XV OFF CACHE BOOL "" FORCE)
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

endif()

if(STASIS_GRAPHICS_BUILD_TESTS)
    enable_testing()
    add_executable(stasis_mixed_quad_planner_test tests/stasis_mixed_quad_planner_test.c)
    add_test(NAME stasis_mixed_quad_planner_test COMMAND stasis_mixed_quad_planner_test)
endif()

function(configure_stasis_target target is_static)
    target_sources(${target} PRIVATE stasis_graphics.c stasis_image_writer.c stasis_audio_assets.c stasis_svg.cpp)
    target_compile_definitions(${target} PRIVATE STASIS_RELEASE_ID="${STASIS_RELEASE_ID}")
    target_compile_definitions(${target} PRIVATE STASIS_BUILD_FINGERPRINT="${STASIS_BUILD_FINGERPRINT}")
    target_link_libraries(${target} PRIVATE
        stasis_thorvg
        $<IF:$<TARGET_EXISTS:SDL3::SDL3>,SDL3::SDL3,SDL3::SDL3-static>
        $<IF:$<TARGET_EXISTS:SDL3_image::SDL3_image>,SDL3_image::SDL3_image,SDL3_image::SDL3_image-static>
    )

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
        target_compile_definitions(${target} PRIVATE STASIS_GRAPHICS_STATIC)
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

if(STASIS_BUILD_MOBILE_RUNTIME)
    add_library(
        stasis_mobile_runtime STATIC
        stasis_mobile_runtime.c
        stasis_mobile_aot_runtime.c
        stasis_platform_services.c
    )
    configure_stasis_target(stasis_mobile_runtime ON)
    set_property(TARGET stasis_mobile_runtime PROPERTY C_STANDARD 11)
    set_property(TARGET stasis_mobile_runtime PROPERTY C_STANDARD_REQUIRED ON)
    if(MSVC)
        target_compile_options(stasis_mobile_runtime PRIVATE /experimental:c11atomics)
    endif()
    target_include_directories(stasis_mobile_runtime PUBLIC "${CMAKE_CURRENT_SOURCE_DIR}")
    set_target_properties(stasis_mobile_runtime PROPERTIES
        POSITION_INDEPENDENT_CODE ON
        ARCHIVE_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/mobile"
    )
    install(TARGETS stasis_mobile_runtime ARCHIVE DESTINATION lib)
    install(
        FILES stasis_asset_path.h stasis_mobile_runtime.h stasis_mobile_aot_runtime.h
              stasis_network_join_card.h stasis_performance_metrics.h stasis_platform_services.h
        DESTINATION include
    )
endif()

if(STASIS_BUILD_MONOLITH)
    if(NOT WIN32)
        message(FATAL_ERROR "The desktop production monolith currently targets Windows")
    endif()
    if(NOT STASIS_MONOLITH_AOT_DIR OR NOT EXISTS "${STASIS_MONOLITH_AOT_DIR}/published_aot_objects.cmake")
        message(FATAL_ERROR "STASIS_MONOLITH_AOT_DIR must contain published_aot_objects.cmake")
    endif()
    if(NOT STASIS_MONOLITH_MAIN_SOURCE OR NOT EXISTS "${STASIS_MONOLITH_MAIN_SOURCE}")
        message(FATAL_ERROR "STASIS_MONOLITH_MAIN_SOURCE must name the generated production host")
    endif()
    include("${STASIS_MONOLITH_AOT_DIR}/published_aot_objects.cmake")
    add_executable(
        stasis_monolith
        "${STASIS_MONOLITH_MAIN_SOURCE}"
        "${STASIS_MONOLITH_AOT_DIR}/published_aot_bindings.c"
        ${STASIS_PUBLISHED_AOT_OBJECTS}
        stasis_runner.manifest
    )
    target_compile_definitions(stasis_monolith PRIVATE STASIS_WINDOWS_MONOLITH=1)
    target_include_directories(
        stasis_monolith
        PRIVATE "${STASIS_MONOLITH_AOT_DIR}" "${CMAKE_CURRENT_SOURCE_DIR}")
    target_link_libraries(
        stasis_monolith
        PRIVATE
            stasis_mobile_runtime
            SDL3::SDL3-static
            SDL3_image::SDL3_image-static
    )
    if(STASIS_MONOLITH_NETWORK_LIBRARY OR STASIS_MONOLITH_NETWORK_INCLUDE_DIR)
        if(NOT STASIS_MONOLITH_NETWORK_LIBRARY OR NOT EXISTS "${STASIS_MONOLITH_NETWORK_LIBRARY}")
            message(FATAL_ERROR "STASIS_MONOLITH_NETWORK_LIBRARY must name the built stasis_network library")
        endif()
        if(NOT STASIS_MONOLITH_NETWORK_INCLUDE_DIR OR
           NOT EXISTS "${STASIS_MONOLITH_NETWORK_INCLUDE_DIR}/stasis_network.h")
            message(FATAL_ERROR "STASIS_MONOLITH_NETWORK_INCLUDE_DIR must contain stasis_network.h")
        endif()
        target_compile_definitions(stasis_mobile_runtime PRIVATE STASIS_NETWORK_ENABLED=1)
        target_compile_definitions(stasis_monolith PRIVATE STASIS_NETWORK_ENABLED=1)
        target_include_directories(stasis_mobile_runtime PRIVATE "${STASIS_MONOLITH_NETWORK_INCLUDE_DIR}")
        target_include_directories(stasis_monolith PRIVATE "${STASIS_MONOLITH_NETWORK_INCLUDE_DIR}")
        target_link_libraries(stasis_monolith PRIVATE "${STASIS_MONOLITH_NETWORK_LIBRARY}")
        if(WIN32)
            target_link_libraries(stasis_monolith PRIVATE ws2_32 bcrypt userenv ntdll)
        endif()
    endif()
    set_property(TARGET stasis_monolith PROPERTY INTERPROCEDURAL_OPTIMIZATION_RELEASE TRUE)
    target_compile_options(stasis_monolith PRIVATE /O1 /Gy /Gw)
    target_link_options(stasis_monolith PRIVATE /OPT:REF /OPT:ICF)
    set_target_properties(stasis_monolith PROPERTIES
        OUTPUT_NAME "${STASIS_MONOLITH_OUTPUT_NAME}"
        RUNTIME_OUTPUT_DIRECTORY "${STASIS_MONOLITH_OUTPUT_DIR}"
        RUNTIME_OUTPUT_DIRECTORY_RELEASE "${STASIS_MONOLITH_OUTPUT_DIR}"
    )
endif()

if(STASIS_MOBILE_RUNTIME_BUILD_TESTS)
    enable_testing()
    add_executable(
        stasis_image_writer_test
        tests/stasis_image_writer_test.c
        stasis_image_writer.c
    )
    target_include_directories(stasis_image_writer_test PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}")
    if(WIN32)
        target_compile_definitions(stasis_image_writer_test PRIVATE _CRT_SECURE_NO_WARNINGS)
    endif()
    add_test(NAME stasis_image_writer_contract COMMAND stasis_image_writer_test)
    add_executable(
        stasis_audio_ring_test
        tests/stasis_audio_ring_test.c
        stasis_audio_ring.c
    )
    target_include_directories(stasis_audio_ring_test PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}")
    add_test(NAME stasis_audio_ring_contract COMMAND stasis_audio_ring_test)
    add_executable(
        stasis_audio_assets_test
        tests/stasis_audio_assets_test.c
        stasis_audio_assets.c
    )
    target_include_directories(stasis_audio_assets_test PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}")
    target_compile_definitions(
        stasis_audio_assets_test
        PRIVATE STASIS_TEST_MP3_PATH="${CMAKE_CURRENT_SOURCE_DIR}/../samples/audio_asset_playback/assets/tone.mp3"
    )
    if(NOT WIN32)
        target_link_libraries(stasis_audio_assets_test PRIVATE m)
    endif()
    add_test(NAME stasis_audio_assets_contract COMMAND stasis_audio_assets_test)
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
        stasis_sprite_atlas_policy_test
        tests/stasis_sprite_atlas_policy_test.c
    )
    target_include_directories(stasis_sprite_atlas_policy_test PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}")
    add_test(NAME stasis_sprite_atlas_policy_contract COMMAND stasis_sprite_atlas_policy_test)
    add_test(
        NAME stasis_sprite_atlas_v3_exports_contract
        COMMAND ${CMAKE_COMMAND}
            -DDEF_FILE=${CMAKE_CURRENT_SOURCE_DIR}/stasis_graphics.def
            -P ${CMAKE_CURRENT_SOURCE_DIR}/tests/stasis_sprite_atlas_exports_test.cmake
    )
    add_executable(
        stasis_renderer_lifecycle_test
        tests/stasis_renderer_lifecycle_test.c
    )
    target_include_directories(stasis_renderer_lifecycle_test PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}")
    add_test(NAME stasis_renderer_lifecycle_contract COMMAND stasis_renderer_lifecycle_test)
    add_executable(
        stasis_performance_metrics_test
        tests/stasis_performance_metrics_test.c
    )
    target_include_directories(stasis_performance_metrics_test PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}")
    add_test(NAME stasis_performance_metrics_contract COMMAND stasis_performance_metrics_test)
    if(TARGET stasis_graphics_static)
        add_executable(
            stasis_asset_tasks_test
            tests/stasis_asset_tasks_test.c
        )
        target_link_libraries(stasis_asset_tasks_test PRIVATE stasis_graphics_static)
        target_compile_definitions(
            stasis_asset_tasks_test
            PRIVATE
                STASIS_TEST_SPRITE_PATH="${CMAKE_SOURCE_DIR}/../samples/sprite_sheet_animation/assets/sprite_sheet_2x2.png"
                STASIS_TEST_AUDIO_PATH="${CMAKE_SOURCE_DIR}/../samples/audio_asset_playback/assets/tone.mp3"
        )
        add_test(NAME stasis_asset_tasks_contract COMMAND stasis_asset_tasks_test)
        set_tests_properties(
            stasis_asset_tasks_contract
            PROPERTIES ENVIRONMENT "SDL_VIDEODRIVER=dummy;SDL_RENDER_DRIVER=software;SDL_AUDIODRIVER=dummy"
        )
        add_executable(
            stasis_recording_audio_test
            tests/stasis_recording_audio_test.c
        )
        target_link_libraries(stasis_recording_audio_test PRIVATE stasis_graphics_static)
        target_compile_definitions(
            stasis_recording_audio_test
            PRIVATE STASIS_TEST_AUDIO_PATH="${CMAKE_SOURCE_DIR}/../samples/audio_asset_playback/assets/tone.wav"
        )
        add_test(NAME stasis_recording_audio_contract COMMAND stasis_recording_audio_test)
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
        stasis_platform_services.c
    )
    set_property(TARGET stasis_mobile_runtime_test PROPERTY C_STANDARD 11)
    set_property(TARGET stasis_mobile_runtime_test PROPERTY C_STANDARD_REQUIRED ON)
    if(MSVC)
        target_compile_options(stasis_mobile_runtime_test PRIVATE /experimental:c11atomics)
    endif()
    target_include_directories(stasis_mobile_runtime_test PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}")
    add_test(NAME stasis_mobile_runtime_lifecycle COMMAND stasis_mobile_runtime_test)
    add_executable(
        stasis_mobile_runtime_network_test
        tests/stasis_mobile_runtime_test.c
        stasis_mobile_runtime.c
        stasis_mobile_aot_runtime.c
        stasis_platform_services.c
    )
    set_property(TARGET stasis_mobile_runtime_network_test PROPERTY C_STANDARD 11)
    set_property(TARGET stasis_mobile_runtime_network_test PROPERTY C_STANDARD_REQUIRED ON)
    target_compile_definitions(stasis_mobile_runtime_network_test PRIVATE STASIS_NETWORK_ENABLED=1)
    target_include_directories(stasis_mobile_runtime_network_test PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}")
    if(MSVC)
        target_compile_options(stasis_mobile_runtime_network_test PRIVATE /experimental:c11atomics)
    endif()
    if(NOT WIN32)
        target_link_libraries(stasis_mobile_runtime_network_test PRIVATE m)
    endif()
    add_test(NAME stasis_mobile_runtime_network_lifecycle COMMAND stasis_mobile_runtime_network_test)
    add_executable(
        stasis_network_join_card_test
        tests/stasis_network_join_card_test.c
    )
    target_include_directories(stasis_network_join_card_test PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}")
    add_test(NAME stasis_network_join_card_contract COMMAND stasis_network_join_card_test)
    add_executable(
        stasis_mobile_frame_pacer_test
        tests/stasis_mobile_frame_pacer_test.c
    )
    target_include_directories(stasis_mobile_frame_pacer_test PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}")
    add_test(NAME stasis_mobile_frame_pacer_contract COMMAND stasis_mobile_frame_pacer_test)
    add_executable(
        stasis_mobile_aot_runtime_test
        tests/stasis_mobile_aot_runtime_test.c
        stasis_mobile_aot_runtime.c
        stasis_platform_services.c
    )
    set_property(TARGET stasis_mobile_aot_runtime_test PROPERTY C_STANDARD 11)
    set_property(TARGET stasis_mobile_aot_runtime_test PROPERTY C_STANDARD_REQUIRED ON)
    if(MSVC)
        target_compile_options(stasis_mobile_aot_runtime_test PRIVATE /experimental:c11atomics)
    endif()
    target_include_directories(stasis_mobile_aot_runtime_test PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}")
    if(NOT WIN32)
        target_link_libraries(stasis_mobile_runtime_test PRIVATE m)
        target_link_libraries(stasis_mobile_aot_runtime_test PRIVATE m)
    endif()
    add_test(NAME stasis_mobile_aot_runtime_contract COMMAND stasis_mobile_aot_runtime_test)
    add_executable(
        stasis_platform_services_test
        tests/stasis_platform_services_test.c
        stasis_platform_services.c
    )
    target_include_directories(stasis_platform_services_test PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}")
    if(NOT WIN32)
        target_link_libraries(stasis_platform_services_test PRIVATE pthread)
    endif()
    add_test(NAME stasis_platform_services_contract COMMAND stasis_platform_services_test)
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
