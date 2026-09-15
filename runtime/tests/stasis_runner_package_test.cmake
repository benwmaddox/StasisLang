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
