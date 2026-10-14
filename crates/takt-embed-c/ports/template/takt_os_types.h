/* takt_os_types.h - die Vorlage `c-port` (12.11, M11 Schritt 18): der
 * Zustand eines Adapters fuer ein System mit Faeden, das Takt noch nicht
 * kennt. Kopiere `ports/template` in ein eigenes Verzeichnis, fuelle jede
 * Stelle `TODO(port)` hier und in `takt_os.c` mit den Primitiven deines
 * Systems und binde es mit `ports/rtos/takt_rtos.c`. Ein Port, der danach
 * `takt port-test` besteht, ist so gut wie ein mitgelieferter (2.9). */
#ifndef TAKT_OS_TYPES_H
#define TAKT_OS_TYPES_H

#include <stdint.h>

/* TODO(port): 1, wenn das System die Arena in einer Partition haelt, die nur
 * der Schrittfaden beschreibt, aus seiner Konfiguration (12.11); sonst 0. */
#define TAKT_OS_PARTITION 0

struct takt_os {
    /* TODO(port): der Zaehlerstand der monotonen Uhr bei `takt_os_init`. */
    int64_t origin_ns;
    /* TODO(port): je Ereignis ein binaeres Semaphor oder ein Ereignisflag des
     * Systems (TAKT_OS_STEP, TAKT_OS_JOB), das mit Frist warten kann. */
    int events[2];
    /* TODO(port): je Faden sein Griff (TAKT_OS_STEP, TAKT_OS_JOB). */
    int threads[2];
};

#endif /* TAKT_OS_TYPES_H */
