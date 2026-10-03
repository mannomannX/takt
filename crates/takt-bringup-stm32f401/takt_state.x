/* Die Arena als eigener Abschnitt am Anfang des RAM (12.3, 12.11).
 *
 * Eine MPU-Region muss an ihrer Groesse ausgerichtet sein; ORIGIN(RAM)
 * ist es fuer jede. Der Abschnitt steht darum vor `.data`. Die Region
 * deckt den Programmbereich vorn in der Arena, `__takt_state_size` Byte,
 * die `build.rs` aus der Arena rechnet (`takt_board_support::mpu::Region`);
 * der Rahmen fuellt den Programmbereich bis dorthin auf, die Runtime folgt
 * dahinter ungeschuetzt. Ein Binary ohne Programm (`natives`, `blink`)
 * bindet keine Arena; der Abschnitt reicht trotzdem bis ans Ende der
 * Region, sonst laege `.data` darin.
 *
 * Die Arena legt das Bring-up hierher (`#[link_section]`). NOLOAD: `P_init`
 * beschreibt die ganze Arena, bevor sie gelesen wird; der Startcode muss
 * sie nicht nullen.
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
} INSERT BEFORE .data;

ASSERT(__takt_state_start == ORIGIN(RAM), "die Arena muss am Anfang des RAM liegen");
