/* takt_irq_chip.h - was ein Chip der Interruptform gibt (12.11).
 *
 * Der Port (`takt_irq.c`) erfuellt den Portvertrag einmal fuer jeden Chip
 * ohne RTOS; ein Adapter je Chip stellt nur diese Primitive und die beiden
 * Interrupts, die den Port rufen. Ein Adapter liegt in einem eigenen
 * Verzeichnis und definiert die Funktionen unten in seinem
 * `takt_irq_chip.c`: `ports/irq/stm32f4`, weitere je Chip.
 *
 * - Die Uhr ist ein freilaufender Zeitgeber, monoton in Nanosekunden seit
 *   `takt_irq_chip_start`; der Takt, der ihn speist, steht in
 *   `takt_irq_config.timer_hz`.
 * - Der Alarm liegt auf demselben Zeitgeber. Seine ISR hat die Prioritaet
 *   `takt_irq_config.step_priority` und ruft `takt_irq_on_step`: mit `due`
 *   wahr, sobald die Frist erreicht ist, sonst falsch (ein Ueberlauf, ein
 *   Anstehen von Hand). Ist die Frist schon vorbei, wenn der Alarm sie
 *   bekommt, steht die ISR sofort an.
 * - Der Job-Interrupt liegt unter allen anderen und ruft `takt_irq_on_jobs`.
 * - Jeder Interrupt sichert die FPU-Register dessen, was er unterbricht
 *   (12.11).
 * - Was ISR und Hauptschleife teilen, ist ein Wort; jedes andere Datum
 *   gehoert einem Kontext.
 * - Hat der Chip eine Schutzeinheit, setzt `takt_irq_chip_protect` eine
 *   Region ueber den Programmbereich der Arena und fuellt die Haken fuer
 *   `takt_core_protect` (12.3): beschreibbar nur, waehrend der Kern ihn
 *   schreibt. Ein Zugriff ausserhalb wird abgewiesen, uebergangen und dem
 *   Kern gemeldet. Jede ISR des Wirts, die den Schritt unterbrechen kann,
 *   laeuft zwischen `takt_irq_chip_isr_enter` und `takt_irq_chip_isr_leave`:
 *   Fuer sie ist der Programmbereich nur lesbar, auch mitten im Schritt.
 */
#ifndef TAKT_IRQ_CHIP_H
#define TAKT_IRQ_CHIP_H

#include <stdint.h>

#include "takt_irq.h"

#ifdef __cplusplus
extern "C" {
#endif

/* Startet die Zeitachse bei 0 und gibt beide Interrupts frei; ihre ISRs
 * rufen danach den Port `irq`. 0 oder ein Fehler des Chips, etwa ein Takt
 * des Zeitgebers, den er nicht teilen kann. */
int takt_irq_chip_start(struct takt_irq *irq);

/* Nanosekunden seit `takt_irq_chip_start`. */
int64_t takt_irq_chip_now(void);

/* Stellt den Alarm auf `at_ns` (wie `takt_irq_chip_now`); er feuert nie
 * vorher. */
void takt_irq_chip_arm(int64_t at_ns);

/* Nimmt den Alarm zurueck. */
void takt_irq_chip_disarm(void);

/* Laesst die ISR des Zeitgebers anstehen, ohne dass eine Frist erreicht ist. */
void takt_irq_chip_pend_step(void);

/* Laesst den Job-Interrupt anstehen. */
void takt_irq_chip_pend_jobs(void);

/* Die Region ueber dem Programmbereich ab `arena`, `align` Byte gross und
 * an `align` ausgerichtet (`P_PROTECT_ALIGN`), die `bytes` davon von vorn
 * deckt (`P_PROTECT_BYTES`), offen, bis der Kern sie schliesst; `protection`
 * bekommt die Haken fuer `takt_core_protect`. 0, TAKT_IRQ_INVALID oder ein
 * Fehler des Chips, wenn keine Region die Arena so deckt; dann gibt es
 * keine, und der Kern bleibt ohne Schutz. */
int takt_irq_chip_protect(void *arena, uint32_t align, uint32_t bytes, struct takt_protection *protection);

/* Am Anfang einer ISR des Wirts: Der Programmbereich ist fuer sie nur
 * lesbar; die Rueckgabe ist, was vorher galt. Ohne Region tut sie nichts. */
uint32_t takt_irq_chip_isr_enter(void);

/* Am Ende derselben ISR: Es gilt wieder `before`. */
void takt_irq_chip_isr_leave(uint32_t before);

#ifdef __cplusplus
}
#endif

#endif /* TAKT_IRQ_CHIP_H */
