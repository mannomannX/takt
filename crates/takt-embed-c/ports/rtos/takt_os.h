/* takt_os.h - was ein Betriebssystem dem Port mit Faeden gibt (12.11).
 *
 * Der Port (`takt_rtos.c`) erfuellt den Portvertrag einmal fuer jedes System
 * mit Faeden; ein Adapter je System stellt nur diese Primitive. Ein Adapter
 * liegt in einem eigenen Verzeichnis, definiert `struct takt_os` in seinem
 * `takt_os_types.h` und die Funktionen unten in seinem `takt_os.c`:
 * `ports/posix`, die Vorlage `ports/template`, weitere je RTOS.
 *
 * - Die Uhr ist monoton, in Nanosekunden seit `takt_os_init`.
 * - Ein Ereignis ist binaer: `give` setzt es, ein zweites `give` vor dem
 *   `take` faellt mit dem ersten zusammen; `take` wartet, bis es gesetzt
 *   ist, und loescht es, oder bis die absolute Frist erreicht ist. Es gibt
 *   ein Ereignis je Faden: `TAKT_OS_STEP` weckt den Schrittfaden,
 *   `TAKT_OS_JOB` den Jobfaden.
 * - Ein Faden laeuft auf dem Stack, den der Wirt stellt, mit der Prioritaet
 *   `priority` (0: die des Systems) und sichert die FPU-Register ueber
 *   Kontextwechsel (12.11).
 */
#ifndef TAKT_OS_H
#define TAKT_OS_H

#include <stddef.h>
#include <stdint.h>

#include "takt_os_types.h"

#ifdef __cplusplus
extern "C" {
#endif

/* Die Ereignisse und Faeden des Ports. */
#define TAKT_OS_STEP 0
#define TAKT_OS_JOB 1

/* Eine Frist, die nie kommt: `take` wartet nur auf das Ereignis. */
#define TAKT_OS_FOREVER INT64_MAX

/* Uhr und Ereignisse bereitstellen; 0 oder der Fehler des Systems. */
int takt_os_init(struct takt_os *os);

/* Was `takt_os_init` bereitstellte, wieder abgeben; kein Faden laeuft. */
void takt_os_destroy(struct takt_os *os);

/* Nanosekunden seit `takt_os_init`. */
int64_t takt_os_now(const struct takt_os *os);

/* Setzt das Ereignis `event` (TAKT_OS_STEP, TAKT_OS_JOB). */
void takt_os_give(struct takt_os *os, int event);

/* Wartet auf das Ereignis `event` und loescht es, oder bis `deadline_ns`
 * (wie `takt_os_now`) erreicht ist; TAKT_OS_FOREVER wartet nur auf das
 * Ereignis. */
void takt_os_take(struct takt_os *os, int event, int64_t deadline_ns);

/* Startet den Faden `which` (TAKT_OS_STEP, TAKT_OS_JOB): `body(arg)` auf
 * `bytes` Byte Stack ab `stack`. 0 oder der Fehler des Systems. */
int takt_os_spawn(struct takt_os *os, int which, void (*body)(void *arg), void *arg, void *stack, size_t bytes,
                  int priority);

/* Wartet auf das Ende des Fadens `which`; 0 oder der Fehler des Systems. */
int takt_os_join(struct takt_os *os, int which);

#ifdef __cplusplus
}
#endif

#endif /* TAKT_OS_H */
