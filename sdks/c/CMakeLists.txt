cmake_minimum_required(VERSION 3.20)
project(mynofy_c_consumer LANGUAGES C)
set(MYNOFY_LIBRARY "" CACHE FILEPATH "Absolute path to a prebuilt libmynofy_c shared/static library")
if(NOT EXISTS "${MYNOFY_LIBRARY}")
    message(FATAL_ERROR "Build cargo build --release -p mynofy-client-c, then set MYNOFY_LIBRARY")
endif()
find_package(Threads REQUIRED)
add_library(mynofy::client UNKNOWN IMPORTED GLOBAL)
set_target_properties(mynofy::client PROPERTIES
    IMPORTED_LOCATION "${MYNOFY_LIBRARY}"
    INTERFACE_INCLUDE_DIRECTORIES "${CMAKE_CURRENT_SOURCE_DIR}/include"
    INTERFACE_LINK_LIBRARIES "Threads::Threads;${CMAKE_DL_LIBS};$<$<PLATFORM_ID:Linux>:gcc_s;util;rt;m;c>")
add_executable(mynofy-c-example examples/local.c)
set_target_properties(mynofy-c-example PROPERTIES C_STANDARD 11 C_STANDARD_REQUIRED YES C_EXTENSIONS NO)
target_link_libraries(mynofy-c-example PRIVATE mynofy::client)
