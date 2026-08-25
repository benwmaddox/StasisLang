cmake_minimum_required(VERSION 3.22.1)
project(stasis_mobile_smoke C)

option(STASIS_ANDROID_SMOKE_ONLY "Build the standalone Android native smoke entrypoint" ON)
option(STASIS_ANDROID_PUBLISHED_AOT "Link compiler-produced AOT objects for published Android builds" OFF)
set(STASIS_PUBLISHED_AOT_DIR "" CACHE PATH "Directory containing generated published Stasis AOT objects")

add_library(
    stasis_mobile_smoke SHARED
    stasis_mobile_smoke.c
    stasis_android_sprite.c
    stasis_android_audio.c
    ../../../../../../runtime/stasis_audio_ring.c
)
target_include_directories(stasis_mobile_smoke PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/../../../../../../runtime")

find_library(log_lib log)
find_library(dl_lib dl)
find_library(math_lib m)
find_library(aaudio_lib aaudio)
target_link_libraries(stasis_mobile_smoke PRIVATE ${log_lib} ${dl_lib} ${math_lib} ${aaudio_lib})

if(STASIS_ANDROID_PUBLISHED_AOT)
    if(NOT STASIS_PUBLISHED_AOT_DIR)
        message(FATAL_ERROR "STASIS_PUBLISHED_AOT_DIR is required when STASIS_ANDROID_PUBLISHED_AOT=ON")
    endif()
    include("${STASIS_PUBLISHED_AOT_DIR}/published_aot_objects.cmake")
    target_sources(
        stasis_mobile_smoke PRIVATE
        ${STASIS_PUBLISHED_AOT_OBJECTS}
        ../../../../../../runtime/stasis_mobile_aot_runtime.c
        stasis_android_runtime_platform.c
    )
    target_include_directories(stasis_mobile_smoke PRIVATE "${STASIS_PUBLISHED_AOT_DIR}")
    target_compile_definitions(stasis_mobile_smoke PRIVATE STASIS_ANDROID_PUBLISHED_AOT=1)
endif()
