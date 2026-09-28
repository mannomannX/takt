/* Der Programmzustand als eigener Abschnitt am Anfang des RAM (12.3).
 *
 * Eine MPU-Region muss an ihrer Groesse ausgerichtet sein; ORIGIN(RAM)
 * ist es fuer jede. Der Abschnitt steht darum vor `.data` und reicht bis
 * zum Ende der geschuetzten Achtel seiner Region, damit dort nichts
 * anderes liegt, das ausserhalb des Ticks beschrieben wird. Die Groesse
 * `__takt_state_size` rechnet `build.rs` aus dem Rahmen
 * (`takt_board_support::mpu::Region`).
 *
 * NOLOAD: Das Bring-up nullt den Abschnitt selbst
 * (`takt_board_stm32f401::mpu::clear_state`), der Startcode nullt nur
 * `.bss`.
 */
SECTIONS
{
  .takt_state (NOLOAD) : ALIGN(8)
  {
    __takt_state_start = .;
    KEEP(*(.takt_state));
    __takt_state_used = .;
    . = __takt_state_start + __takt_state_size;
    __takt_state_end = .;
  } > RAM
} INSERT BEFORE .data;

ASSERT(__takt_state_start == ORIGIN(RAM), "der Programmzustand muss am Anfang des RAM liegen");
ASSERT(__takt_state_used <= __takt_state_end, "der Programmzustand passt nicht in seine Region");
