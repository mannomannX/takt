/* handback.h - das Board zurueck an den Host (FB-275): Schreibt der Host
 * `TAKT` auf die Leitung, gibt die Anwendung das Board ab, damit er das
 * naechste Abbild ohne Hand schreibt. Was das heisst, haengt am Board: der
 * F401 startet mit dem Wunsch nach dem DFU-Bootloader des ROM neu, und das
 * Sprungbrett springt hinein (handback_stm32f401.c); jedes andere startet
 * neu (handback_reboot.c). Den Wunsch des Hosts nimmt handback.c an. */
#ifndef HANDBACK_H
#define HANDBACK_H

#include <zephyr/toolchain.h>

/* Der Host hat `TAKT` geschrieben; aus der ISR der Leitung. */
void handback_wish(void);

/* Gibt das Board ab, wenn der Host es verlangt hat; sonst kehrt sie zurueck.
 * Aus einem Faden, nie aus einer ISR. */
void handback_serve(void);

/* Wartet, bis der Host das Board verlangt, und gibt es ab; kehrt nicht
 * zurueck. */
void handback_await(void);

/* Gibt das Board ab, je Board; kehrt nicht zurueck. Aus einem Faden. */
FUNC_NORETURN void handback_give(void);

#endif /* HANDBACK_H */
