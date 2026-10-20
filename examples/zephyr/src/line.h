/* line.h - die Leitung des Wirts (12.2, 12.5) auf dem UART-Geraet, das der
 * Devicetree als `takt,line` waehlt. */
#ifndef LINE_H
#define LINE_H

#include "takt_sdk.h"

/* Richtet das Geraet ein und fuellt `line` mit seinen Rueckrufen; 0 oder
 * -ENODEV, wenn das Geraet nicht bereit ist. */
int line_open(struct takt_line *line);

/* TODO(FB-757): Vermerke fuer die Suche nach dem Stillstand des C6 unter
 * Zephyr; fort, sobald die Ursache behoben ist. Sie gehen nur an eine
 * Konsole neben der Leitung (auf dem C6 UART0), damit der Trace auf der
 * Leitung bleibt, wie das Urteil ihn liest; ohne eine solche tun sie
 * nichts. Waehrend des Laufs schreiben sie nichts: `printk` auf UART0 haelt
 * den Rufenden Millisekunden lang auf und machte Ueberlaeufe. */

/* Vor dem Kern: `takt wecker`, wenn ein Faden mit Frist aufwacht, also der
 * Interrupt des Systemzeitgebers kommt. */
void line_watch_alarm(void);

/* Vor `takt_rtos_start`, nach dem Kern: `takt start`. Mit `ticking` laeuft
 * dazu ein stiller Zeitgeber alle 100 ms, ohne etwas zu schreiben: Haengt
 * der Lauf ohne ihn und laeuft mit ihm, verlor der Systemzeitgeber eine
 * Frist, und erst die naechste Frist eines anderen weckte. */
void line_watch_arm(int ticking);

/* Am Ende des Laufs: `takt ende: <what>` und wie viele Byte der Ring der
 * Leitung noch haelt. */
void line_watch_end(const char *what);

#endif /* LINE_H */
