/* Speicherkarte der Black Pill fuer den C-Wirt unter FreeRTOS-MPU (M11
 * Schritt 20b), wie `stm32f401.ld` und dazu die Abschnitte, die der Port
 * `ARM_CM4_MPU` von FreeRTOS verlangt.
 *
 * **Die acht Regionen der MPU gehoeren FreeRTOS** (12.11). Fest: 7 die
 * Daten des Kerns (`privileged_data`, nur privilegiert), 6 sein Code mit
 * der Vektortabelle (`privileged_functions`, nur privilegiert), 5 der ganze
 * Flash (lesbar fuer jede Aufgabe), 4 die Peripherie. Je Aufgabe: 3 ihr
 * Stack, 0 bis 2 was der Wirt ihr gibt. Eine Region ist eine Zweierpotenz
 * gross und an ihrer Groesse ausgerichtet; die Groessen unten sind es, und
 * der Linker bricht ab, wenn der Inhalt nicht hineinpasst.
 *
 * Privilegierter Code schreibt ueberall, wo keine Region es verbietet
 * (PRIVDEFENA); eine Aufgabe ohne Privileg nur in ihre Regionen. Die Arena
 * liegt in keiner davon (`Schutz: Partition`).
 *
 * Der Stack der Interrupts waechst vom Ende des RAM nach unten bis zu seinem
 * Waechter, 32 Byte an einer 32-Byte-Grenze ueber `.bss` (12.3). Den
 * Waechter haelt ein Watchpoint der DWT statt einer Region
 * (`board_watch_stack`).
 */
MEMORY
{
  FLASH (rx) : ORIGIN = 0x08010000, LENGTH = 192K
  RAM (rwx)  : ORIGIN = 0x20000000, LENGTH = 64K
}

ENTRY(Reset_Handler)

/* Region 6: Vektortabelle und Code des Kerns am Anfang der Anwendung. */
board_privileged_flash_bytes = 32K;
/* Region 7: die Daten des Kerns am Anfang des RAM. */
board_privileged_ram_bytes = 1K;

__privileged_functions_start__ = ORIGIN(FLASH);
__privileged_functions_end__ = ORIGIN(FLASH) + board_privileged_flash_bytes;
__privileged_data_start__ = ORIGIN(RAM);
__privileged_data_end__ = ORIGIN(RAM) + board_privileged_ram_bytes;

/* Region 5: der ganze Flash des STM32F401CC, 256 KiB ab 0x08000000. */
__FLASH_segment_start__ = 0x08000000;
__FLASH_segment_end__ = 0x08040000;

/* Region 3 einer Aufgabe ohne eigene Regionen: das ganze RAM. Solche
 * Aufgaben sind hier nur privilegierte (Leerlauf, Wirt, Schritt, Jobs). */
__SRAM_segment_start__ = ORIGIN(RAM);
__SRAM_segment_end__ = ORIGIN(RAM) + LENGTH(RAM);

SECTIONS
{
  .privileged_functions :
  {
    board_vectors = .;
    KEEP(*(.vectors))
    *(privileged_functions)
    . = ALIGN(board_privileged_flash_bytes);
  } > FLASH

  /* Die Hueller der Systemaufrufe: Nur von hier darf eine Aufgabe ohne
   * Privileg den Kern rufen (`configENFORCE_SYSTEM_CALLS_FROM_KERNEL_ONLY`). */
  .freertos_system_calls : ALIGN(4)
  {
    __syscalls_flash_start__ = .;
    *(freertos_system_calls)
    __syscalls_flash_end__ = .;
  } > FLASH

  .text :
  {
    *(.text .text.*)
  } > FLASH

  .rodata : ALIGN(4)
  {
    *(.rodata .rodata.*)
  } > FLASH

  /* Die Daten des Kerns zuerst und mit Anfangswerten: Der Start kopiert sie
   * mit `.data` (`startup.c`). */
  .data : ALIGN(8)
  {
    board_data_start = .;
    *(privileged_data)
    . = ALIGN(board_privileged_ram_bytes);
    board_privileged_data_end = .;
    *(.data .data.*)
    . = ALIGN(4);
    board_data_end = .;
  } > RAM AT > FLASH
  board_data_load = LOADADDR(.data);

  .bss (NOLOAD) : ALIGN(8)
  {
    board_bss_start = .;
    *(.bss .bss.* COMMON)
    . = ALIGN(4);
    board_bss_end = .;
  } > RAM

  .stack (NOLOAD) : ALIGN(32)
  {
    board_stack_guard = .;
    . += 32;
    board_stack_bottom = .;
  } > RAM

  /DISCARD/ :
  {
    *(.ARM.exidx .ARM.exidx.* .ARM.extab .ARM.extab.*)
  }
}

board_stack_top = ORIGIN(RAM) + LENGTH(RAM);

ASSERT(ADDR(.privileged_functions) == __privileged_functions_start__ &&
       SIZEOF(.privileged_functions) == board_privileged_flash_bytes,
       "Vektortabelle und Code des Kerns passen nicht in Region 6");
ASSERT(ADDR(.data) == __privileged_data_start__ && board_privileged_data_end == __privileged_data_end__,
       "die Daten des Kerns passen nicht in Region 7");
ASSERT(board_stack_top - board_stack_bottom >= board_stack_bytes,
       "der Stack der Interrupts ist kleiner als board_stack_bytes (12.3)");
