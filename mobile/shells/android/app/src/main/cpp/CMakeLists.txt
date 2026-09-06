cmake_minimum_required(VERSION 3.22.1)
project(stasis_mobile_android C CXX)

option(STASIS_ENABLE_SEAM_TESTS "Enable generated release-shell seam hooks" OFF)
set(STASIS_NETWORK_ENABLED @STASIS_NETWORK_ENABLED@)
set(STASIS_NETWORK_CLIENT_ENABLED @STASIS_NETWORK_CLIENT_ENABLED@)
if(STASIS_NETWORK_ENABLED AND STASIS_NETWORK_CLIENT_ENABLED)
    message(FATAL_ERROR "Host and client networking capabilities are mutually exclusive")
endif()

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
set(CMAKE_POSITION_INDEPENDENT_CODE ON)
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

if(STASIS_NETWORK_ENABLED OR STASIS_NETWORK_CLIENT_ENABLED)
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
    else()
        target_compile_definitions(stasis_mobile_runtime PUBLIC STASIS_NETWORK_CLIENT_ENABLED=1)
    endif()
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
set_property(TARGET main PROPERTY LINKER_LANGUAGE CXX)
if(STASIS_ENABLE_SEAM_TESTS)
    target_compile_definitions(main PRIVATE STASIS_ENABLE_SEAM_TESTS=1)
endif()
target_include_directories(main PRIVATE "${STASIS_AOT_DIR}" "${STASIS_ROOT}/runtime")
target_link_libraries(main PRIVATE
    stasis_mobile_runtime
    SDL3::SDL3-static
    SDL3_image::SDL3_image-static
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
    else()
        target_compile_definitions(main PRIVATE STASIS_NETWORK_CLIENT_ENABLED=1)
    endif()
endif()
