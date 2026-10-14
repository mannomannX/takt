/* line.h - die Leitung des Wirts unter ESP-IDF (12.2, `struct takt_line`):
 * der USB-Serial-JTAG des C6, ohne den Treiber von ESP-IDF und ohne
 * Interrupt, nach dem Vertrag des Bring-ups in Rust (line.c).
 *
 * **Ohne zu warten** (12.2): `try_write` legt ein Byte ins FIFO des
 * USB-Serial-JTAG, wenn es frei ist, `flush` schickt das Paket ab. Den Ring
 * davor, das Zaehlen und das Verwerfen macht das SDK. Der Schritt ruft beide
 * in seiner IRAM-ISR, auch waehrend ESP-IDF auf den Flash zugreift; das
 * Fragment `host.lf` legt diese Datei darum in den RAM.
 *
 * **Die Gegenrichtung liest `flush` mit.** Steht `TAKT` darin, verlangt der
 * Host das Board zurueck, und der Chip setzt sich auf RTC-Ebene zurueck: Das
 * USB-Geraet meldet sich ab und neu an. Ebenso, wenn der Host den Wunsch
 * ueber JTAG in LP_AON STORE0 schrieb (`line_open`).
 */
#ifndef LINE_H
#define LINE_H

#include "takt_sdk.h"

/* Fuellt `line`; vorher setzt sich der Chip zurueck, wenn der Host es
 * verlangt hat. */
void line_open(struct takt_line *line);

#endif /* LINE_H */
