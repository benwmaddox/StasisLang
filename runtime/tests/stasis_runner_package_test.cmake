if(NOT DEFINED TEST_ROOT OR NOT DEFINED RUNNER OR NOT DEFINED GRAPHICS OR NOT DEFINED GAME)
    message(FATAL_ERROR "packaged runner smoke test paths are required")
endif()

file(REMOVE_RECURSE "${TEST_ROOT}")
file(MAKE_DIRECTORY "${TEST_ROOT}/package" "${TEST_ROOT}/caller")
file(COPY_FILE "${RUNNER}" "${TEST_ROOT}/package/game")
file(CHMOD "${TEST_ROOT}/package/game" PERMISSIONS OWNER_READ OWNER_WRITE OWNER_EXECUTE)
get_filename_component(GRAPHICS_NAME "${GRAPHICS}" NAME)
file(COPY_FILE "${GRAPHICS}" "${TEST_ROOT}/package/${GRAPHICS_NAME}")
get_filename_component(GAME_NAME "${GAME}" NAME)
file(COPY_FILE "${GAME}" "${TEST_ROOT}/package/${GAME_NAME}")
file(WRITE "${TEST_ROOT}/package/game.launch" "dll=${GAME_NAME}\nentry=main\nfps=60\n")

execute_process(
    COMMAND "../package/game"
    WORKING_DIRECTORY "${TEST_ROOT}/caller"
    TIMEOUT 30
    RESULT_VARIABLE RESULT
    OUTPUT_VARIABLE STDOUT
    ERROR_VARIABLE STDERR
)
if(NOT RESULT EQUAL 0 OR NOT STDOUT MATCHES "PACKAGED_RUNNER_OK")
    message(FATAL_ERROR "packaged runner failed (${RESULT})\nstdout=${STDOUT}\nstderr=${STDERR}")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" -E env STASIS_TEST_WINDOW_INIT_FAILURE=1 "../package/game"
    WORKING_DIRECTORY "${TEST_ROOT}/caller"
    TIMEOUT 30
    RESULT_VARIABLE INIT_FAILURE_RESULT
    OUTPUT_VARIABLE INIT_FAILURE_STDOUT
    ERROR_VARIABLE INIT_FAILURE_STDERR
)
if(INIT_FAILURE_RESULT EQUAL 0 OR
   NOT INIT_FAILURE_STDERR MATCHES "packaged graphics runtime failed to initialize a window")
    message(FATAL_ERROR
        "packaged runner did not reject failed graphics initialization (${INIT_FAILURE_RESULT})\n"
        "stdout=${INIT_FAILURE_STDOUT}\nstderr=${INIT_FAILURE_STDERR}")
endif()

function(run_lifecycle_metadata_case CASE_NAME METADATA EXPECTED_VERSION)
    if("${METADATA}" STREQUAL "")
        set(LIFECYCLE_LINE "")
    else()
        set(LIFECYCLE_LINE "render_construction_lifecycle_version=${METADATA}\n")
    endif()
    file(WRITE "${TEST_ROOT}/package/game.launch"
        "dll=${GAME_NAME}\nentry=main\nfps=60\n${LIFECYCLE_LINE}")
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -E env STASIS_RUNNER_DIAG=1 "../package/game"
        WORKING_DIRECTORY "${TEST_ROOT}/caller"
        TIMEOUT 30
        RESULT_VARIABLE CASE_RESULT
        OUTPUT_VARIABLE CASE_STDOUT
        ERROR_VARIABLE CASE_STDERR
    )
    if(NOT CASE_RESULT EQUAL 0 OR
       NOT CASE_STDOUT MATCHES "PACKAGED_RUNNER_OK" OR
       NOT CASE_STDERR MATCHES "render_construction_lifecycle_version=${EXPECTED_VERSION}")
        message(FATAL_ERROR
            "packaged runner lifecycle case ${CASE_NAME} failed (${CASE_RESULT})\n"
            "stdout=${CASE_STDOUT}\nstderr=${CASE_STDERR}")
    endif()
endfunction()

run_lifecycle_metadata_case(absent "" 0)
run_lifecycle_metadata_case(v0 0 0)

file(WRITE "${TEST_ROOT}/package/game.launch"
    "dll=${GAME_NAME}\nentry=main\nfps=60\nrender_construction_lifecycle_version=1\n")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E env STASIS_RUNNER_DIAG=1 "../package/game"
    WORKING_DIRECTORY "${TEST_ROOT}/caller"
    TIMEOUT 30
    RESULT_VARIABLE MISSING_RENDER_RESULT
    OUTPUT_VARIABLE MISSING_RENDER_STDOUT
    ERROR_VARIABLE MISSING_RENDER_STDERR
)
if(MISSING_RENDER_RESULT EQUAL 0 OR
   NOT MISSING_RENDER_STDERR MATCHES "requires a non-empty render entry")
    message(FATAL_ERROR
        "packaged runner accepted lifecycle v1 without a render entry (${MISSING_RENDER_RESULT})\n"
        "stdout=${MISSING_RENDER_STDOUT}\nstderr=${MISSING_RENDER_STDERR}")
endif()

file(WRITE "${TEST_ROOT}/package/game.launch"
    "dll=${GAME_NAME}\nentry=main\nfps=60\nrender_construction_lifecycle_version=2\n")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E env STASIS_RUNNER_DIAG=1 "../package/game"
    WORKING_DIRECTORY "${TEST_ROOT}/caller"
    TIMEOUT 30
    RESULT_VARIABLE UNSUPPORTED_RESULT
    OUTPUT_VARIABLE UNSUPPORTED_STDOUT
    ERROR_VARIABLE UNSUPPORTED_STDERR
)
if(UNSUPPORTED_RESULT EQUAL 0 OR
   NOT UNSUPPORTED_STDERR MATCHES "unsupported render construction lifecycle version" OR
   NOT UNSUPPORTED_STDERR MATCHES "expected 0 or 1")
    message(FATAL_ERROR
        "packaged runner did not reject unsupported render lifecycle metadata (${UNSUPPORTED_RESULT})\n"
        "stdout=${UNSUPPORTED_STDOUT}\nstderr=${UNSUPPORTED_STDERR}")
endif()

file(WRITE "${TEST_ROOT}/package/game.launch"
    "dll=${GAME_NAME}\nentry=main\nfps=60\nrender_construction_lifecycle_version=0\nrender_construction_lifecycle_version=1\n")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E env STASIS_RUNNER_DIAG=1 "../package/game"
    WORKING_DIRECTORY "${TEST_ROOT}/caller"
    TIMEOUT 30
    RESULT_VARIABLE DUPLICATE_RESULT
    OUTPUT_VARIABLE DUPLICATE_STDOUT
    ERROR_VARIABLE DUPLICATE_STDERR
)
if(DUPLICATE_RESULT EQUAL 0 OR
   NOT DUPLICATE_STDERR MATCHES "duplicate render_construction_lifecycle_version")
    message(FATAL_ERROR
        "packaged runner did not reject duplicate render lifecycle metadata (${DUPLICATE_RESULT})\n"
        "stdout=${DUPLICATE_STDOUT}\nstderr=${DUPLICATE_STDERR}")
endif()

if(NOT DEFINED LIFECYCLE_GRAPHICS OR NOT DEFINED LIFECYCLE_GAME)
    message(FATAL_ERROR "packaged runner lifecycle fixture paths are required")
endif()
file(MAKE_DIRECTORY "${TEST_ROOT}/lifecycle-package" "${TEST_ROOT}/lifecycle-caller")
file(COPY_FILE "${RUNNER}" "${TEST_ROOT}/lifecycle-package/game")
file(CHMOD "${TEST_ROOT}/lifecycle-package/game" PERMISSIONS OWNER_READ OWNER_WRITE OWNER_EXECUTE)
get_filename_component(LIFECYCLE_GRAPHICS_NAME "${GRAPHICS}" NAME)
get_filename_component(LIFECYCLE_GAME_NAME "${LIFECYCLE_GAME}" NAME)
file(COPY_FILE "${LIFECYCLE_GRAPHICS}" "${TEST_ROOT}/lifecycle-package/${LIFECYCLE_GRAPHICS_NAME}")
file(COPY_FILE "${LIFECYCLE_GAME}" "${TEST_ROOT}/lifecycle-package/${LIFECYCLE_GAME_NAME}")
file(WRITE "${TEST_ROOT}/lifecycle-package/game.launch"
    "dll=${LIFECYCLE_GAME_NAME}\nentry=main\nfps=60\nrender=legacy_render\nrender_construction_lifecycle_version=0\n")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E env STASIS_RUNNER_DIAG=1 "../lifecycle-package/game"
    WORKING_DIRECTORY "${TEST_ROOT}/lifecycle-caller"
    TIMEOUT 30
    RESULT_VARIABLE LEGACY_RESULT
    OUTPUT_VARIABLE LEGACY_STDOUT
    ERROR_VARIABLE LEGACY_STDERR
)
if(LEGACY_RESULT NOT EQUAL 0 OR
   NOT LEGACY_STDOUT MATCHES "PACKAGED_RUNNER_LIFECYCLE_V0_DIRECT" OR
   NOT LEGACY_STDERR MATCHES "render_construction_lifecycle_version=0")
    message(FATAL_ERROR
        "packaged runner lifecycle v0 fixture failed (${LEGACY_RESULT})\n"
        "stdout=${LEGACY_STDOUT}\nstderr=${LEGACY_STDERR}")
endif()

file(WRITE "${TEST_ROOT}/lifecycle-package/game.launch"
    "dll=${LIFECYCLE_GAME_NAME}\nentry=main\nfps=60\nrender=render\nrender_construction_lifecycle_version=1\n")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E env STASIS_RUNNER_DIAG=1 "../lifecycle-package/game"
    WORKING_DIRECTORY "${TEST_ROOT}/lifecycle-caller"
    TIMEOUT 30
    RESULT_VARIABLE BRIDGE_RESULT
    OUTPUT_VARIABLE BRIDGE_STDOUT
    ERROR_VARIABLE BRIDGE_STDERR
)
if(BRIDGE_RESULT NOT EQUAL 0 OR
   NOT BRIDGE_STDOUT MATCHES "PACKAGED_RUNNER_LIFECYCLE_V1_PUBLISHED" OR
   NOT BRIDGE_STDOUT MATCHES "PACKAGED_RUNNER_LIFECYCLE_V1_NESTED_BEGIN_REJECTED" OR
   NOT BRIDGE_STDERR MATCHES "render_construction_lifecycle_version=1")
    message(FATAL_ERROR
        "packaged runner lifecycle v1 fixture failed (${BRIDGE_RESULT})\n"
        "stdout=${BRIDGE_STDOUT}\nstderr=${BRIDGE_STDERR}")
endif()
