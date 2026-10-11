/* Das Sprungbrett der WeAct Black Pill (STM32F401CCU6, FB-385).
 *
 * Der HID-Bootloader in Sektor 0 springt nach 0x0800_4000. Dort, in
 * Sektor 1, liegt dieses Programm; Sektor 2 und 3 haelt das Journal, die
 * Anwendung beginnt in Sektor 4 (`memory.x` des Bring-ups).
 */
MEMORY
{
  FLASH : ORIGIN = 0x08004000, LENGTH = 16K
  RAM   : ORIGIN = 0x20000000, LENGTH = 64K
}
