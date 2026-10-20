# Der Wirt unter ESP-IDF (main.c, line.c) und sein Programm (12.11, M11
# Schritt 21a). Das Programm heisst `program`: `app` ist ein Ziel von
# ESP-IDF selbst.
#
# Das Programm ist das Pruefprogramm der Portpruefung, wenn der Bau kein
# anderes nennt (`TAKT_PROGRAM`), im Sim-Build, wie `takt port-test --emit
# embed` es liefert: Die Ausgaenge gehen ohne Treiber in den Trace. Die
# Hardware-Konfiguration ist die des C6 unter ESP-IDF
# (`corpus-try/hw/esp32c6-idf.hw`): Das Journal haelt dort den Kern nicht
# an.
#
# `TAKT_LOAD` waehlt die fremde Last neben Takt (load.h, Schritt 21b): `quiet`
# keine, `wifi` WLAN und NVS, `ble` BLE und NVS, `suspend` die Messung des
# automatischen Aussetzens vor dem Lauf. WLAN und BLE brauchen ihre
# Konfiguration dazu (`sdkconfig.ble`), die der Bau als `SDKCONFIG_DEFAULTS`
# nennt. `TAKT_LOGICAL` faehrt den Lauf in logischer Zeit (13.8).
#
# Was im Schritt laeuft, liegt im RAM: die Komponente `takt` und das
# SDK-Archiv (`esp-idf/takt/takt.lf`), das Programm (sein Fragment `program.lf`),
# die Leitung (`host.lf`) und der Watchdog des Wirts (`IRAM_ATTR`).

set(root ${CMAKE_CURRENT_LIST_DIR}/../../..)
set(program_lf ${CMAKE_CURRENT_BINARY_DIR}/takt/program/program.lf)

idf_build_get_property(load TAKT_LOAD)
set(load_srcs)
set(load_requires)
if(load STREQUAL "quiet")
  set(load_srcs load_quiet.c)
elseif(load STREQUAL "wifi")
  set(load_srcs load_wifi.c load_nvs.c)
  set(load_requires esp_wifi esp_event nvs_flash)
elseif(load STREQUAL "ble")
  set(load_srcs load_ble.c load_nvs.c)
  set(load_requires bt nvs_flash)
elseif(load STREQUAL "suspend")
  set(load_srcs load_suspend.c)
  set(load_requires spi_flash esp_partition esp_timer)
else()
  message(FATAL_ERROR "TAKT_LOAD: `${load}` ist keine Last (quiet, wifi, ble, suspend)")
endif()

idf_component_register(
  SRCS main.c line.c ${load_srcs}
  INCLUDE_DIRS .
  REQUIRES takt esp_hal_usb esp_hal_wdt esp_hw_support esp_system freertos ${load_requires}
  LDFRAGMENTS host.lf ${program_lf})

if(CMAKE_BUILD_EARLY_EXPANSION)
  return()
endif()

set(TAKT_PROGRAM "${root}/corpus-try/124_port_test.takt" CACHE FILEPATH "das Programm des Wirts")
set(TAKT_TICKS 2000 CACHE STRING "so viele Ticks faehrt der Lauf; 0: bis das Programm endet")
option(TAKT_FRESH_JOURNAL "das Journal vor dem Lauf loeschen" OFF)
option(TAKT_LOGICAL "der Lauf in logischer Zeit" OFF)

takt_add_program(program SOURCE "${TAKT_PROGRAM}" HARDWARE "${root}/corpus-try/hw/esp32c6-idf.hw" BUILD sim LDGEN)
target_link_libraries(${COMPONENT_LIB} PRIVATE program)
if(TAKT_FRESH_JOURNAL)
  set(fresh 1)
else()
  set(fresh 0)
endif()
if(TAKT_LOGICAL)
  set(logical 1)
else()
  set(logical 0)
endif()
target_compile_definitions(${COMPONENT_LIB} PRIVATE TAKT_TICKS=${TAKT_TICKS}u TAKT_FRESH_JOURNAL=${fresh}
                                                    TAKT_LOGICAL=${logical})
