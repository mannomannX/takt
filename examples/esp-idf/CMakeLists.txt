# Takt unter ESP-IDF (12.11, M11 Schritt 21a): ein Programm als Baustein
# einer Anwendung von ESP-IDF auf dem ESP32-C6, in der Interruptform.
#
#   idf.py -C examples/esp-idf -B <bau> -DTAKT_EXECUTABLE=<takt>
#          -DTAKT_SDK_LIBRARY=<SDK-Archiv fuer riscv32imac-unknown-none-elf>
#          [-DTAKT_PROGRAM=<programm.takt>] [-DTAKT_TICKS=<n>]
#          [-DTAKT_FRESH_JOURNAL=ON] build merge-bin
#
# Das SDK-Archiv baut `cargo rustc --release --lib --crate-type staticlib
# --target riscv32imac-unknown-none-elf --manifest-path
# crates/takt-embed-c/Cargo.toml`. Die Komponente `takt` liegt unter
# `esp-idf/` an der Wurzel von Takt; eine Anwendung ausserhalb nimmt sie ueber
# `EXTRA_COMPONENT_DIRS`. Die Fassung von ESP-IDF ist v6.1-beta1
# (`.github/actions/esp-idf`).

cmake_minimum_required(VERSION 3.22)

set(TAKT_ROOT ${CMAKE_CURRENT_LIST_DIR}/../..)
set(EXTRA_COMPONENT_DIRS ${TAKT_ROOT}/esp-idf)
include($ENV{IDF_PATH}/tools/cmake/project.cmake)
# Nur `main` und was es verlangt: Was die Anwendung nicht nennt, gehoert
# nicht ins Abbild.
idf_build_set_property(MINIMAL_BUILD ON)
# Die fremde Last neben Takt (main/load.h): als Eigenschaft des Baus, denn
# welche Komponenten `main` verlangt, liest ESP-IDF in einem eigenen
# Skript, das den Cache nicht sieht.
set(TAKT_LOAD quiet CACHE STRING "die fremde Last neben Takt: quiet, wifi, ble oder suspend")
idf_build_set_property(TAKT_LOAD "${TAKT_LOAD}")
project(takt_esp_idf)
