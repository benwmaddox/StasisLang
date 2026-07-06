cmake_minimum_required(VERSION 3.22.1)
project(stasis_mobile_smoke C)

option(STASIS_ANDROID_SMOKE_ONLY "Build the standalone Android native smoke entrypoint" ON)

add_library(stasis_mobile_smoke SHARED stasis_mobile_smoke.c)

find_library(log_lib log)
find_library(dl_lib dl)
target_link_libraries(stasis_mobile_smoke PRIVATE ${log_lib} ${dl_lib})