cmake_minimum_required(VERSION 3.22.1)
project(stasis_mobile_smoke C)

option(STASIS_ANDROID_SMOKE_ONLY "Build the standalone Android native smoke entrypoint" ON)

add_library(stasis_mobile_smoke SHARED
    stasis_mobile_smoke.c
    stasis_android_sprite.c
    "${CMAKE_CURRENT_SOURCE_DIR}/../../../../../../runtime/stasis_platform_storage.c")
target_include_directories(stasis_mobile_smoke PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/../../../../../../runtime")

find_library(log_lib log)
find_library(dl_lib dl)
find_library(math_lib m)
target_link_libraries(stasis_mobile_smoke PRIVATE ${log_lib} ${dl_lib} ${math_lib})
