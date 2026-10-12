# Takt ueber CMSIS-RTOS2 (12.11, M11 Schritt 19c): das Pruefprogramm der
# Portpruefung mit dem generischen Adapter `ports/cmsis-rtos2`, unter
# Zephyrs CMSIS-Schicht auf `blackpill_f401cc`.
#
#   west build -b blackpill_f401cc examples/cmsis-rtos2 --
#       -DTAKT_EXECUTABLE=<takt> -DTAKT_SDK_LIBRARY=<SDK-Archiv des Ziels>
#
# Leitung und Rueckgabe des Boards teilt es mit `examples/zephyr`.

cmake_minimum_required(VERSION 3.20.0)

set(TAKT_ROOT ${CMAKE_CURRENT_LIST_DIR}/../..)
list(APPEND ZEPHYR_EXTRA_MODULES ${TAKT_ROOT})
find_package(Zephyr REQUIRED HINTS $ENV{ZEPHYR_BASE})
project(takt_cmsis_rtos2 C)

if(NOT BOARD STREQUAL "blackpill_f401cc")
  message(FATAL_ERROR "examples/cmsis-rtos2: geprueft auf blackpill_f401cc, nicht auf ${BOARD}")
endif()

# Im Sim-Build, wie `takt port-test --emit embed` es liefert.
takt_add_program(port_test SOURCE ${TAKT_ROOT}/corpus-try/124_port_test.takt
                 HARDWARE ${TAKT_ROOT}/corpus-try/hw/stm32f401.hw BUILD sim)

set(shared ${TAKT_ROOT}/examples/zephyr/src)
target_include_directories(app PRIVATE ${shared})
target_sources(app PRIVATE src/main.c ${shared}/line.c ${shared}/handback.c ${shared}/handback_stm32f401.c)
target_link_libraries(app PRIVATE port_test)
