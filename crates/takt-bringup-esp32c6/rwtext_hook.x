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
