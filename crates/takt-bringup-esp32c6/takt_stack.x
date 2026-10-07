/* Der Hauptstack ist der Schritt-Stack (12.3): Er reicht von
 * `_stack_start_cpu0` hinunter bis ueber das Waechterwort `__stack_chk_guard`,
 * das `esp-hal` mit einem Watchpoint bewacht. Dazwischen muss
 * `TICK_STACK_BYTES` Platz haben: das Programm, die Reserve des Ports und die
 * Marge, die `build.rs` aus dem Manifest als `__takt_tick_stack_bytes` setzt.
 */
ASSERT(_stack_start_cpu0 - (__stack_chk_guard + 4) >= __takt_tick_stack_bytes,
       "der Hauptstack ist kleiner als TICK_STACK_BYTES (12.3)");
