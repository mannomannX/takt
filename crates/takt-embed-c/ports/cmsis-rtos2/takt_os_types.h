/* takt_os_types.h - der Zustand des CMSIS-RTOS2-Adapters (takt_os.h, M11
 * Schritt 19c): die Uhr als 64-Bit-Erweiterung des Tickzaehlers, je Ereignis
 * ein binaeres Semaphor, je Faden seine Kennung. Fuer jede Implementierung
 * von CMSIS-RTOS2 (Keil RTX5, CMSIS-FreeRTOS, Zephyrs CMSIS-Schicht);
 * geprueft unter Zephyr auf dem F401.
 *
 * **Die Ereignisse** sind binaere Semaphore (`osSemaphoreNew(1, 0, …)`),
 * nicht die Thread-Flags: Zephyrs CMSIS-Schicht rechnet die Wartezeit von
 * `osThreadFlagsWait` in ganze Millisekunden um und rundet ab, eine Frist
 * unter einer Millisekunde waere „nicht warten“ (FB-736).
 *
 * **Die Uhr** zaehlt in Ticks des Systems (`osKernelGetTickCount`, 32 Bit).
 * Nur der Schrittfaden liest sie und wartet mit Frist (takt_os.h); er
 * erweitert sie dabei ohne Atomik auf 64 Bit und wartet eine ferne Frist in
 * Stuecken unter dem halben Umlauf ab.
 *
 * **Die Prioritaeten** zaehlen in CMSIS-RTOS2 wie in Takt aufwaerts: Eine
 * Prioritaet `p` von Takt ist `osPriority_t` `p` (`osPriorityLow` 8 bis
 * `osPriorityRealtime7` 55), 0 ist `osPriorityNormal`.
 *
 * **Die Stacks** stellt der Wirt; `bytes` ist die Groesse, die `osThreadNew`
 * als `stack_size` bekommt (unter Zephyr `K_THREAD_STACK_SIZEOF` eines
 * `K_THREAD_STACK_DEFINE`). Die Implementierung fuellt den Stack beim
 * Anlegen selbst oder fuehrt ihre Hochwassermarke (Zephyr: `INIT_STACKS`,
 * RTX5: Watermark); der Adapter malt darum nicht und misst mit
 * `osThreadGetStackSpace`: Die Tiefe ist `bytes` minus der freie Platz. Die
 * Kontrollbloecke nimmt die Implementierung aus ihren Pools (`cb_mem` NULL). */
#ifndef TAKT_OS_TYPES_H
#define TAKT_OS_TYPES_H

#include <stddef.h>
#include <stdint.h>

#include "cmsis_os2.h"

/* Die Prioritaeten aus CMSIS-RTOS2: der Schritt ueber allem, was Takt-Kanaele
 * bedient, der Jobfaden darunter (12.11). */
#define TAKT_OS_STEP_PRIORITY ((int)osPriorityRealtime7)
#define TAKT_OS_JOB_PRIORITY ((int)osPriorityHigh)

/* CMSIS-RTOS2 kennt keine Partitionen; was das System darunter kann, sieht
 * der Adapter nicht (12.11). */
#define TAKT_OS_PARTITION 0

/* Ein Faden: seine Kennung, was er ausfuehrt und wie gross sein Stack ist. */
struct takt_os_thread {
    osThreadId_t id;
    void (*body)(void *arg);
    void *arg;
    size_t bytes;
};

struct takt_os {
    uint64_t ticks; /* Ticks seit `takt_os_init`, erweitert auf 64 Bit */
    uint32_t last;  /* der Tickzaehler bei der letzten Erweiterung */
    osSemaphoreId_t events[2];
    struct takt_os_thread threads[2];
};

#endif /* TAKT_OS_TYPES_H */
