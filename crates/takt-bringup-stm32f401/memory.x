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

/* `.text` beginnt auf der naechsten 8-Byte-Grenze hinter der
 * Vektortabelle. `cortex-m-rt` legt es direkt dahinter (404 Byte, also
 * bei 0x08004194), aber die f64-Routinen aus `compiler_builtins`
 * (`__adddf3`, `__muldf3`, `__divdf3`) verlangen 8 Byte Ausrichtung —
 * jedes Programm mit f64 auf diesem Kern ohne f64-FPU zieht sie herein.
 * Der Linker richtete die Routinen selbst richtig aus und warnte nur
 * ueber den Anfang des Abschnitts; hier wird die Ursache behoben statt
 * die Warnung hingenommen.
 */
_stext = ALIGN(ADDR(.vector_table) + SIZEOF(.vector_table), 8);

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
