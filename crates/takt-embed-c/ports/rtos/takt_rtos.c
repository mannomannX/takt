/* takt_rtos.c - der Port fuer Systeme mit Faeden (takt_rtos.h, 12.11). */
#include "takt_rtos.h"

#include <string.h>

/* Womit die Stacks vor dem Lauf bemalt sind (12.3). */
#define TAKT_RTOS_PAINT 0xA5u

/* Das Ende des Laufs in Nanosekunden nach Tick 0. */
static int64_t takt_rtos_end(const struct takt_rtos_config *c) {
    const int64_t most = INT64_MAX / c->tick_ns;
    int64_t end = INT64_MAX;
    if ((c->ticks != 0u) && (c->ticks <= (uint64_t)most)) {
        end = (int64_t)c->ticks * c->tick_ns;
    }
    return end;
}

/* Der Schrittfaden: `takt_core_service` zu jeder Frist, und frueher, wenn
 * der Jobfaden einen Auftrag fertig meldet. Am Ende weckt er den Jobfaden,
 * damit auch der endet, und wartet, bis er sein Ende meldet: Danach misst
 * `takt_rtos_join` beide Stacks, bevor es die Faeden abwartet. */
static void takt_rtos_step(void *arg) {
    struct takt_rtos *r = arg;
    const int64_t end = takt_rtos_end(&r->config);
    int done = 0;
    while (done == 0) {
        const takt_next next = takt_core_service(r->core, takt_os_now(&r->os));
        if (next.jobs != 0u) {
            takt_os_give(&r->os, TAKT_OS_JOB);
        }
        if ((next.ended != 0) || (next.deadline_ns >= end)) {
            done = 1;
        } else {
            takt_os_take(&r->os, TAKT_OS_STEP, next.deadline_ns);
        }
    }
    __atomic_store_n(&r->stop, 1, __ATOMIC_SEQ_CST);
    takt_os_give(&r->os, TAKT_OS_JOB);
    while (__atomic_load_n(&r->job_ended, __ATOMIC_SEQ_CST) == 0) {
        takt_os_take(&r->os, TAKT_OS_STEP, TAKT_OS_FOREVER);
    }
    __atomic_store_n(&r->running, 0, __ATOMIC_SEQ_CST);
}

/* Der Jobfaden: rechnet, wenn der Kern Jobs meldet, und meldet danach dem
 * Schrittfaden, dass der Auftrag fertig ist (4.5, 12.11). Ein Wecken ohne
 * vergebenen Auftrag rechnet nichts (FB-528). Am Ende meldet er es dem
 * Schrittfaden. */
static void takt_rtos_job(void *arg) {
    struct takt_rtos *r = arg;
    int done = 0;
    while (done == 0) {
        takt_os_take(&r->os, TAKT_OS_JOB, TAKT_OS_FOREVER);
        if (__atomic_load_n(&r->stop, __ATOMIC_SEQ_CST) != 0) {
            done = 1;
        } else {
            takt_core_job_work(r->core);
            takt_os_give(&r->os, TAKT_OS_STEP);
        }
    }
    __atomic_store_n(&r->job_ended, 1, __ATOMIC_SEQ_CST);
    takt_os_give(&r->os, TAKT_OS_STEP);
}

/* Startet beide Faeden; scheitert der zweite, endet der erste wieder. */
static int takt_rtos_threads(struct takt_rtos *r) {
    const struct takt_rtos_config *c = &r->config;
    int rc = takt_os_spawn(&r->os, TAKT_OS_JOB, takt_rtos_job, r, c->job_stack, c->job_stack_bytes, c->job_priority);
    if (rc == 0) {
        rc = takt_os_spawn(&r->os, TAKT_OS_STEP, takt_rtos_step, r, c->tick_stack, c->tick_stack_bytes,
                           c->step_priority);
        if (rc != 0) {
            __atomic_store_n(&r->stop, 1, __ATOMIC_SEQ_CST);
            takt_os_give(&r->os, TAKT_OS_JOB);
            (void)takt_os_join(&r->os, TAKT_OS_JOB);
        }
    }
    return rc;
}

int takt_rtos_start(struct takt_rtos *rtos, takt_core *core, const struct takt_rtos_config *config) {
    int rc = TAKT_RTOS_INVALID;
    if ((rtos != NULL) && (core != NULL) && (config != NULL) && (config->tick_stack != NULL) &&
        (config->job_stack != NULL) && (config->tick_ns > 0)) {
        (void)memset(rtos, 0, sizeof *rtos);
        rtos->core = core;
        rtos->config = *config;
        rc = takt_os_init(&rtos->os);
        if (rc == 0) {
            takt_os_paint(&rtos->os, TAKT_OS_STEP, config->tick_stack, config->tick_stack_bytes);
            takt_os_paint(&rtos->os, TAKT_OS_JOB, config->job_stack, config->job_stack_bytes);
            __atomic_store_n(&rtos->running, 1, __ATOMIC_SEQ_CST);
            rc = takt_rtos_threads(rtos);
            if (rc != 0) {
                __atomic_store_n(&rtos->running, 0, __ATOMIC_SEQ_CST);
                takt_os_destroy(&rtos->os);
            }
        }
    }
    return rc;
}

int takt_rtos_running(const struct takt_rtos *rtos) { return __atomic_load_n(&rtos->running, __ATOMIC_SEQ_CST); }

void takt_rtos_paint(void *stack, size_t bytes) { (void)memset(stack, (int)TAKT_RTOS_PAINT, bytes); }

int takt_rtos_depth(const void *stack, size_t bytes, uint32_t *depth) {
    const uint8_t *s = stack;
    size_t untouched = 0u;
    while ((untouched < bytes) && (s[untouched] == (uint8_t)TAKT_RTOS_PAINT)) {
        untouched++;
    }
    /* In 64 Bit: Mit einem `size_t` von 32 Bit waere der Vergleich immer falsch. */
    const size_t used = bytes - untouched;
    const uint64_t wide = used;
    *depth = (wide > (uint64_t)UINT32_MAX) ? UINT32_MAX : (uint32_t)used;
    return 0;
}

/* Die Tiefe eines Stacks fuer die Bilanz; 0 heisst nicht gemessen. */
static int takt_rtos_measure(struct takt_rtos *r, int which, const void *stack, size_t bytes, uint32_t *depth) {
    uint32_t measured = 0u;
    const int rc = takt_os_depth(&r->os, which, stack, bytes, &measured);
    *depth = (rc == 0) ? measured : 0u;
    return rc;
}

int takt_rtos_join(struct takt_rtos *rtos, struct takt_stacks *stacks) {
    int rc = TAKT_RTOS_INVALID;
    if (takt_rtos_running(rtos) == 0) {
        struct takt_stacks measured;
        (void)memset(&measured, 0, sizeof measured);
        const int step_depth = takt_rtos_measure(rtos, TAKT_OS_STEP, rtos->config.tick_stack,
                                                 rtos->config.tick_stack_bytes, &measured.tick);
        const int job_depth = takt_rtos_measure(rtos, TAKT_OS_JOB, rtos->config.job_stack,
                                                rtos->config.job_stack_bytes, &measured.job);
        const int step = takt_os_join(&rtos->os, TAKT_OS_STEP);
        const int job = takt_os_join(&rtos->os, TAKT_OS_JOB);
        takt_os_destroy(&rtos->os);
        if (stacks != NULL) {
            *stacks = measured;
        }
        /* Der erste Fehler zaehlt: erst das Abwarten, dann das Messen. */
        rc = step;
        if (rc == 0) {
            rc = job;
        }
        if (rc == 0) {
            rc = step_depth;
        }
        if (rc == 0) {
            rc = job_depth;
        }
    }
    return rc;
}
