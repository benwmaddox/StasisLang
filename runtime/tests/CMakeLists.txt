cmake_minimum_required(VERSION 3.16)
project(stasis_mobile_runtime_tests C)

enable_testing()

add_library(stasis_mobile_runtime_core STATIC ../stasis_mobile_runtime.c)
target_include_directories(stasis_mobile_runtime_core PUBLIC ..)

add_library(stasis_mobile_fake_sdl STATIC fake_sdl_runtime.c)

add_executable(stasis_mobile_runtime_test stasis_mobile_runtime_test.c)
target_link_libraries(stasis_mobile_runtime_test PRIVATE
    stasis_mobile_runtime_core
    stasis_mobile_fake_sdl)
add_test(NAME stasis_mobile_runtime_lifecycle COMMAND stasis_mobile_runtime_test)

foreach(platform android ios)
    add_executable(stasis_${platform}_shell_link_test mobile_shell_link_test.c)
    target_compile_definitions(stasis_${platform}_shell_link_test PRIVATE
        STASIS_TEST_PLATFORM="${platform}")
    target_link_libraries(stasis_${platform}_shell_link_test PRIVATE
        stasis_mobile_runtime_core
        stasis_mobile_fake_sdl)
    add_test(NAME stasis_${platform}_shell_links_shared_core
        COMMAND stasis_${platform}_shell_link_test)
endforeach()
