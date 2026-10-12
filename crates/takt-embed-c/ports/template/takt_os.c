/* takt_os.c - die Vorlage `c-port` (takt_os.h): ein Adapter fuer ein System
 * mit Faeden. Jede Stelle `TODO(port)` braucht das Primitiv des Systems; bis
 * dahin scheitert `takt_os_init`, und kein Faden startet. Was jede Funktion
 * zusagen muss, steht in `ports/rtos/takt_os.h`. Muster: `ports/posix`. */
#include "takt_os.h"

/* Bis eine Stelle gefuellt ist: der Fehler, mit dem die Vorlage scheitert. */
#define TAKT_OS_NOT_PORTED (-1)

int takt_os_init(struct takt_os *os) {
    /* TODO(port): die beiden Ereignisse anlegen, leer; `origin_ns` auf die
     * monotone Uhr setzen. Ein Fehler des Systems kommt als Rueckgabe zurueck. */
    os->origin_ns = 0;
    return TAKT_OS_NOT_PORTED;
}

void takt_os_destroy(struct takt_os *os) {
    /* TODO(port): die Ereignisse wieder abgeben. */
    (void)os;
}

int64_t takt_os_now(const struct takt_os *os) {
    /* TODO(port): die monotone Uhr in Nanosekunden, minus `origin_ns`. Sie
     * springt nie zurueck und steht nie still. */
    return -os->origin_ns;
}

void takt_os_give(struct takt_os *os, int event) {
    /* TODO(port): das Ereignis `event` setzen; ein zweites Setzen vor dem
     * Warten faellt mit dem ersten zusammen (binaeres Semaphor, Ereignisflag). */
    (void)os;
    (void)event;
}

void takt_os_take(struct takt_os *os, int event, int64_t deadline_ns) {
    /* TODO(port): auf das Ereignis `event` warten und es loeschen, hoechstens
     * bis `deadline_ns` (absolut, wie `takt_os_now`); TAKT_OS_FOREVER heisst
     * ohne Frist. Rechne eine relative Wartezeit erst hier aus der absoluten
     * Frist, sonst wandert die Frist um jede Verspaetung (12.2). */
    (void)os;
    (void)event;
    (void)deadline_ns;
}

int takt_os_spawn(struct takt_os *os, int which, void (*body)(void *arg), void *arg, void *stack, size_t bytes,
                  int priority) {
    /* TODO(port): einen Faden anlegen, der `body(arg)` auf `bytes` Byte ab
     * `stack` ausfuehrt, mit der Prioritaet `priority` (TAKT_OS_STEP die
     * hoechste der Takt-Faeden) und mit gesicherten FPU-Registern (12.11). */
    (void)os;
    (void)which;
    (void)body;
    (void)arg;
    (void)stack;
    (void)bytes;
    (void)priority;
    return TAKT_OS_NOT_PORTED;
}

int takt_os_join(struct takt_os *os, int which) {
    /* TODO(port): warten, bis der Faden `which` endet. */
    (void)os;
    (void)which;
    return TAKT_OS_NOT_PORTED;
}
