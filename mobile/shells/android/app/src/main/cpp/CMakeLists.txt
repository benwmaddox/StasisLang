cmake_minimum_required(VERSION 3.22.1)
project(stasis_mobile_android C CXX)

option(STASIS_ENABLE_SEAM_TESTS "Enable generated release-shell seam hooks" OFF)
set(STASIS_NETWORK_ENABLED @STASIS_NETWORK_ENABLED@)
set(STASIS_NETWORK_CLIENT_ENABLED @STASIS_NETWORK_CLIENT_ENABLED@)
set(STASIS_ANDROID_RUNTIME_MODE "@STASIS_ANDROID_RUNTIME_MODE@")
set(STASIS_ANDROID_RUNTIME_VARIANT "@STASIS_ANDROID_RUNTIME_VARIANT@")
if(NOT STASIS_ANDROID_RUNTIME_MODE STREQUAL "source" AND
   NOT STASIS_ANDROID_RUNTIME_MODE STREQUAL "prebuilt")
    message(FATAL_ERROR "Android runtime mode must be source or prebuilt")
endif()

set(STASIS_ROOT "${CMAKE_CURRENT_LIST_DIR}/../../../../..")
set(STASIS_AOT_DIR "${STASIS_ROOT}/aot")
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

if(STASIS_NETWORK_ENABLED OR STASIS_NETWORK_CLIENT_ENABLED)
    set(STASIS_NETWORK_LIBRARY "${CMAKE_CURRENT_LIST_DIR}/network/libstasis_network.so")
    if(NOT EXISTS "${STASIS_NETWORK_LIBRARY}")
        message(FATAL_ERROR "Network capability enabled but ${STASIS_NETWORK_LIBRARY} is missing")
    endif()
    add_library(stasis_network SHARED IMPORTED)
    set_target_properties(stasis_network PROPERTIES
        IMPORTED_LOCATION "${STASIS_NETWORK_LIBRARY}"
        INTERFACE_INCLUDE_DIRECTORIES "${CMAKE_CURRENT_LIST_DIR}/network/include")
endif()

if(STASIS_ANDROID_RUNTIME_MODE STREQUAL "source")
    if(NOT STASIS_SDL3_SOURCE OR NOT STASIS_SDL3_IMAGE_SOURCE)
        message(FATAL_ERROR
            "Source-build development packages require STASIS_SDL3_SOURCE and STASIS_SDL3_IMAGE_SOURCE")
    endif()
    set(STASIS_BUILD_MOBILE_RUNTIME ON CACHE BOOL "" FORCE)
    set(STASIS_GRAPHICS_BUILD_SHARED OFF CACHE BOOL "" FORCE)
    set(STASIS_GRAPHICS_BUILD_STATIC OFF CACHE BOOL "" FORCE)
    set(STASIS_BUILD_RUNNER OFF CACHE BOOL "" FORCE)
    set(STASIS_BUILD_SYS OFF CACHE BOOL "" FORCE)
    set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
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
    set(SDL_TEST_LIBRARY OFF CACHE BOOL "" FORCE)
    set(SDL_TESTS OFF CACHE BOOL "" FORCE)
    set(SDL_INSTALL OFF CACHE BOOL "" FORCE)
    add_subdirectory("${STASIS_SDL3_SOURCE}" "${CMAKE_BINARY_DIR}/sdl3")
    set(SDLIMAGE_SAMPLES OFF CACHE BOOL "" FORCE)
    set(SDLIMAGE_TESTS OFF CACHE BOOL "" FORCE)
    set(SDLIMAGE_VENDORED OFF CACHE BOOL "" FORCE)
    set(SDLIMAGE_BACKEND_STB ON CACHE BOOL "" FORCE)
    set(SDLIMAGE_ANI OFF CACHE BOOL "" FORCE)
    set(SDLIMAGE_AVIF OFF CACHE BOOL "" FORCE)
    set(SDLIMAGE_BMP OFF CACHE BOOL "" FORCE)
    set(SDLIMAGE_GIF OFF CACHE BOOL "" FORCE)
    set(SDLIMAGE_JPG OFF CACHE BOOL "" FORCE)
    set(SDLIMAGE_JXL OFF CACHE BOOL "" FORCE)
    set(SDLIMAGE_LBM OFF CACHE BOOL "" FORCE)
    set(SDLIMAGE_PCX OFF CACHE BOOL "" FORCE)
    set(SDLIMAGE_PNG ON CACHE BOOL "" FORCE)
    set(SDLIMAGE_PNG_LIBPNG OFF CACHE BOOL "" FORCE)
    set(SDLIMAGE_PNM OFF CACHE BOOL "" FORCE)
    set(SDLIMAGE_QOI OFF CACHE BOOL "" FORCE)
    set(SDLIMAGE_SVG OFF CACHE BOOL "" FORCE)
    set(SDLIMAGE_TGA OFF CACHE BOOL "" FORCE)
    set(SDLIMAGE_TIF OFF CACHE BOOL "" FORCE)
    set(SDLIMAGE_WEBP OFF CACHE BOOL "" FORCE)
    set(SDLIMAGE_XCF OFF CACHE BOOL "" FORCE)
    set(SDLIMAGE_XPM OFF CACHE BOOL "" FORCE)
    set(SDLIMAGE_XV OFF CACHE BOOL "" FORCE)
    add_subdirectory("${STASIS_SDL3_IMAGE_SOURCE}" "${CMAKE_BINARY_DIR}/sdl3_image")
    add_subdirectory("${STASIS_ROOT}/runtime" "${CMAKE_BINARY_DIR}/stasis_runtime")
    if(STASIS_NETWORK_ENABLED OR STASIS_NETWORK_CLIENT_ENABLED)
        target_link_libraries(stasis_mobile_runtime PUBLIC stasis_network)
        if(STASIS_NETWORK_ENABLED)
            target_compile_definitions(stasis_mobile_runtime PUBLIC STASIS_NETWORK_ENABLED=1)
        endif()
        if(STASIS_NETWORK_CLIENT_ENABLED)
            target_compile_definitions(stasis_mobile_runtime PUBLIC STASIS_NETWORK_CLIENT_ENABLED=1)
        endif()
    endif()
    set(STASIS_MOBILE_RUNTIME_TARGET stasis_mobile_runtime)
    set(STASIS_SDL3_TARGET SDL3::SDL3-static)
    set(STASIS_SDL3_IMAGE_TARGET SDL3_image::SDL3_image-static)
    set(STASIS_THORVG_TARGET stasis_thorvg)
else()
    set(STASIS_PREBUILT_RUNTIME "${CMAKE_CURRENT_LIST_DIR}/../../../../runtime")
    set(STASIS_PREBUILT_MANIFEST "${STASIS_PREBUILT_RUNTIME}/manifest.json")
    if(NOT EXISTS "${STASIS_PREBUILT_MANIFEST}")
        message(FATAL_ERROR "Prebuilt Android runtime manifest is missing: ${STASIS_PREBUILT_MANIFEST}")
    endif()
    file(READ "${STASIS_PREBUILT_MANIFEST}" STASIS_PREBUILT_JSON)
    foreach(field IN ITEMS schema abi ndk_version)
        string(JSON STASIS_PREBUILT_${field} ERROR_VARIABLE json_error
            GET "${STASIS_PREBUILT_JSON}" "${field}")
        if(json_error)
            message(FATAL_ERROR "Prebuilt Android runtime manifest is missing ${field}: ${json_error}")
        endif()
    endforeach()
    if(NOT STASIS_PREBUILT_schema STREQUAL "stasis.android_runtime.v1" OR
       NOT STASIS_PREBUILT_abi STREQUAL "${ANDROID_ABI}" OR
       NOT STASIS_PREBUILT_ndk_version STREQUAL "27.0.12077973")
        message(FATAL_ERROR "Prebuilt Android runtime schema, ABI, or NDK identity mismatch")
    endif()
    if(DEFINED CMAKE_ANDROID_NDK_VERSION AND
       NOT CMAKE_ANDROID_NDK_VERSION STREQUAL STASIS_PREBUILT_ndk_version)
        message(FATAL_ERROR
            "Configured NDK ${CMAKE_ANDROID_NDK_VERSION} does not match prebuilt runtime ${STASIS_PREBUILT_ndk_version}")
    endif()
    function(stasis_import_prebuilt target manifest_group manifest_name)
        string(JSON library_path ERROR_VARIABLE path_error GET
            "${STASIS_PREBUILT_JSON}" "${manifest_group}" "${manifest_name}")
        string(JSON expected_sha ERROR_VARIABLE hash_error GET
            "${STASIS_PREBUILT_JSON}" "files" "${library_path}")
        if(path_error OR hash_error)
            message(FATAL_ERROR "Prebuilt Android runtime manifest is missing ${manifest_group}.${manifest_name}")
        endif()
        set(full_path "${STASIS_PREBUILT_RUNTIME}/${library_path}")
        if(NOT EXISTS "${full_path}")
            message(FATAL_ERROR "Prebuilt Android runtime library is missing: ${full_path}")
        endif()
        file(SHA256 "${full_path}" actual_sha)
        if(NOT actual_sha STREQUAL expected_sha)
            message(FATAL_ERROR "Prebuilt Android runtime hash mismatch: ${library_path}")
        endif()
        add_library(${target} STATIC IMPORTED)
        set_target_properties(${target} PROPERTIES IMPORTED_LOCATION "${full_path}")
    endfunction()
    stasis_import_prebuilt(stasis_mobile_runtime_prebuilt runtime_variants
        "${STASIS_ANDROID_RUNTIME_VARIANT}")
    stasis_import_prebuilt(stasis_sdl3_prebuilt libraries sdl3)
    stasis_import_prebuilt(stasis_sdl3_image_prebuilt libraries sdl3_image)
    stasis_import_prebuilt(stasis_thorvg_prebuilt libraries thorvg)
    set_target_properties(stasis_mobile_runtime_prebuilt PROPERTIES
        INTERFACE_INCLUDE_DIRECTORIES "${STASIS_PREBUILT_RUNTIME}/include/stasis")
    set_target_properties(stasis_sdl3_prebuilt stasis_sdl3_image_prebuilt PROPERTIES
        INTERFACE_INCLUDE_DIRECTORIES "${STASIS_PREBUILT_RUNTIME}/include")
    # SDL's source target carries these Android system libraries transitively.
    # Imported static archives need the same closure declared explicitly.
    set_target_properties(stasis_sdl3_prebuilt PROPERTIES
        INTERFACE_LINK_LIBRARIES "m;dl;OpenSLES;log;android;GLESv1_CM;GLESv2")
    set(STASIS_MOBILE_RUNTIME_TARGET stasis_mobile_runtime_prebuilt)
    set(STASIS_SDL3_TARGET stasis_sdl3_prebuilt)
    set(STASIS_SDL3_IMAGE_TARGET stasis_sdl3_image_prebuilt)
    set(STASIS_THORVG_TARGET stasis_thorvg_prebuilt)
endif()

include("${STASIS_AOT_DIR}/published_aot_objects.cmake")
if(NOT STASIS_PUBLISHED_AOT_OBJECTS)
    message(FATAL_ERROR "No published Android AOT objects found in ${STASIS_AOT_DIR}")
endif()

add_library(main SHARED
    "${STASIS_ROOT}/common/stasis_mobile_main.c"
    "${CMAKE_CURRENT_LIST_DIR}/stasis_android_assets.c"
    "${CMAKE_CURRENT_LIST_DIR}/stasis_android_external_url.c"
    "${CMAKE_CURRENT_LIST_DIR}/stasis_android_network.c"
    "${STASIS_AOT_DIR}/published_aot_bindings.c"
    ${STASIS_PUBLISHED_AOT_OBJECTS}
)
set_property(TARGET main PROPERTY LINKER_LANGUAGE CXX)
if(STASIS_ENABLE_SEAM_TESTS)
    target_compile_definitions(main PRIVATE STASIS_ENABLE_SEAM_TESTS=1)
endif()
target_include_directories(main PRIVATE "${STASIS_AOT_DIR}" "${STASIS_ROOT}/runtime")
target_link_libraries(main PRIVATE
    ${STASIS_MOBILE_RUNTIME_TARGET}
    ${STASIS_SDL3_IMAGE_TARGET}
    ${STASIS_SDL3_TARGET}
    ${STASIS_THORVG_TARGET}
    log
    android)
target_compile_options(main PRIVATE -Oz -ffunction-sections -fdata-sections -flto)
target_link_options(main PRIVATE
    -flto
    -Wl,--gc-sections
    -Wl,--icf=safe
    "-Wl,-Map,${CMAKE_CURRENT_BINARY_DIR}/libmain.map")
if(STASIS_NETWORK_ENABLED OR STASIS_NETWORK_CLIENT_ENABLED)
    target_link_libraries(main PRIVATE stasis_network)
    if(STASIS_NETWORK_ENABLED)
        target_compile_definitions(main PRIVATE STASIS_NETWORK_ENABLED=1)
    endif()
    if(STASIS_NETWORK_CLIENT_ENABLED)
        target_compile_definitions(main PRIVATE STASIS_NETWORK_CLIENT_ENABLED=1)
    endif()
endif()
