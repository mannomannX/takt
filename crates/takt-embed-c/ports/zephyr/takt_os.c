/* takt_os.c - der Zephyr-Adapter des Ports mit Faeden (takt_os.h,
 * takt_os_types.h).
 *
 * **Die Uhr und die Frist** rechnen in Systemticks, denn darin fuehrt der
 * Zephyr-Kern seine Zeitgeber; der Ursprung ist der Tick von
 * `takt_os_init`. Eine absolute Frist wird auf den naechsten Tick
 * aufgerundet: Der Schritt kommt nie vor ihr, und hoechstens einen
 * Systemtick (`CONFIG_SYS_CLOCK_TICKS_PER_SEC`) nach ihr; ist die
 * Tickperiode des Programms ein Vielfaches des Systemticks, liegt jede Frist
 * auf einem Tick, und die Rundung verspaetet nichts. Abgerundet kaeme der
 * Schritt bis zu einen Tick zu frueh, `takt_core_service` rechnete keine
 * Grenze und nennte dieselbe Frist, und der Schrittfaden, der hoechste von
 * allen, kreiste bis zum Tick der Frist. Die Ticks sind 64 Bit breit
 * (`CONFIG_TIMEOUT_64BIT`, das Kconfig verlangt): Jede Frist wartet der
 * Zephyr-Kern in einem Stueck, und die Uhr laeuft nicht ueber.
 *
 * Nicht `k_cycle_get_64`: Der Zaehler des Systemzeitgebers und die Ticks
 * haben nicht auf jedem Ziel denselben Ursprung — auf dem ESP32 zaehlt er ab
 * dem Einschalten, die Ticks ab dem Start des Treibers —, und eine Frist aus
 * Zyklen (`K_TIMEOUT_ABS_CYC`) laege um den Abstand daneben. */
#include "takt_os.h"
#include "takt_rtos.h"

#include <errno.h>

int takt_os_init(struct takt_os *os) {
    int rc = k_sem_init(&os->events[TAKT_OS_STEP], 0U, 1U);
    if (rc == 0) {
        rc = k_sem_init(&os->events[TAKT_OS_JOB], 0U, 1U);
    }
    os->origin = k_uptime_ticks();
    return rc;
}

/* Semaphore und Faeden des Zephyr-Kerns geben nichts frei. */
void takt_os_destroy(struct takt_os *os) {
    ARG_UNUSED(os);
}

int64_t takt_os_now(struct takt_os *os) {
    const uint64_t ns = k_ticks_to_ns_floor64((uint64_t)(k_uptime_ticks() - os->origin));
    return (ns > (uint64_t)INT64_MAX) ? INT64_MAX : (int64_t)ns;
}

/* Aus einem Faden oder einer ISR; die Grenze 1 macht zwei `give` zu einem. */
void takt_os_give(struct takt_os *os, int event) {
    k_sem_give(&os->events[event]);
}

void takt_os_take(struct takt_os *os, int event, int64_t deadline_ns) {
    k_timeout_t until = K_FOREVER;
    if (deadline_ns != TAKT_OS_FOREVER) {
        const uint64_t ticks = k_ns_to_ticks_ceil64((uint64_t)((deadline_ns > 0) ? deadline_ns : 0));
        until = K_TIMEOUT_ABS_TICKS(os->origin + (int64_t)ticks);
    }
    (void)k_sem_take(&os->events[event], until);
}

/* Der Einstieg jedes Fadens: was `takt_os_spawn` ihm gab. */
static void takt_os_run(void *thread, void *unused1, void *unused2) {
    const struct takt_os_thread *t = thread;
    ARG_UNUSED(unused1);
    ARG_UNUSED(unused2);
    t->body(t->arg);
}

/* Die Prioritaet des Zephyr-Kerns fuer `priority` von Takt
 * (takt_os_types.h); 0 die des rufenden Fadens. */
static int takt_os_zephyr_priority(int priority) {
    return (priority == 0) ? k_thread_priority_get(k_current_get()) : (K_LOWEST_APPLICATION_THREAD_PRIO + 1 - priority);
}

/* `K_FP_REGS`: Mit `CONFIG_FPU_SHARING`, das `CONFIG_TAKT` waehlt, sichert
 * der Zephyr-Kern die FPU-Register des Fadens (12.11). Nicht `K_ESSENTIAL`:
 * Beide Faeden enden mit dem Lauf, und das Ende eines wesentlichen Fadens
 * hielte der Zephyr-Kern fuer einen Fehler. */
int takt_os_spawn(struct takt_os *os, int which, void (*body)(void *arg), void *arg, void *stack, size_t bytes,
                  int priority) {
    struct takt_os_thread *t = &os->threads[which];
    const int zephyr = takt_os_zephyr_priority(priority);
    int rc = -EINVAL;
    if ((bytes > K_KERNEL_STACK_RESERVED) && (zephyr >= K_HIGHEST_THREAD_PRIO) &&
        (zephyr <= K_LOWEST_APPLICATION_THREAD_PRIO) && ((which != TAKT_OS_JOB) || (zephyr >= 0))) {
        t->body = body;
        t->arg = arg;
        (void)k_thread_create(&t->thread, (k_thread_stack_t *)stack, bytes - K_KERNEL_STACK_RESERVED, takt_os_run, t,
                              NULL, NULL, zephyr, K_FP_REGS, K_NO_WAIT);
        rc = 0;
    }
    return rc;
}

int takt_os_join(struct takt_os *os, int which) {
    return k_thread_join(&os->threads[which].thread, K_FOREVER);
}

/* Der Zephyr-Kern fuellt den Stack beim Anlegen nicht (`CONFIG_INIT_STACKS`
 * schliesst Kconfig aus): die gemeinsame Vorgabe, ueber das ganze
 * Stackobjekt (takt_os_types.h). */
void takt_os_paint(struct takt_os *os, int which, void *stack, size_t bytes) {
    ARG_UNUSED(os);
    ARG_UNUSED(which);
    takt_rtos_paint(stack, bytes);
}

int takt_os_depth(struct takt_os *os, int which, const void *stack, size_t bytes, uint32_t *depth) {
    ARG_UNUSED(os);
    ARG_UNUSED(which);
    return takt_rtos_depth(stack, bytes, depth);
}
