# Takt unter Zephyr mit Treibern aus dem Devicetree (8.10, 12.11; M11
# Schritt 19): die Messschleife `takt/wire.takt` auf der Drahtbruecke des
# Boards, auf `blackpill_f401cc` (PA0 an PA1) und
# `esp32c6_devkitc/esp32c6/hpcore` (GPIO7 an GPIO17).
#
#   west build -b blackpill_f401cc examples/zephyr-gpio --
#       -DTAKT_EXECUTABLE=<takt> -DTAKT_SDK_LIBRARY=<SDK-Archiv des Ziels>
#
# `takt/wire.hw` nennt fuer beide Kanaele den Treiber `zephyr-gpio` und als
# `port` die Labels der Knoten aus boards/*.overlay; das Modul gibt `takt
# build` dafuer `--zephyr` und uebersetzt `wire_zephyr.c` mit. Leitung und
# Rueckgabe des Boards teilt das Beispiel mit `examples/zephyr`.

cmake_minimum_required(VERSION 3.20.0)

set(TAKT_ROOT ${CMAKE_CURRENT_LIST_DIR}/../..)
list(APPEND ZEPHYR_EXTRA_MODULES ${TAKT_ROOT})
find_package(Zephyr REQUIRED HINTS $ENV{ZEPHYR_BASE})
project(takt_zephyr_gpio C)

takt_add_program(wire SOURCE takt/wire.takt HARDWARE takt/wire.hw)

set(shared ${TAKT_ROOT}/examples/zephyr/src)
target_include_directories(app PRIVATE ${shared})
target_sources(app PRIVATE src/main.c ${shared}/line.c ${shared}/handback.c)
if(BOARD STREQUAL "blackpill_f401cc")
  target_sources(app PRIVATE ${shared}/handback_stm32f401.c)
else()
  target_sources(app PRIVATE ${shared}/handback_reboot.c)
endif()
target_link_libraries(app PRIVATE wire)
