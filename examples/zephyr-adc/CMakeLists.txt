# Takt unter Zephyr mit einem analogen Eingang aus dem Devicetree (8.10,
# 12.11; FB-735): die Messschleife `takt/analog.takt` auf der Drahtbruecke
# des F401 (PA0 an PA1, PA1 als Eingang 1 von ADC1). Fuer
# `esp32c6_devkitc/esp32c6/hpcore` baut das Beispiel nur: Die Bruecke des C6
# endet an GPIO17, und der ist kein Eingang des ADC.
#
#   west build -b blackpill_f401cc examples/zephyr-adc --
#       -DTAKT_EXECUTABLE=<takt> -DTAKT_SDK_LIBRARY=<SDK-Archiv des Ziels>
#
# `takt/analog.hw` nennt fuer den Pin `zephyr-gpio` und fuer den Eingang
# `zephyr-adc`, als `port` die Labels der Knoten aus boards/*.overlay; das
# Modul gibt `takt build` dafuer `--zephyr` und uebersetzt `analog_zephyr.c`
# mit. Leitung und Rueckgabe des Boards teilt das Beispiel mit
# `examples/zephyr`.

cmake_minimum_required(VERSION 3.20.0)

set(TAKT_ROOT ${CMAKE_CURRENT_LIST_DIR}/../..)
list(APPEND ZEPHYR_EXTRA_MODULES ${TAKT_ROOT})
find_package(Zephyr REQUIRED HINTS $ENV{ZEPHYR_BASE})
project(takt_zephyr_adc C)

takt_add_program(analog SOURCE takt/analog.takt HARDWARE takt/analog.hw)

set(shared ${TAKT_ROOT}/examples/zephyr/src)
target_include_directories(app PRIVATE ${shared})
target_sources(app PRIVATE src/main.c ${shared}/line.c ${shared}/handback.c)
if(BOARD STREQUAL "blackpill_f401cc")
  target_sources(app PRIVATE ${shared}/handback_stm32f401.c)
else()
  target_sources(app PRIVATE ${shared}/handback_reboot.c)
endif()
target_link_libraries(app PRIVATE analog)
