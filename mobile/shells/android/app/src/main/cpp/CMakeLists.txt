cmake_minimum_required(VERSION 3.22.1)
project(stasis_mobile_android C)

option(STASIS_ENABLE_SEAM_TESTS "Enable generated release-shell seam hooks" OFF)
set(STASIS_NETWORK_ENABLED @STASIS_NETWORK_ENABLED@)

if(NOT STASIS_SDL3_SOURCE OR NOT STASIS_SDL3_IMAGE_SOURCE)
    message(FATAL_ERROR "STASIS_SDL3_SOURCE and STASIS_SDL3_IMAGE_SOURCE are required")
endif()

set(STASIS_ROOT "${CMAKE_CURRENT_LIST_DIR}/../../../../..")
set(STASIS_AOT_DIR "${STASIS_ROOT}/aot")
set(STASIS_BUILD_MOBILE_RUNTIME ON CACHE BOOL "" FORCE)
set(STASIS_GRAPHICS_BUILD_SHARED OFF CACHE BOOL "" FORCE)
set(STASIS_GRAPHICS_BUILD_STATIC OFF CACHE BOOL "" FORCE)
set(STASIS_BUILD_RUNNER OFF CACHE BOOL "" FORCE)
set(STASIS_BUILD_SYS OFF CACHE BOOL "" FORCE)

if(STASIS_NETWORK_ENABLED)
    set(STASIS_NETWORK_LIBRARY "${CMAKE_CURRENT_LIST_DIR}/network/libstasis_network.a")
    if(NOT EXISTS "${STASIS_NETWORK_LIBRARY}")
        message(FATAL_ERROR "Network capability enabled but ${STASIS_NETWORK_LIBRARY} is missing")
    endif()
    add_library(stasis_network STATIC IMPORTED)
    set_target_properties(stasis_network PROPERTIES
        IMPORTED_LOCATION "${STASIS_NETWORK_LIBRARY}"
        INTERFACE_INCLUDE_DIRECTORIES "${CMAKE_CURRENT_LIST_DIR}/network/include")
endif()

add_subdirectory("${STASIS_SDL3_SOURCE}" "${CMAKE_BINARY_DIR}/sdl3")
set(SDLIMAGE_SAMPLES OFF CACHE BOOL "" FORCE)
set(SDLIMAGE_TESTS OFF CACHE BOOL "" FORCE)
set(SDLIMAGE_VENDORED OFF CACHE BOOL "" FORCE)
set(SDLIMAGE_BACKEND_STB ON CACHE BOOL "" FORCE)
set(SDLIMAGE_PNG ON CACHE BOOL "" FORCE)
set(SDLIMAGE_PNG_LIBPNG OFF CACHE BOOL "" FORCE)
add_subdirectory("${STASIS_SDL3_IMAGE_SOURCE}" "${CMAKE_BINARY_DIR}/sdl3_image")
add_subdirectory("${STASIS_ROOT}/runtime" "${CMAKE_BINARY_DIR}/stasis_runtime")
if(STASIS_NETWORK_ENABLED)
    target_compile_definitions(stasis_mobile_runtime PUBLIC STASIS_NETWORK_ENABLED=1)
    target_link_libraries(stasis_mobile_runtime PUBLIC stasis_network)
endif()

include("${STASIS_AOT_DIR}/published_aot_objects.cmake")
if(NOT STASIS_PUBLISHED_AOT_OBJECTS)
    message(FATAL_ERROR "No published Android AOT objects found in ${STASIS_AOT_DIR}")
endif()

add_library(main SHARED
    "${STASIS_ROOT}/common/stasis_mobile_main.c"
    "${CMAKE_CURRENT_LIST_DIR}/stasis_android_assets.c"
    "${CMAKE_CURRENT_LIST_DIR}/stasis_android_network.c"
    "${STASIS_AOT_DIR}/published_aot_bindings.c"
    ${STASIS_PUBLISHED_AOT_OBJECTS}
)
if(STASIS_ENABLE_SEAM_TESTS)
    target_compile_definitions(main PRIVATE STASIS_ENABLE_SEAM_TESTS=1)
endif()
target_include_directories(main PRIVATE "${STASIS_AOT_DIR}" "${STASIS_ROOT}/runtime")
target_link_libraries(main PRIVATE stasis_mobile_runtime SDL3::SDL3 SDL3_image::SDL3_image log android)
if(STASIS_NETWORK_ENABLED)
    target_link_libraries(main PRIVATE stasis_network)
    target_compile_definitions(main PRIVATE STASIS_NETWORK_ENABLED=1)
endif()
target_link_options(main PRIVATE "-Wl,-Map,${CMAKE_CURRENT_BINARY_DIR}/libmain.map")
