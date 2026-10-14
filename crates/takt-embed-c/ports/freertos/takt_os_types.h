/* takt_os_types.h - der Zustand des FreeRTOS-Adapters (takt_os.h, M11
 * Schritt 20): die Uhr als 64-Bit-Erweiterung des Tickzaehlers, je Ereignis
 * ein binaeres Semaphor, je Faden seine Aufgabe mit statischem
 * Kontrollblock. Fuer jeden Port von FreeRTOS, der die FPU-Register jeder
 * Aufgabe sichert (auf Cortex-M4F `ARM_CM4F`, mit FreeRTOS-MPU
 * `ARM_CM4_MPU`); geprueft auf dem F401. Unter FreeRTOS-MPU sind beide
 * Faeden privilegiert (M11 Schritt 20b).
 *
 * **Die Konfiguration** (`FreeRTOSConfig.h`) verlangt statische Aufgaben und
 * Semaphore (`configSUPPORT_STATIC_ALLOCATION`), `vTaskDelete` fuer das Ende
 * eines Fadens und `uxTaskGetStackHighWaterMark2` fuer die Messung der
 * Stacks. Mit dieser fuellt FreeRTOS jeden neuen Stack selbst mit 0xA5
 * (`tskSET_NEW_STACKS_TO_KNOWN_VALUE`); der Adapter malt darum nicht und misst
 * mit FreeRTOS.
 *
 * **Die Uhr** zaehlt in Ticks des Systems (`xTaskGetTickCount`,
 * `configTICK_RATE_HZ`). Nur der Schrittfaden liest sie und wartet mit Frist
 * (takt_os.h); er erweitert sie dabei ohne Atomik auf 64 Bit und wartet eine
 * ferne Frist in Stuecken unter dem halben Umlauf ab.
 *
 * **Die Prioritaeten** zaehlen in FreeRTOS wie in Takt aufwaerts: Eine
 * Prioritaet `p` von Takt ist die Prioritaet `p` der Aufgabe, 0 ist die
 * niedrigste ueber der Leerlaufaufgabe. Der Schritt steht ganz oben, der
 * Jobfaden darunter (12.11). */
#ifndef TAKT_OS_TYPES_H
#define TAKT_OS_TYPES_H

#include <stddef.h>
#include <stdint.h>

#include "FreeRTOS.h"
#include "semphr.h"
#include "task.h"

#if configSUPPORT_STATIC_ALLOCATION != 1
#error "der FreeRTOS-Adapter von Takt legt Aufgaben und Semaphore statisch an: configSUPPORT_STATIC_ALLOCATION 1"
#endif
#if INCLUDE_vTaskDelete != 1
#error "ein Faden des Ports endet mit vTaskDelete: INCLUDE_vTaskDelete 1"
#endif
#if INCLUDE_uxTaskGetStackHighWaterMark2 != 1
#error "der Adapter misst die Stacks mit FreeRTOS (12.3): INCLUDE_uxTaskGetStackHighWaterMark2 1"
#endif

/* Die Prioritaeten: der Schritt ueber allem, was Takt-Kanaele bedient, der
 * Jobfaden darunter (12.11). */
#define TAKT_OS_STEP_PRIORITY ((int)configMAX_PRIORITIES - 1)
#define TAKT_OS_JOB_PRIORITY ((int)configMAX_PRIORITIES - 2)

/* Ein binaeres Semaphor mit seinem Speicher. */
struct takt_os_event {
    SemaphoreHandle_t handle;
    StaticSemaphore_t storage;
};

/* Ein Faden: seine Aufgabe, was er ausfuehrt, das Ereignis seines Endes und
 * der freie Stack, den er am Ende misst. */
struct takt_os_thread {
    TaskHandle_t task;
    StaticTask_t tcb;
    void (*body)(void *arg);
    void *arg;
    struct takt_os_event ended;
    configSTACK_DEPTH_TYPE unused; /* der Stack, den der Faden nie erreichte, in Worten */
    int over;                      /* das Ende ist abgewartet */
};

struct takt_os {
    uint64_t ticks;  /* Ticks seit `takt_os_init`, erweitert auf 64 Bit */
    TickType_t last; /* der Tickzaehler bei der letzten Erweiterung */
    struct takt_os_event events[2];
    struct takt_os_thread threads[2];
};

#endif /* TAKT_OS_TYPES_H */
