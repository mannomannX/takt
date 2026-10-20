/* load.h - die fremde Last des Wirts unter ESP-IDF (M11 Schritt 21b): Funk,
 * NVS und die Messung des automatischen Aussetzens, je nach `TAKT_LOAD` des
 * Baus (`quiet`, `wifi`, `ble`, `suspend`; main/CMakeLists.txt).
 *
 * Die Last ist fremder Code neben Takt (12.8, `shared`): Sie laeuft in
 * Aufgaben und ISRs von ESP-IDF, und was sie den Schritt kostet, misst die
 * Portpruefung an der Drift der Ticks. Jede Last nennt sich im Kopf des Laufs
 * mit einer Zeile `takt last <art>`, die der Harness prueft, und einer Zeile
 * fuer den Leser, die sagt, wie viel sie sendet und schreibt.
 */
#ifndef LOAD_H
#define LOAD_H

/* Startet die Last vor dem Lauf; 0, oder -1, wenn ESP-IDF sie nicht anlegt.
 * `suspend` misst dabei schon, was es misst (load_suspend.c). */
int load_start(void);

/* Schreibt die Zeilen der Last in den Kopf des Laufs, vor die Marke. */
void load_banner(void);

#endif /* LOAD_H */
