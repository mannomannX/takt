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
 * nichts. */

/* Vor dem Lauf: `takt wecker`, wenn ein Faden mit Frist aufwacht, also der
 * Interrupt des Systemzeitgebers kommt. */
void line_watch_alarm(void);

/* Nach `takt_rtos_start`: `takt lauf`. */
void line_watch_start(void);

/* Aus dem `kick` des Watchdogs, nach jedem Schritt des Kerns. */
void line_watch_step(void);

/* Aus der Schleife von `main` waehrend des Laufs: `takt erster schritt`,
 * sobald der Kern einen meldete, und eine Sekunde nach dem Start, wie viele
 * es waren. */
void line_watch_round(void);

#endif /* LINE_H */
