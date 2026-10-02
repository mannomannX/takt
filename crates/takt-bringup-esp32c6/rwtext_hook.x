/* RAM-Residenz des Takt-Programms (12.3, `xip_flash`).
 *
 * `esp-hal` zieht diese Datei in `.rwtext`, wenn
 * ESP_HAL_CONFIG_USE_RWTEXT_LD_HOOK gesetzt ist (build.rs). Der Tick-Pfad
 * in Rust traegt `#[ram]`; der erzeugte Takt-Code und sein C-Rahmen
 * koennen das nicht und werden hier ueber ihr Archiv benannt — samt der
 * Konstanten, die der Tick liest (DFA- und `const`-Tabellen, `safe`).
 */
*libtaktprogramm.a:(.text .text.* .rodata .rodata.* .srodata .srodata.*)

/* Die Mathematik, die der erzeugte Code ruft (FB-301): Die Grundrechenarten
 * in Soft-Float liegen im ROM des Chips, `fma` und `sqrt` kommen aus
 * `compiler_builtins` und laegen sonst im Flash hinter dem Cache. Der
 * Linker behaelt davon nur, was gerufen wird.
 */
*libcompiler_builtins-*.rlib:*(.text .text.* .rodata .rodata.* .srodata .srodata.*)

/* Der Rest des Tick-Pfads in Rust (12.3, FB-357): die Schleife
 * (`takt-rt-core`, `takt-rt-baremetal`), die Bindung des Programms
 * (`takt-mcu-program`), der Treiberrand (`takt_edge_*`, `takt-hal`) und die
 * Bibliotheksroutinen, die der erzeugte Code ruft — die korrekt gerundete
 * Mathematik (`takt_m_*`, `libtaktm`) und die kuratierten Natives
 * (`takt_native_*`, `takt-native`, `takt-native-abi`). Das Bring-up baut mit
 * LTO, also steht keines davon in einem eigenen Archiv; ihre Abschnitte
 * tragen die Namen ihrer Symbole, auch die Monomorphisierungen, die eine
 * Implementierung des Boards einschliessen. Was nur das Board-Crate
 * enthaelt, traegt dort `#[ram]`. Die Rechnung der Job-Natives
 * (`takt-crypto`) laeuft ausserhalb des Ticks (4.5) und bleibt im Flash.
 */
*(.text.takt_m_* .text.takt_native_* .text.takt_edge_*)
*(.text.*8libtaktm* .text.*11takt_native* .text.*15takt_native_abi* .text.*8takt_hal*)
*(.text.*12takt_rt_core* .text.*17takt_rt_baremetal* .text.*16takt_mcu_program*)

/* Ihre Konstanten: die Tabellen, `static` mit Namen, und was der Compiler
 * namenlos ablegt (`.Lanon`), wenn eine Konstante per Referenz uebergeben
 * wird — etwa `Big::ONE` in `libtaktm`.
 */
*(.rodata.*8libtaktm* .srodata.*8libtaktm* .rodata.*11takt_native* .srodata.*11takt_native*)
*(.rodata.*12takt_rt_core* .srodata.*12takt_rt_core* .rodata.*17takt_rt_baremetal* .srodata.*17takt_rt_baremetal*)
*(.rodata..Lanon.* .srodata..Lanon.*)
