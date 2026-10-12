/* takt_rtos.h - der Port fuer Systeme mit Faeden (12.11, M11 Schritt 18):
 * ein Kern des SDK (`takt_sdk.h`) in einem Schrittfaden und einem Jobfaden,
 * ueber die Primitive eines Adapters (`takt_os.h`). Jedes System mit Faeden
 * teilt diesen Port ueber seinen Adapter (`ports/posix`); ein neues bekommt
 * ihn mit einem Adapter nach der Vorlage `ports/template`.
 *
 * Der Schrittfaden ruft `takt_core_service` zu jeder Frist. Er wartet auf
 * die absolute Frist, die der Kern nennt, und wacht frueher, wenn der
 * Jobfaden einen Auftrag fertig meldet: Dann verteilt `takt_core_service`
 * vor der Frist den naechsten wartenden Job (12.2, 12.11). Der Jobfaden ruft
 * `takt_core_job_work`, wenn der Kern Jobs meldet, und laeuft unter dem
 * Schrittfaden: Mit Prioritaeten unterbricht der Schritt den Job; ohne sie
 * rechnen beide auf verschiedenen Kernen oder im Wechsel, und eine
 * Zeitzusage gibt es nicht.
 *
 * Der Wirt stellt beide Stacks; der Port bemalt sie vor dem Start und misst
 * nach dem Ende, wie tief jeder reichte (12.3). Die Stacks wachsen nach
 * unten, wie auf jedem Ziel von Takt.
 *
 * Ablauf: `takt_rtos_start`, dann im aufrufenden Faden fremde Arbeit,
 * solange `takt_rtos_running` wahr ist, dann `takt_rtos_join` und
 * `takt_core_report`.
 */
#ifndef TAKT_RTOS_H
#define TAKT_RTOS_H

#include <stddef.h>
#include <stdint.h>

#include "takt_os.h"
#include "takt_sdk.h"

#ifdef __cplusplus
extern "C" {
#endif

/* Was der Wirt dem Port gibt. */
struct takt_rtos_config {
    uint64_t ticks;          /* so viele Ticks; 0: bis das Programm endet (12.7) */
    int64_t tick_ns;         /* die Tickperiode, `P_TICK_NS` */
    void *tick_stack;        /* der Stack des Schrittfadens */
    size_t tick_stack_bytes;
    void *job_stack;         /* der Stack des Jobfadens */
    size_t job_stack_bytes;
    int step_priority;       /* hoechste; 0: die des Systems, ohne Zeitzusage */
    int job_priority;        /* unter `step_priority` */
};

/* Der Port; der Wirt stellt den Speicher, die Felder gehoeren dem Port. */
struct takt_rtos {
    takt_core *core;
    struct takt_rtos_config config;
    struct takt_os os;
    int stop;    /* atomar: der Lauf ist zu Ende */
    int running; /* atomar: der Schrittfaden laeuft */
};

/* Ungueltige Konfiguration: ein Zeiger fehlt, oder die Tickperiode ist nicht
 * positiv. */
#define TAKT_RTOS_INVALID (-1)

/* Startet den Lauf des Kerns `core` (aus `takt_core_init`): Tick 0 liegt
 * jetzt. 0, TAKT_RTOS_INVALID oder der Fehler des Adapters; bei einem Fehler
 * laeuft kein Faden. */
int takt_rtos_start(struct takt_rtos *rtos, takt_core *core, const struct takt_rtos_config *config);

/* Wahr, solange der Schrittfaden laeuft. */
int takt_rtos_running(const struct takt_rtos *rtos);

/* Wartet auf das Ende beider Faeden und nennt in `stacks` die Tiefe jedes
 * Stacks; die Schranken traegt der Wirt ein. 0 oder der Fehler des
 * Adapters. */
int takt_rtos_join(struct takt_rtos *rtos, struct takt_stacks *stacks);

#ifdef __cplusplus
}
#endif

#endif /* TAKT_RTOS_H */
