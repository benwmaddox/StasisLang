cmake_minimum_required(VERSION 3.22.1)
project(stasis_mobile_android C)

if(NOT STASIS_SDL2_SOURCE OR NOT STASIS_SDL2_IMAGE_SOURCE)
    message(FATAL_ERROR "STASIS_SDL2_SOURCE and STASIS_SDL2_IMAGE_SOURCE are required")
endif()

set(STASIS_ROOT "${CMAKE_CURRENT_LIST_DIR}/../../../../..")
set(STASIS_AOT_DIR "${STASIS_ROOT}/aot")
set(STASIS_BUILD_MOBILE_RUNTIME ON CACHE BOOL "" FORCE)
set(STASIS_GRAPHICS_BUILD_SHARED OFF CACHE BOOL "" FORCE)
set(STASIS_GRAPHICS_BUILD_STATIC OFF CACHE BOOL "" FORCE)
set(STASIS_BUILD_RUNNER OFF CACHE BOOL "" FORCE)
set(STASIS_BUILD_SYS OFF CACHE BOOL "" FORCE)

add_subdirectory("${STASIS_SDL2_SOURCE}" "${CMAKE_BINARY_DIR}/sdl2")
set(SDL2IMAGE_SAMPLES OFF CACHE BOOL "" FORCE)
set(SDL2IMAGE_TESTS OFF CACHE BOOL "" FORCE)
add_subdirectory("${STASIS_SDL2_IMAGE_SOURCE}" "${CMAKE_BINARY_DIR}/sdl2_image")
add_subdirectory("${STASIS_ROOT}/runtime" "${CMAKE_BINARY_DIR}/stasis_runtime")

file(GLOB STASIS_AOT_OBJECTS CONFIGURE_DEPENDS "${STASIS_AOT_DIR}/*.o")
if(NOT STASIS_AOT_OBJECTS)
    message(FATAL_ERROR "No Android AOT objects found in ${STASIS_AOT_DIR}")
endif()

add_library(main SHARED
    "${STASIS_ROOT}/common/stasis_mobile_main.c"
    "${CMAKE_CURRENT_LIST_DIR}/stasis_android_assets.c"
    "${STASIS_AOT_DIR}/published_aot_bindings.c"
    ${STASIS_AOT_OBJECTS}
)
target_include_directories(main PRIVATE "${STASIS_AOT_DIR}" "${STASIS_ROOT}/runtime")
target_link_libraries(main PRIVATE stasis_mobile_runtime SDL2::SDL2 SDL2_image::SDL2_image log android)
