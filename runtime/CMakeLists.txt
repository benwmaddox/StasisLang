cmake_minimum_required(VERSION 3.16)
project(stasis_graphics C)

# Find SDL2 (via vcpkg or system)
find_package(SDL2 CONFIG REQUIRED)

# Find OpenGL
find_package(OpenGL REQUIRED)
find_package(GLEW REQUIRED)

# Create shared library
add_library(stasis_graphics SHARED stasis_graphics.c)

# Link dependencies
target_link_libraries(stasis_graphics PRIVATE
    $<TARGET_NAME_IF_EXISTS:SDL2::SDL2main>
    $<IF:$<TARGET_EXISTS:SDL2::SDL2>,SDL2::SDL2,SDL2::SDL2-static>
    OpenGL::GL
    GLEW::GLEW
)

# Export symbols on Windows
if(WIN32)
    target_compile_definitions(stasis_graphics PRIVATE _CRT_SECURE_NO_WARNINGS)
endif()

# Set output directory to be next to the Stasis CLI
set_target_properties(stasis_graphics PROPERTIES
    RUNTIME_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/bin"
    LIBRARY_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/bin"
)

# Install target
install(TARGETS stasis_graphics
    RUNTIME DESTINATION bin
    LIBRARY DESTINATION lib
)
