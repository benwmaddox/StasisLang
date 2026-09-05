cmake_minimum_required(VERSION 3.22.1)
project(stasis_mobile_smoke C CXX)

option(STASIS_ANDROID_SMOKE_ONLY "Build the standalone Android native smoke entrypoint" ON)
option(STASIS_RENDER_ACCEPTANCE "Emit render-acceptance-only Workshop seam evidence" OFF)

add_library(stasis_mobile_smoke SHARED
    stasis_mobile_smoke.c
    stasis_android_sprite.c
    stasis_android_audio.c
    "${CMAKE_CURRENT_SOURCE_DIR}/../../../../../../runtime/stasis_svg.cpp"
    "${CMAKE_CURRENT_SOURCE_DIR}/../../../../../../runtime/stasis_audio_assets.c"
    "${CMAKE_CURRENT_SOURCE_DIR}/../../../../../../runtime/stasis_audio_ring.c"
    "${CMAKE_CURRENT_SOURCE_DIR}/../../../../../../runtime/stasis_platform_storage.c")
add_subdirectory(
    "${CMAKE_CURRENT_SOURCE_DIR}/../../../../../../runtime/third_party/thorvg"
    "${CMAKE_CURRENT_BINARY_DIR}/stasis_thorvg")
if(STASIS_RENDER_ACCEPTANCE)
    target_compile_definitions(stasis_mobile_smoke PRIVATE STASIS_RENDER_ACCEPTANCE=1)
endif()
target_include_directories(stasis_mobile_smoke PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/../../../../../../runtime")

find_library(log_lib log)
find_library(dl_lib dl)
find_library(math_lib m)
target_link_libraries(stasis_mobile_smoke PRIVATE stasis_thorvg ${log_lib} ${dl_lib} ${math_lib})
find_library(aaudio_lib aaudio)
target_link_libraries(stasis_mobile_smoke PRIVATE ${aaudio_lib})
