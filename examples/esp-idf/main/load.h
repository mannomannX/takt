/* load.h - die fremde Last des Wirts unter ESP-IDF (M11 Schritt 21b): Funk,
 * NVS und die Messung des automatischen Aussetzens, je nach `TAKT_LOAD` des
 * Baus (`quiet`, `wifi`, `ble`, `suspend`; main/CMakeLists.txt).
 *
 * Die Last ist fremder Code neben Takt (12.8, `shared`): Sie laeuft in
 * Aufgaben und ISRs von ESP-IDF, und was sie den Schritt kostet, misst die
 * Portpruefung an der Drift der Ticks. Jede Last nennt sich im Kopf des Laufs
 * mit einer Zeile `takt last <art>`, die der Harness prueft, und einer Zeile
 * fuer den Leser, die sagt, wie viel sie sendet und schreibt. Vor der Bilanz
 * zaehlt sie, was sie tat, seit sie steht; der Harness verlangt Mindestwerte
 * aus Dauer und Takt der Last (`takt_conformance::esp_idf::load_present`).
 */
#ifndef LOAD_H
#define LOAD_H

/* Startet die Last vor dem Lauf und wartet, bis sie steht: Funk sendete zum
 * ersten Mal. 0, oder -1, wenn ESP-IDF sie nicht anlegt oder sie nicht
 * anlaeuft. `suspend` misst dabei schon, was es misst (load_suspend.c). */
int load_start(void);

/* Schreibt die Zeilen der Last in den Kopf des Laufs, vor die Marke. */
void load_banner(void);

/* Schreibt vor der Bilanz, was die Last tat, seit sie steht: `takt last
 * dauer <ms>` und Paare `<zaehler> <n>`; ohne Zaehler nichts. Nur nach
 * einem `load_start`, das 0 gab. */
void load_report(void);

#endif /* LOAD_H */
