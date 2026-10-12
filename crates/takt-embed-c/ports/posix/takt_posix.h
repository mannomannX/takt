/* takt_posix.h - der POSIX-Port (12.11, M11 Schritt 18c): ein Kern des SDK
 * (`takt_sdk.h`) in zwei Faeden, fuer jedes System mit `pthread` und
 * `CLOCK_MONOTONIC` (Linux ohne `linux_rt`-Zusage, NuttX, RTEMS, QNX).
 *
 * Der Schrittfaden ruft `takt_core_service` zu jeder Frist. Er wartet
 * absolut auf `CLOCK_MONOTONIC` bis zur Frist, die der Kern nennt, und
 * wacht frueher, wenn der Jobfaden einen Auftrag fertig meldet: Dann
 * verteilt `takt_core_service` vor der Frist den naechsten wartenden Job,
 * wie die Interruptform nach dem Job-Interrupt. Der Jobfaden ruft
 * `takt_core_job_work`, wenn der Kern Jobs meldet, und laeuft unter dem
 * Schrittfaden: Mit Prioritaeten unter `SCHED_FIFO` unterbricht der Schritt
 * den Job; ohne sie rechnen beide auf verschiedenen Kernen oder im Wechsel,
 * und eine Zeitzusage gibt es nicht.
 *
 * Der Wirt stellt beide Stacks; der Port bemalt sie vor dem Start und misst
 * nach dem Ende, wie tief jeder reichte (12.3). Die Stacks wachsen nach
 * unten, wie auf jedem Ziel von Takt.
 *
 * Ablauf: `takt_posix_start`, dann im aufrufenden Faden fremde Arbeit,
 * solange `takt_posix_running` wahr ist, dann `takt_posix_join` und
 * `takt_core_report`.
 */
#ifndef TAKT_POSIX_H
#define TAKT_POSIX_H

#include <pthread.h>
#include <stddef.h>
#include <stdint.h>
#include <time.h>

#include "takt_sdk.h"

#ifdef __cplusplus
extern "C" {
#endif

/* Was der Wirt dem Port gibt. */
struct takt_posix_config {
    uint64_t ticks;          /* so viele Ticks; 0: bis das Programm endet (12.7) */
    int64_t tick_ns;         /* die Tickperiode, `P_TICK_NS` */
    void *tick_stack;        /* der Stack des Schrittfadens, mindestens PTHREAD_STACK_MIN */
    size_t tick_stack_bytes;
    void *job_stack;         /* der Stack des Jobfadens, ebenso */
    size_t job_stack_bytes;
    int step_priority;       /* unter SCHED_FIFO; 0: die Politik des Wirts, ohne Zeitzusage */
    int job_priority;        /* unter `step_priority` */
};

/* Der Port; der Wirt stellt den Speicher, die Felder gehoeren dem Port. */
struct takt_posix {
    takt_core *core;
    struct takt_posix_config config;
    struct timespec origin;
    pthread_mutex_t lock;
    pthread_cond_t step_wake;
    pthread_cond_t job_wake;
    pthread_t step;
    pthread_t job;
    uint8_t job_pending; /* unter `lock`: der Kern meldete Jobs */
    uint8_t job_done;    /* unter `lock`: der Jobfaden rechnete einen Auftrag */
    uint8_t stop;        /* unter `lock`: der Lauf ist zu Ende */
    int running;         /* atomar: der Schrittfaden laeuft */
};

/* Startet den Lauf des Kerns `core` (aus `takt_core_init`): Tick 0 liegt
 * jetzt. 0 oder der Fehler von `pthread`; bei einem Fehler laeuft kein
 * Faden. */
int takt_posix_start(struct takt_posix *posix, takt_core *core, const struct takt_posix_config *config);

/* Wahr, solange der Schrittfaden laeuft. */
int takt_posix_running(struct takt_posix *posix);

/* Wartet auf das Ende beider Faeden und nennt in `stacks` die Tiefe jedes
 * Stacks; die Schranken traegt der Wirt ein. 0 oder der Fehler von
 * `pthread_join`. */
int takt_posix_join(struct takt_posix *posix, struct takt_stacks *stacks);

#ifdef __cplusplus
}
#endif

#endif /* TAKT_POSIX_H */
