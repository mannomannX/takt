# Takt unter Zephyr (12.11, M11 Schritt 19): das Pruefprogramm der
# Portpruefung als Baustein einer Zephyr-Anwendung, auf `blackpill_f401cc`
# und `esp32c6_devkitc/esp32c6/hpcore`.
#
#   west build -b blackpill_f401cc examples/zephyr --
#       -DTAKT_EXECUTABLE=<takt> -DTAKT_SDK_LIBRARY=<SDK-Archiv des Ziels>
#
# Das SDK-Archiv ist fuer das Tripel aus `CONFIG_TAKT_TARGET` gebaut
# (`cargo rustc --release --lib --crate-type staticlib --target TRIPEL
# --manifest-path crates/takt-embed-c/Cargo.toml`). Das Modul ist die Wurzel
# von Takt (`zephyr/module.yml`); eine Anwendung ausserhalb nimmt es ueber
# ihr west-Manifest oder `ZEPHYR_EXTRA_MODULES`.

cmake_minimum_required(VERSION 3.20.0)

set(TAKT_ROOT ${CMAKE_CURRENT_LIST_DIR}/../..)
list(APPEND ZEPHYR_EXTRA_MODULES ${TAKT_ROOT})
find_package(Zephyr REQUIRED HINTS $ENV{ZEPHYR_BASE})
project(takt_zephyr C)

# Die Hardware-Konfiguration des Boards (8.10): Kalibrierung, Stack-Reserve
# der Form `rtos`, Journal.
if(BOARD STREQUAL "blackpill_f401cc")
  set(hardware ${TAKT_ROOT}/corpus-try/hw/stm32f401.hw)
elseif(BOARD STREQUAL "esp32c6_devkitc")
  set(hardware ${TAKT_ROOT}/corpus-try/hw/esp32c6.hw)
else()
  message(FATAL_ERROR "examples/zephyr: keine Hardware-Konfiguration fuer ${BOARD}")
endif()

# Im Sim-Build, wie `takt port-test --emit embed` es liefert: Die Ausgaenge
# gehen ohne Treiber in den Trace.
takt_add_program(port_test SOURCE ${TAKT_ROOT}/corpus-try/124_port_test.takt HARDWARE ${hardware} BUILD sim)

target_sources(app PRIVATE src/main.c src/line.c src/handback.c src/partition.c)
if(BOARD STREQUAL "blackpill_f401cc")
  target_sources(app PRIVATE src/handback_stm32f401.c)
else()
  target_sources(app PRIVATE src/handback_reboot.c)
endif()
target_link_libraries(app PRIVATE port_test)

# Ein frisches Journal fuer die Portpruefung, die mit dem Interpreter ohne
# gespeicherte Werte vergleicht; ohne ueberlebt `persist` einen Neustart.
option(TAKT_FRESH_JOURNAL "das Journal vor dem Lauf loeschen" OFF)
if(TAKT_FRESH_JOURNAL)
  target_compile_definitions(app PRIVATE TAKT_FRESH_JOURNAL=1)
else()
  target_compile_definitions(app PRIVATE TAKT_FRESH_JOURNAL=0)
endif()
