# Takt als Zephyr-Modul (12.11, M11 Schritt 19).
#
# Mit `CONFIG_TAKT=y` baut das Modul den Port mit Faeden
# (`crates/takt-embed-c/ports/rtos`) mit dem Adapter fuer Zephyr
# (`ports/zephyr`) und stellt der Anwendung `takt_add_program` aus dem
# CMake-Paket (tools/cmake/TaktConfig.cmake), mit der Form `rtos` und dem
# Tripel aus `CONFIG_TAKT_TARGET` als Vorgabe:
#
#   takt_add_program(valve SOURCE takt/valve.takt HARDWARE takt/board.hw)
#   target_link_libraries(app PRIVATE valve)
#
# Das Werkzeug und das SDK-Archiv fuer das Ziel nennt der Bau wie beim
# CMake-Paket: `-DTAKT_EXECUTABLE=…` und `-DTAKT_SDK_LIBRARY=…`.

if(CONFIG_TAKT)
  if(NOT CONFIG_TAKT_TARGET)
    message(FATAL_ERROR "Takt: CONFIG_TAKT_TARGET nennt kein Tripel fuer dieses Ziel (zephyr/Kconfig)")
  endif()
  set(TAKT_FORM rtos CACHE INTERNAL "die Form der Programme unter Zephyr (12.11)")
  set(TAKT_TARGET "${CONFIG_TAKT_TARGET}" CACHE INTERNAL "das Tripel der Programme (CONFIG_TAKT_TARGET)")
  include(${ZEPHYR_CURRENT_MODULE_DIR}/tools/cmake/TaktConfig.cmake)

  set(sdk ${ZEPHYR_CURRENT_MODULE_DIR}/crates/takt-embed-c)
  zephyr_include_directories(${sdk}/include ${sdk}/ports/rtos ${sdk}/ports/zephyr)
  zephyr_library_named(takt_port)
  zephyr_library_sources(${sdk}/ports/rtos/takt_rtos.c ${sdk}/ports/zephyr/takt_os.c)
  zephyr_library_link_libraries(Takt::sdk)
endif()
