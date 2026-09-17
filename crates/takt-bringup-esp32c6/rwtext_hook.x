/* RAM-Residenz des Takt-Programms (12.3, `xip_flash`).
 *
 * `esp-hal` zieht diese Datei in `.rwtext`, wenn
 * ESP_HAL_CONFIG_USE_RWTEXT_LD_HOOK gesetzt ist (build.rs). Der Tick-Pfad
 * in Rust traegt `#[ram]`; der erzeugte Takt-Code und sein C-Rahmen
 * koennen das nicht und werden hier ueber ihr Archiv benannt — samt der
 * Konstanten, die der Tick liest (DFA- und `const`-Tabellen, `safe`).
 */
*libtaktprogramm.a:(.text .text.* .rodata .rodata.* .srodata .srodata.*)
