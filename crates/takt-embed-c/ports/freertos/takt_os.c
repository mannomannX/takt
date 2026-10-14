/* takt_os.c - der FreeRTOS-Adapter des Ports mit Faeden (takt_os.h,
 * takt_os_types.h). */
#include "takt_os.h"

#define TAKT_NS_PER_S 1000000000ULL

/* So viele Ticks wartet `take` hoechstens am Stueck: unter dem halben
 * Umlauf des Tickzaehlers, damit die Erweiterung keinen verpasst. */
#define TAKT_OS_CHUNK (portMAX_DELAY / 4U)

/* Der Fehler des Adapters, wenn FreeRTOS ein Objekt nicht anlegt. */
#define TAKT_OS_FAILED (-1)

/* Die Ticks seit `takt_os_init`, auf 64 Bit erweitert; nur der Schrittfaden
 * ruft sie (takt_os.h). Die Differenz zweier Staende ohne Vorzeichen deckt
 * einen Ueberlauf, solange zwischen zwei Aufrufen kein zweiter liegt. */
static uint64_t takt_os_ticks(struct takt_os *os) {
    const TickType_t now = xTaskGetTickCount();
    const TickType_t passed = now - os->last;
    os->ticks += (uint64_t)passed;
    os->last = now;
    return os->ticks;
}

/* Nanosekunden `ns` als Ticks, aufgerundet: Eine Frist kommt nie zu frueh.
 * Erst die ganzen Sekunden, damit nichts ueberlaeuft. */
static uint64_t takt_os_ns_to_ticks(uint64_t ns) {
    const uint64_t hz = (uint64_t)configTICK_RATE_HZ;
    const uint64_t rest = (ns % TAKT_NS_PER_S) * hz;
    return ((ns / TAKT_NS_PER_S) * hz) + ((rest + TAKT_NS_PER_S - 1U) / TAKT_NS_PER_S);
}

/* Ticks als Nanosekunden, abgerundet. */
static uint64_t takt_os_ticks_to_ns(uint64_t ticks) {
    const uint64_t hz = (uint64_t)configTICK_RATE_HZ;
    return ((ticks / hz) * TAKT_NS_PER_S) + (((ticks % hz) * TAKT_NS_PER_S) / hz);
}

/* Ein binaeres Semaphor, leer; 0 oder TAKT_OS_FAILED. */
static int takt_os_event_init(struct takt_os_event *e) {
    e->handle = xSemaphoreCreateBinaryStatic(&e->storage);
    return (e->handle != NULL) ? 0 : TAKT_OS_FAILED;
}

static void takt_os_event_destroy(struct takt_os_event *e) {
    if (e->handle != NULL) {
        vSemaphoreDelete(e->handle);
        e->handle = NULL;
    }
}

/* Wartet ohne Frist; ohne `INCLUDE_vTaskSuspend` ist `portMAX_DELAY` nur eine
 * lange Frist, darum in einer Schleife. */
static void takt_os_event_wait(const struct takt_os_event *e) {
    while (xSemaphoreTake(e->handle, portMAX_DELAY) != pdTRUE) {
        /* weiter warten */
    }
}

int takt_os_init(struct takt_os *os) {
    int rc = 0;
    for (size_t i = 0U; i < 2U; i++) {
        os->events[i].handle = NULL;
        os->threads[i].task = NULL;
        os->threads[i].ended.handle = NULL;
        os->threads[i].over = 0;
    }
    for (size_t i = 0U; (i < 2U) && (rc == 0); i++) {
        rc = takt_os_event_init(&os->events[i]);
        if (rc == 0) {
            rc = takt_os_event_init(&os->threads[i].ended);
        }
    }
    if (rc != 0) {
        takt_os_destroy(os);
    }
    os->ticks = 0U;
    os->last = xTaskGetTickCount();
    return rc;
}

void takt_os_destroy(struct takt_os *os) {
    for (size_t i = 0U; i < 2U; i++) {
        takt_os_event_destroy(&os->events[i]);
        takt_os_event_destroy(&os->threads[i].ended);
    }
}

int64_t takt_os_now(struct takt_os *os) {
    const uint64_t ns = takt_os_ticks_to_ns(takt_os_ticks(os));
    return (ns > (uint64_t)INT64_MAX) ? INT64_MAX : (int64_t)ns;
}

/* Ist das Ereignis schon gesetzt, lehnt FreeRTOS ab, und zwei `give` sind
 * eines. */
void takt_os_give(struct takt_os *os, int event) { (void)xSemaphoreGive(os->events[event].handle); }

/* Mit Frist: die verbleibenden Ticks aus der absoluten Frist, in Stuecken
 * unter dem halben Umlauf; nach jedem Stueck die Uhr neu. Eine erreichte
 * Frist nimmt ein gesetztes Ereignis noch mit (Wartezeit 0). */
void takt_os_take(struct takt_os *os, int event, int64_t deadline_ns) {
    const struct takt_os_event *e = &os->events[event];
    if (deadline_ns == TAKT_OS_FOREVER) {
        takt_os_event_wait(e);
    } else {
        const uint64_t due = takt_os_ns_to_ticks((uint64_t)((deadline_ns > 0) ? deadline_ns : 0));
        const TickType_t chunk = TAKT_OS_CHUNK;
        int waiting = 1;
        while (waiting != 0) {
            const uint64_t now = takt_os_ticks(os);
            const uint64_t left = (due > now) ? (due - now) : 0U;
            const TickType_t wait = (left > (uint64_t)chunk) ? chunk : (TickType_t)left;
            const BaseType_t taken = xSemaphoreTake(e->handle, wait);
            waiting = ((taken != pdTRUE) && (left > (uint64_t)chunk)) ? 1 : 0;
        }
    }
}

/* FreeRTOS fuellt jeden neuen Stack selbst (takt_os_types.h). */
void takt_os_paint(struct takt_os *os, int which, void *stack, size_t bytes) {
    (void)os;
    (void)which;
    (void)stack;
    (void)bytes;
}

/* Der Einstieg jedes Fadens: was `takt_os_spawn` ihm gab. Danach misst er,
 * wie viel Stack er nie erreichte, meldet sein Ende und loescht sich, wie
 * FreeRTOS es von einer Aufgabe verlangt, die endet. */
static void takt_os_run(void *thread) {
    struct takt_os_thread *t = thread;
    t->body(t->arg);
    t->unused = uxTaskGetStackHighWaterMark2(NULL);
    (void)xSemaphoreGive(t->ended.handle);
    vTaskDelete(NULL);
}

/* Der Port von FreeRTOS sichert die FPU-Register jeder Aufgabe, die sie
 * benutzt (auf Cortex-M4F `ARM_CM4F` und `ARM_CM4_MPU`, 12.11). Schritt und
 * Jobs beschreiben die Arena: Unter FreeRTOS-MPU sind sie privilegiert, und
 * die Arena liegt in keiner Region einer Aufgabe ohne Privileg (12.11,
 * `Schutz: Partition`); ohne MPU ist `portPRIVILEGE_BIT` 0. */
int takt_os_spawn(struct takt_os *os, int which, void (*body)(void *arg), void *arg, void *stack, size_t bytes,
                  int priority) {
    struct takt_os_thread *t = &os->threads[which];
    int rc = TAKT_OS_FAILED;
    if ((priority >= 0) && (priority < (int)configMAX_PRIORITIES)) {
        const UBaseType_t level = (priority == 0) ? (UBaseType_t)(tskIDLE_PRIORITY + 1U) : (UBaseType_t)priority;
        t->body = body;
        t->arg = arg;
        t->over = 0;
        t->task = xTaskCreateStatic(takt_os_run, (which == TAKT_OS_STEP) ? "takt-step" : "takt-job",
                                    (configSTACK_DEPTH_TYPE)(bytes / sizeof(StackType_t)), t,
                                    level | (UBaseType_t)portPRIVILEGE_BIT, (StackType_t *)stack, &t->tcb);
        rc = (t->task != NULL) ? 0 : TAKT_OS_FAILED;
    }
    return rc;
}

/* Wartet einmal auf das Ende des Fadens: `depth` vor `join` und `join`
 * selbst teilen es. */
static void takt_os_await(struct takt_os_thread *t) {
    if (t->over == 0) {
        takt_os_event_wait(&t->ended);
        t->over = 1;
    }
}

/* Die Tiefe aus dem freien Stack, den der Faden an seinem Ende maß. */
int takt_os_depth(struct takt_os *os, int which, const void *stack, size_t bytes, uint32_t *depth) {
    struct takt_os_thread *t = &os->threads[which];
    (void)stack;
    takt_os_await(t);
    const uint64_t unused = (uint64_t)t->unused * (uint64_t)sizeof(StackType_t);
    const uint64_t size = (uint64_t)bytes;
    const uint64_t used = (unused < size) ? (size - unused) : 0U;
    *depth = (used > (uint64_t)UINT32_MAX) ? UINT32_MAX : (uint32_t)used;
    return 0;
}

int takt_os_join(struct takt_os *os, int which) {
    takt_os_await(&os->threads[which]);
    return 0;
}
