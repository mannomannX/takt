# Takt als Zephyr-Modul (12.11, M11 Schritt 19).
#
# Mit `CONFIG_TAKT=y` baut das Modul den Port mit Faeden
# (`crates/takt-embed-c/ports/rtos`) mit dem Adapter, den `CONFIG_TAKT_PORT`
# waehlt (`ports/zephyr` oder `ports/cmsis-rtos2`), und stellt der Anwendung
# `takt_add_program` aus dem
# CMake-Paket (tools/cmake/TaktConfig.cmake), mit der Form `rtos` und dem
# Tripel aus `CONFIG_TAKT_TARGET` als Vorgabe; ein Programm mit Hardware-
# Konfiguration bekommt die Treiber aus dem Devicetree dazu (8.10):
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
  set(TAKT_ZEPHYR ON CACHE INTERNAL "die Treiber aus dem Devicetree (8.10): takt build --zephyr")
  include(${ZEPHYR_CURRENT_MODULE_DIR}/tools/cmake/TaktConfig.cmake)

  # Der Adapter des Ports mit Faeden, den `CONFIG_TAKT_PORT` waehlt.
  if(CONFIG_TAKT_PORT_CMSIS_RTOS2)
    set(adapter cmsis-rtos2)
    # `cmsis_os2.h` der CMSIS-Schicht, fuer den Adapter und die Anwendung.
    zephyr_include_directories(${ZEPHYR_BASE}/include/zephyr/portability)
  else()
    set(adapter zephyr)
  endif()
  set(sdk ${ZEPHYR_CURRENT_MODULE_DIR}/crates/takt-embed-c)
  zephyr_include_directories(${sdk}/include ${sdk}/ports/rtos ${sdk}/ports/${adapter})
  zephyr_library_named(takt_port)
  zephyr_library_sources(${sdk}/ports/rtos/takt_rtos.c ${sdk}/ports/${adapter}/takt_os.c)
  zephyr_library_link_libraries(Takt::sdk)

  # Das Geraet des Journals im Flash, fuer jeden Adapter.
  if(CONFIG_TAKT_JOURNAL_FLASH)
    zephyr_include_directories(${sdk}/ports/zephyr-nvm)
    zephyr_library_sources(${sdk}/ports/zephyr-nvm/takt_zephyr_nvm.c)
  endif()
endif()
