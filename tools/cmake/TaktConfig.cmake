# Takt als CMake-Paket (12.11, M11 Schritt 18a).
#
#   find_package(Takt REQUIRED)        # Takt_DIR nennt dieses Verzeichnis
#   takt_add_program(valve SOURCE takt/valve.takt FORM logical
#                    [HARDWARE takt/board.hw] [TARGET tripel] [BUILD sim|hw])
#   target_link_libraries(app PRIVATE valve)
#
# `takt_add_program` legt die Bibliothek `valve` an: die Lieferform von
# `takt build --emit embed` (Bibliothek, `valve.h`, `takt_sdk.h`, die Tabelle
# `valve_ops`) und dazu das SDK-Archiv `Takt::sdk` mit dem Kern ohne Warten.
# Das Paket hat keine eigene Logik: Es ruft das Werkzeug, wie der Bauhelfer
# von `takt-embed` es tut (FB-138), und baut nicht anders als `takt build`.
#
# Woher die Teile kommen:
#   TAKT_EXECUTABLE   das Werkzeug `takt` (Vorgabe: aus dem PATH)
#   TAKT_SDK_LIBRARY  das SDK-Archiv fuer das Ziel (`libtakt_sdk.a`,
#                     `takt_sdk.lib`), gebaut mit
#                     `cargo rustc --release --lib --crate-type staticlib
#                      --manifest-path crates/takt-embed-c/Cargo.toml
#                      [--target TRIPEL]`
#
# Das Ziel (`TARGET`) ist das Tripel des Wirts; ohne Angabe nimmt das Paket
# `TAKT_TARGET`, dann `CMAKE_C_COMPILER_TARGET`, und auf einem x86-64-Wirt
# unter Windows oder Linux dessen Tripel. Ohne `FORM` nimmt es `TAKT_FORM`.
# Beide setzt das Zephyr-Modul (`zephyr/`) aus seiner Konfiguration. `BUILD`
# geht als `--build` an das Werkzeug (8.1); ohne Angabe baut es `hw`.
#
# Unter Zephyr (`TAKT_ZEPHYR`, vom Modul gesetzt) bekommt ein Programm mit
# `HARDWARE` die Treiber aus dem Devicetree (8.10): `takt build --zephyr`
# schreibt `<name>_zephyr.c` und `<name>_zephyr.h`, und die Anwendung `app`
# uebersetzt die Quelle mit den Koepfen von Zephyr. Nicht in einer eigenen
# Bibliothek: Eine, die erst die Anwendung anlegt, haengt nicht an den Koepfen,
# die der Bau von Zephyr erzeugt (`zephyr_generated_headers`).

include_guard(GLOBAL)

find_program(TAKT_EXECUTABLE takt DOC "das Werkzeug takt (11.1)")
if(NOT TAKT_EXECUTABLE)
  message(FATAL_ERROR "Takt: `takt` nicht gefunden; TAKT_EXECUTABLE nennt es")
endif()

set(TAKT_SDK_LIBRARY "" CACHE FILEPATH "das SDK-Archiv fuer das Ziel (crates/takt-embed-c)")
if(NOT TAKT_SDK_LIBRARY)
  message(FATAL_ERROR "Takt: TAKT_SDK_LIBRARY nennt kein SDK-Archiv (`cargo rustc --release --lib "
                      "--crate-type staticlib --manifest-path crates/takt-embed-c/Cargo.toml`)")
endif()
if(NOT TARGET Takt::sdk)
  add_library(Takt::sdk STATIC IMPORTED GLOBAL)
  set_target_properties(Takt::sdk PROPERTIES IMPORTED_LOCATION "${TAKT_SDK_LIBRARY}")
endif()

# Das Tripel des Wirts, wenn `takt_add_program` keines nennt.
function(_takt_host_triple out)
  if(TAKT_TARGET)
    set(${out} "${TAKT_TARGET}" PARENT_SCOPE)
  elseif(CMAKE_C_COMPILER_TARGET)
    set(${out} "${CMAKE_C_COMPILER_TARGET}" PARENT_SCOPE)
  elseif(WIN32 AND CMAKE_SIZEOF_VOID_P EQUAL 8)
    set(${out} "x86_64-pc-windows-msvc" PARENT_SCOPE)
  elseif(CMAKE_SYSTEM_NAME STREQUAL "Linux" AND CMAKE_SYSTEM_PROCESSOR MATCHES "^(x86_64|AMD64)$")
    set(${out} "x86_64-unknown-linux-gnu" PARENT_SCOPE)
  else()
    message(FATAL_ERROR "Takt: kein Tripel fuer diesen Wirt; `TARGET` an takt_add_program nennen")
  endif()
endfunction()

function(takt_add_program name)
  cmake_parse_arguments(PARSE_ARGV 1 ARG "" "SOURCE;FORM;HARDWARE;TARGET;BUILD" "")
  set(form "${ARG_FORM}")
  if(NOT form)
    set(form "${TAKT_FORM}")
  endif()
  if(NOT ARG_SOURCE OR NOT form)
    message(FATAL_ERROR "takt_add_program(${name}): SOURCE und FORM fehlen (12.11)")
  endif()
  get_filename_component(source "${ARG_SOURCE}" ABSOLUTE)
  set(triple "${ARG_TARGET}")
  if(NOT triple)
    _takt_host_triple(triple)
  endif()
  set(out "${CMAKE_CURRENT_BINARY_DIR}/takt/${name}")
  file(MAKE_DIRECTORY "${out}")
  if(triple MATCHES "-msvc$")
    set(lib "${out}/${name}.lib")
  else()
    set(lib "${out}/lib${name}.a")
  endif()
  set(args build "${source}" --emit embed --form "${form}" --target "${triple}" --prefix "${name}"
           --out "${out}")
  if(ARG_BUILD)
    list(APPEND args --build "${ARG_BUILD}")
  endif()
  set(depends "${source}")
  set(outputs "${lib}" "${out}/${name}.h" "${out}/takt_sdk.h")
  set(devicetree OFF)
  if(ARG_HARDWARE)
    get_filename_component(hardware "${ARG_HARDWARE}" ABSOLUTE)
    list(APPEND args --hardware "${hardware}")
    list(APPEND depends "${hardware}")
    if(TAKT_ZEPHYR)
      set(devicetree ON)
      list(APPEND args --zephyr)
      list(APPEND outputs "${out}/${name}_zephyr.c" "${out}/${name}_zephyr.h")
    endif()
  endif()
  add_custom_command(
    OUTPUT ${outputs}
    COMMAND "${TAKT_EXECUTABLE}" ${args}
    DEPENDS ${depends}
    COMMENT "takt build ${name} (${form}, ${triple})"
    VERBATIM)
  add_custom_target(${name}_takt DEPENDS "${lib}")
  # Der erzeugte Code ruft `round`, `fma` und `fmaf`: unter Linux aus
  # `libm`, unter Windows aus der C-Laufzeit (FB-526).
  set(libs Takt::sdk)
  if(UNIX AND NOT APPLE)
    list(APPEND libs m)
  endif()
  add_library(${name} STATIC IMPORTED GLOBAL)
  set_target_properties(${name} PROPERTIES
    IMPORTED_LOCATION "${lib}"
    INTERFACE_INCLUDE_DIRECTORIES "${out}"
    INTERFACE_LINK_LIBRARIES "${libs}")
  add_dependencies(${name} ${name}_takt)
  if(devicetree)
    target_sources(app PRIVATE "${out}/${name}_zephyr.c")
  endif()
endfunction()
