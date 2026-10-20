/* Die Arenen zweier Programme am Anfang des RAM (12.3, 12.11; M11 Schritt 16).
 *
 * Wie `takt_state.x`, fuer das Binary `pair`: Die Arena von `fast` liegt in
 * `.takt_state` am Anfang des RAM, die von `slow` dahinter in
 * `.takt_state2`. Jede ist eine Region der MPU und darum an deren Groesse
 * ausgerichtet; `build.rs` setzt die geschuetzten Teile
 * (`__takt_state_size`, `__takt_state2_size`) und die Ausrichtung der
 * zweiten (`__takt_state2_align`). Die Runtime jeder Arena folgt ihrem
 * geschuetzten Teil ungeschuetzt.
 */
SECTIONS
{
  .takt_state (NOLOAD) : ALIGN(8)
  {
    __takt_state_start = .;
    KEEP(*(.takt_state));
    __takt_state_end = __takt_state_start + __takt_state_size;
    . = MAX(., __takt_state_end);
  } > RAM
  .takt_state2 (NOLOAD) : ALIGN(__takt_state2_align)
  {
    __takt_state2_start = .;
    KEEP(*(.takt_state2));
    __takt_state2_end = __takt_state2_start + __takt_state2_size;
    . = MAX(., __takt_state2_end);
  } > RAM
} INSERT BEFORE .data;

ASSERT(__takt_state_start == ORIGIN(RAM), "die Arena von fast muss am Anfang des RAM liegen");
ASSERT(__takt_state2_start % __takt_state2_align == 0, "die Arena von slow muss an ihrer Region ausgerichtet sein");
