/* line.h - die Leitung des Wirts unter ESP-IDF (12.2, `struct takt_line`):
 * der USB-Serial-JTAG des C6, ohne den Treiber von ESP-IDF, mit einer
 * eigenen IRAM-ISR.
 *
 * **Ohne zu warten** (12.2): `try_write` legt ein Byte in ein FIFO, wenn Platz
 * ist, und die ISR gibt es paketweise an den Sender. Den Ring davor, das
 * Zaehlen und das Verwerfen macht das SDK. Der Schritt ruft `try_write` in seiner
 * IRAM-ISR, auch waehrend ESP-IDF auf den Flash zugreift; das Fragment
 * `host.lf` legt diese Datei darum in den RAM, und die ISR der Leitung ist
 * als IRAM-sicher angemeldet.
 *
 * **Die Gegenrichtung liest dieselbe ISR.** Steht `TAKT` darin, verlangt der
 * Host das Board zurueck: Der Wirt startet neu, sobald er es sieht, und
 * `probe-rs` schreibt das naechste Abbild ueber JTAG.
 */
#ifndef LINE_H
#define LINE_H

#include "takt_sdk.h"

/* Die Stufe der ISR der Leitung: ueber der des Schritts, damit sie den Ring
 * leert, waehrend der Schritt rechnet. */
#define LINE_PRIORITY 3U

/* Richtet die ISR ein und fuellt `line`; 0, oder -1, wenn ESP-IDF die ISR
 * nicht anlegt. */
int line_open(struct takt_line *line);

/* Ob der Host das Board zurueckverlangt hat. */
int line_handback(void);

#endif /* LINE_H */
