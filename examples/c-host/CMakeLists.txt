# Ein Fuellventil als Takt-Programm in einem C-Projekt (12.11, M11 Schritt 18a).
#
#   cmake -S examples/c-host -B build -G Ninja -DCMAKE_C_COMPILER=clang
#         -DTakt_DIR=<takt>/tools/cmake -DTAKT_EXECUTABLE=<takt>
#         -DTAKT_SDK_LIBRARY=<SDK-Archiv>
#   cmake --build build
#   build/c_host 1000
#
# Der Trace auf stdout gleicht dem von `takt sim takt/valve.takt --ticks 1000`.

cmake_minimum_required(VERSION 3.20)
project(c_host C)
set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)

find_package(Takt REQUIRED)
takt_add_program(valve SOURCE takt/valve.takt FORM logical)

add_executable(c_host main.c)
target_link_libraries(c_host PRIVATE valve)
