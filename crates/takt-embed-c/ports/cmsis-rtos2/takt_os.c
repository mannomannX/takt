/* takt_os.c - der CMSIS-RTOS2-Adapter des Ports mit Faeden (takt_os.h,
 * takt_os_types.h). */
#include "takt_os.h"

#include <string.h>

#define TAKT_NS_PER_S 1000000000ULL

/* So viele Ticks wartet `take` hoechstens am Stueck: unter dem halben
 * Umlauf des 32-Bit-Zaehlers, damit die Erweiterung keinen verpasst. */
#define TAKT_OS_CHUNK (1UL << 30)

/* Was die Implementierung bei einem Fehler meldet, als Fehler des Adapters. */
static int takt_os_status(osStatus_t status) { return (status == osOK) ? 0 : (int)status; }

/* Die Ticks seit `takt_os_init`, auf 64 Bit erweitert; nur der Schrittfaden
 * ruft sie (takt_os.h). Die Differenz zweier Staende ohne Vorzeichen deckt
 * einen Ueberlauf, solange zwischen zwei Aufrufen keiner zweite liegt. */
static uint64_t takt_os_ticks(struct takt_os *os) {
    const uint32_t now = osKernelGetTickCount();
    os->ticks += (uint64_t)(uint32_t)(now - os->last);
    os->last = now;
    return os->ticks;
}

/* Nanosekunden `ns` als Ticks, aufgerundet: Eine Frist kommt nie zu frueh.
 * Erst die ganzen Sekunden, damit nichts ueberlaeuft. */
static uint64_t takt_os_ns_to_ticks(uint64_t ns, uint64_t hz) {
    const uint64_t rest = (ns % TAKT_NS_PER_S) * hz;
    return ((ns / TAKT_NS_PER_S) * hz) + ((rest + TAKT_NS_PER_S - 1U) / TAKT_NS_PER_S);
}

/* Ticks als Nanosekunden, abgerundet. */
static uint64_t takt_os_ticks_to_ns(uint64_t ticks, uint64_t hz) {
    return ((ticks / hz) * TAKT_NS_PER_S) + (((ticks % hz) * TAKT_NS_PER_S) / hz);
}

int takt_os_init(struct takt_os *os) {
    int rc = 0;
    (void)memset(os, 0, sizeof *os);
    os->events[TAKT_OS_STEP] = osSemaphoreNew(1U, 0U, NULL);
    os->events[TAKT_OS_JOB] = osSemaphoreNew(1U, 0U, NULL);
    if ((os->events[TAKT_OS_STEP] == NULL) || (os->events[TAKT_OS_JOB] == NULL)) {
        takt_os_destroy(os);
        rc = (int)osErrorNoMemory;
    }
    os->last = osKernelGetTickCount();
    return rc;
}

void takt_os_destroy(struct takt_os *os) {
    for (size_t i = 0U; i < (sizeof os->events / sizeof os->events[0]); i++) {
        if (os->events[i] != NULL) {
            (void)osSemaphoreDelete(os->events[i]);
            os->events[i] = NULL;
        }
    }
}

int64_t takt_os_now(struct takt_os *os) {
    const uint64_t ns = takt_os_ticks_to_ns(takt_os_ticks(os), osKernelGetTickFreq());
    return (ns > (uint64_t)INT64_MAX) ? INT64_MAX : (int64_t)ns;
}

/* Aus einem Faden oder einer ISR; ist das Ereignis schon gesetzt, meldet die
 * Implementierung `osErrorResource`, und zwei `give` sind eines. */
void takt_os_give(struct takt_os *os, int event) {
    (void)osSemaphoreRelease(os->events[event]);
}

/* Mit Frist: die verbleibenden Ticks aus der absoluten Frist, in Stuecken
 * unter dem halben Umlauf; nach jedem Stueck die Uhr neu. Eine erreichte
 * Frist nimmt ein gesetztes Ereignis noch mit (Wartezeit 0). */
void takt_os_take(struct takt_os *os, int event, int64_t deadline_ns) {
    if (deadline_ns == TAKT_OS_FOREVER) {
        (void)osSemaphoreAcquire(os->events[event], osWaitForever);
    } else {
        const uint64_t hz = osKernelGetTickFreq();
        const uint64_t due = takt_os_ns_to_ticks((uint64_t)((deadline_ns > 0) ? deadline_ns : 0), hz);
        int waiting = 1;
        while (waiting != 0) {
            const uint64_t now = takt_os_ticks(os);
            const uint64_t left = (due > now) ? (due - now) : 0U;
            const uint32_t wait = (left > TAKT_OS_CHUNK) ? (uint32_t)TAKT_OS_CHUNK : (uint32_t)left;
            const osStatus_t status = osSemaphoreAcquire(os->events[event], wait);
            waiting = ((status == osErrorTimeout) && (left > TAKT_OS_CHUNK)) ? 1 : 0;
        }
    }
}

/* Der Einstieg jedes Fadens: was `takt_os_spawn` ihm gab; danach endet er
 * ausdruecklich, wie CMSIS-RTOS2 es von jedem Faden verlangt. */
static void takt_os_run(void *thread) {
    const struct takt_os_thread *t = thread;
    t->body(t->arg);
    osThreadExit();
}

/* Die Implementierung sichert die FPU-Register jedes Fadens, der sie
 * benutzt: unter Zephyr mit `CONFIG_FPU_SHARING`, unter RTX5 ueber das
 * verzoegerte Sichern des Cortex-M (12.11). */
int takt_os_spawn(struct takt_os *os, int which, void (*body)(void *arg), void *arg, void *stack, size_t bytes,
                  int priority) {
    struct takt_os_thread *t = &os->threads[which];
    int rc = (int)osErrorParameter;
    if ((priority == 0) || ((priority >= (int)osPriorityLow) && (priority <= (int)osPriorityRealtime7))) {
        osThreadAttr_t attr;
        (void)memset(&attr, 0, sizeof attr);
        attr.name = (which == TAKT_OS_STEP) ? "takt-step" : "takt-job";
        attr.attr_bits = osThreadJoinable;
        attr.stack_mem = stack;
        attr.stack_size = (uint32_t)bytes;
        attr.priority = (priority == 0) ? osPriorityNormal : (osPriority_t)priority;
        t->body = body;
        t->arg = arg;
        t->bytes = bytes;
        t->id = osThreadNew(takt_os_run, t, &attr);
        rc = (t->id != NULL) ? 0 : (int)osError;
    }
    return rc;
}

/* Ein Faden, der schon beendet ist, gilt als abgewartet: Zephyrs CMSIS-Schicht
 * lehnt `osThreadJoin` fuer ihn mit `osErrorResource` ab (FB-736). */
int takt_os_join(struct takt_os *os, int which) {
    const osThreadId_t id = os->threads[which].id;
    osStatus_t status = osThreadJoin(id);
    if ((status == osErrorResource) && (osThreadGetState(id) == osThreadTerminated)) {
        status = osOK;
    }
    return takt_os_status(status);
}

/* Die Implementierung fuellt den Stack beim Anlegen selbst. */
void takt_os_paint(struct takt_os *os, int which, void *stack, size_t bytes) {
    (void)os;
    (void)which;
    (void)stack;
    (void)bytes;
}

/* Die Tiefe ueber `stack_size` (`bytes`): was ueber dem freien Platz liegt,
 * den die Implementierung unter ihrem Wasserstand meldet. */
int takt_os_depth(struct takt_os *os, int which, const void *stack, size_t bytes, uint32_t *depth) {
    const uint32_t space = osThreadGetStackSpace(os->threads[which].id);
    const size_t used = (space < bytes) ? (bytes - space) : 0U;
    (void)stack;
    *depth = (used > UINT32_MAX) ? UINT32_MAX : (uint32_t)used;
    return 0;
}
