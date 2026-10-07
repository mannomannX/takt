/* Der Hauptstack ist der Schritt-Stack (12.3): Er reicht von `_stack_start`
 * hinunter bis zum Waechter der MPU, 32 Byte an der naechsten 32-Byte-Grenze
 * ueber `_stack_end` (`mpu::stack_floor`). Dazwischen muss `TICK_STACK_BYTES`
 * Platz haben: das Programm, die Reserve des Ports und die Marge, die
 * `build.rs` aus dem Manifest als `__takt_tick_stack_bytes` setzt.
 */
ASSERT(_stack_start - (ALIGN(_stack_end, 32) + 32) >= __takt_tick_stack_bytes,
       "der Hauptstack ist kleiner als TICK_STACK_BYTES (12.3)");
