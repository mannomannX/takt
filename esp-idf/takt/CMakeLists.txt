# Takt als Komponente von ESP-IDF (12.11, M11 Schritt 21a).
#
# Die Komponente baut den Port der Interruptform
# (`crates/takt-embed-c/ports/irq`) mit dem Adapter fuer ESP-IDF
# (`ports/irq/esp-idf`) und das Geraet des Journals in einer Partition
# (`ports/esp-idf-nvm`), und sie stellt `takt_add_program` aus dem CMake-Paket
# (tools/cmake/TaktConfig.cmake) mit der Form `interrupt` und dem Tripel des
# C6 als Vorgabe:
#
#   takt_add_program(valve SOURCE takt/valve.takt HARDWARE takt/board.hw LDGEN)
#   idf_component_register(... LDFRAGMENTS ${CMAKE_CURRENT_BINARY_DIR}/takt/valve/valve.lf)
#   target_link_libraries(${COMPONENT_LIB} PRIVATE valve)
#
# Eine Anwendung nimmt sie ueber `EXTRA_COMPONENT_DIRS` (`<Takt>/esp-idf`);
# das Werkzeug und das SDK-Archiv fuer `riscv32imac-unknown-none-elf` nennt
# der Bau wie beim CMake-Paket: `-DTAKT_EXECUTABLE=…` und
# `-DTAKT_SDK_LIBRARY=…`.
#
# **Die Form ist `interrupt`** (plan/m11.md 1.7, 2.5): Unter ESP-IDF haelt
# jeder Flash-Zugriff den Planer an, eine Aufgabe stuende dann still. Der
# Schritt rechnet darum in Interrupts, die ESP-IDF weiterlaufen laesst, und
# alles, was sie rufen, liegt im RAM: Port, Adapter und Geraet aus dieser
# Komponente und das SDK-Archiv (`takt.lf`), das Programm ueber sein
# Fragment (`LDGEN`). Was der Wirt im Schritt ruft — Leitung, Watchdog —,
# legt er selbst dorthin.

set(root ${CMAKE_CURRENT_LIST_DIR}/../..)
set(sdk ${root}/crates/takt-embed-c)

if(BOOTLOADER_BUILD)
  # Der Bootloader baut ohne Takt.
  idf_component_register()
  return()
endif()

idf_component_register(
  SRCS ${sdk}/ports/irq/takt_irq.c ${sdk}/ports/irq/esp-idf/takt_irq_chip.c ${sdk}/ports/esp-idf-nvm/takt_idf_nvm.c
  INCLUDE_DIRS ${sdk}/include ${sdk}/ports/irq ${sdk}/ports/irq/esp-idf ${sdk}/ports/esp-idf-nvm
  REQUIRES esp_driver_gptimer esp_partition esp_hw_support freertos soc
  LDFRAGMENTS takt.lf)

if(CMAKE_BUILD_EARLY_EXPANSION)
  return()
endif()

set(TAKT_FORM interrupt CACHE INTERNAL "die Form der Programme unter ESP-IDF (12.11, plan/m11.md 2.5)")
set(TAKT_TARGET riscv32imac-unknown-none-elf CACHE INTERNAL "das Tripel der Programme auf dem C6")
include(${root}/tools/cmake/TaktConfig.cmake)
target_link_libraries(${COMPONENT_LIB} PUBLIC Takt::sdk)
