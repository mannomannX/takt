/* line.h - die Leitung des Wirts (12.2, 12.5) auf dem UART-Geraet, das der
 * Devicetree als `takt,line` waehlt. */
#ifndef LINE_H
#define LINE_H

#include "takt_sdk.h"

/* Richtet das Geraet ein und fuellt `line` mit seinen Rueckrufen; 0 oder
 * -ENODEV, wenn das Geraet nicht bereit ist. */
int line_open(struct takt_line *line);

#endif /* LINE_H */
