/* Speicherkarte der WeAct Black Pill (STM32F401CCU6).
 *
 * **Der Versatz ist nicht Zierde.** Das Board wird mit einem
 * HID-Bootloader ausgeliefert, der die ersten Flash-Seiten belegt und
 * die Anwendung dahinter erwartet. Wer an 0x0800_0000 linkt,
 * ueberschreibt ihn beim ersten Flashen — und danach geht nur noch SWD.
 *
 * Der WeAct-Bootloader belegt 16 KiB. Wird spaeter ueber SWD geflasht
 * (mit `probe-rs`), kann der Versatz entfallen; dann steht hier
 * 0x0800_0000 und die vollen 256 KiB.
 *
 * Die Zahlen gehen in `takt size` (11.5) ein: Fuer `baremetal`-Profile
 * ist die Summe gegen `ram` und `flash` der Hardware-Konfiguration zu
 * pruefen (8.10), und diese Datei ist die Quelle dafuer.
 */
MEMORY
{
  /* 256 KiB gesamt, davon 16 KiB fuer den Bootloader. */
  FLASH : ORIGIN = 0x08004000, LENGTH = 240K
  RAM   : ORIGIN = 0x20000000, LENGTH = 64K
}

/* Der Stack waechst von oben nach unten. `_stack_start` ist sein
 * oberes Ende; `cortex-m-rt` setzt den Stackzeiger darauf.
 */
_stack_start = ORIGIN(RAM) + LENGTH(RAM);

/* **Zur Ausrichtungswarnung des Linkers.** Er meldet, dass `.text` bei
 * 0x08004194 nicht auf 8 ausgerichtet ist — die Vektortabelle ist 404
 * Byte lang, und 404 teilt nicht durch 8. Das ist folgenlos: Thumb-Code
 * braucht 2 Byte Ausrichtung, Sprungtabellen 4, und beides ist erfuellt.
 *
 * Die 8 fordert der C-Rahmen (12.1) fuer seine statischen Puffer, weil
 * Prozessabbild und Latch `long long` enthalten. Die liegen aber in
 * `.bss`, wo der Linker sie ohnehin ausrichtet — nicht in `.text`.
 */

/* Der Schutzbereich aus 12.3.
 *
 * Das Kanarienwort steht als gewoehnliche `static` im Programm, nicht
 * hier: Eine Adresse im Linker-Skript waere eine zweite Quelle fuer
 * dieselbe Angabe, und sie zu lesen braeuchte einen rohen Zeiger. Was
 * das Skript beitraegt, ist die *Reihenfolge* — `.uninit` liegt am
 * unteren Ende des RAM, also unter allem, was der Stack erreicht, wenn
 * er ueberlaeuft.
 *
 * Die MPU-Variante (12.3 nennt sie zuerst) braucht eine ausgerichtete
 * Region und kommt, wenn der Bring-up steht; bis dahin traegt das
 * Kanarienwort dieselbe Aussage mit einer Tickperiode Verzoegerung.
 */
