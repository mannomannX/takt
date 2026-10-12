/* Speicherkarte der WeAct Black Pill (STM32F401CCU6) fuer einen C-Wirt
 * (M11 Schritt 18b), dieselbe wie die des Bring-ups in Rust
 * (`crates/takt-bringup-stm32f401/memory.x`): Sektor 0 haelt der
 * HID-Bootloader, Sektor 1 das Sprungbrett, Sektor 2 und 3 das Journal
 * (FB-385). Die Anwendung beginnt in Sektor 4 mit ihrer Vektortabelle.
 *
 * Der Stack waechst vom Ende des RAM nach unten bis zum Waechter, 32 Byte
 * an einer 32-Byte-Grenze ueber `.bss` (12.3). Dazwischen muss Platz fuer
 * `board_stack_bytes` sein: Schritt und Jobs rechnen in der Interruptform
 * auf diesem Stack, also `P_TICK_STACK_BYTES + P_JOB_STACK_BYTES`; den Wert
 * setzt der Bau (`--defsym`).
 */
MEMORY
{
  FLASH (rx) : ORIGIN = 0x08010000, LENGTH = 192K
  RAM (rwx)  : ORIGIN = 0x20000000, LENGTH = 64K
}

ENTRY(Reset_Handler)

SECTIONS
{
  .vectors :
  {
    board_vectors = .;
    KEEP(*(.vectors))
  } > FLASH

  .text :
  {
    *(.text .text.*)
  } > FLASH

  .rodata : ALIGN(4)
  {
    *(.rodata .rodata.*)
  } > FLASH

  .data : ALIGN(8)
  {
    board_data_start = .;
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

  /* Abwickeln gibt es nicht: Das SDK bricht bei einer Panik ab, der Wirt
   * ist C ohne Ausnahmen. */
  /DISCARD/ :
  {
    *(.ARM.exidx .ARM.exidx.* .ARM.extab .ARM.extab.*)
  }
}

board_stack_top = ORIGIN(RAM) + LENGTH(RAM);

ASSERT(board_stack_top - board_stack_bottom >= board_stack_bytes,
       "der Stack ist kleiner als P_TICK_STACK_BYTES + P_JOB_STACK_BYTES (12.3)");
