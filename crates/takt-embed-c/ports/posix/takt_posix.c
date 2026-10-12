/* takt_posix.c - der POSIX-Port (takt_posix.h, 12.11, M11 Schritt 18c). */
#define _POSIX_C_SOURCE 200809L

#include "takt_posix.h"

#include <errno.h>
#include <sched.h>
#include <string.h>
#include <unistd.h>

/* Womit die Stacks vor dem Lauf bemalt sind (12.3). */
#define TAKT_POSIX_PAINT 0xA5u

#define TAKT_NS_PER_S 1000000000LL

/* Nanosekunden seit Tick 0. `CLOCK_MONOTONIC` hat `takt_posix_start`
 * geprueft; danach scheitert die Uhr nicht mehr. */
static int64_t takt_posix_now(const struct takt_posix *p) {
    struct timespec now = p->origin;
    (void)clock_gettime(CLOCK_MONOTONIC, &now);
    return ((int64_t)now.tv_sec - (int64_t)p->origin.tv_sec) * TAKT_NS_PER_S +
           ((int64_t)now.tv_nsec - (int64_t)p->origin.tv_nsec);
}

/* Der Augenblick `ns` nach Tick 0 auf `CLOCK_MONOTONIC`; `ns` >= 0. */
static struct timespec takt_posix_at(const struct takt_posix *p, int64_t ns) {
    struct timespec at = p->origin;
    const int64_t nsec = (int64_t)at.tv_nsec + (ns % TAKT_NS_PER_S);
    at.tv_sec += (time_t)((ns / TAKT_NS_PER_S) + (nsec / TAKT_NS_PER_S));
    at.tv_nsec = (long)(nsec % TAKT_NS_PER_S);
    return at;
}

/* Das Ende des Laufs in Nanosekunden nach Tick 0. */
static int64_t takt_posix_end(const struct takt_posix_config *c) {
    if (c->ticks == 0u || c->ticks > (uint64_t)(INT64_MAX / c->tick_ns)) {
        return INT64_MAX;
    }
    return (int64_t)c->ticks * c->tick_ns;
}

/* Der Schrittfaden: `takt_core_service` zu jeder Frist, und frueher, wenn
 * ein Auftrag fertig ist. Die Sperre haelt er nur zwischen zwei Aufrufen. */
static void *takt_posix_step(void *arg) {
    struct takt_posix *p = arg;
    const int64_t end = takt_posix_end(&p->config);
    (void)pthread_mutex_lock(&p->lock);
    for (;;) {
        takt_next next;
        struct timespec deadline;
        p->job_done = 0u;
        (void)pthread_mutex_unlock(&p->lock);
        next = takt_core_service(p->core, takt_posix_now(p));
        (void)pthread_mutex_lock(&p->lock);
        if (next.jobs != 0u) {
            p->job_pending = 1u;
            (void)pthread_cond_signal(&p->job_wake);
        }
        if ((next.ended != 0) || (next.deadline_ns >= end)) {
            break;
        }
        deadline = takt_posix_at(p, next.deadline_ns);
        while (p->job_done == 0u) {
            if (pthread_cond_timedwait(&p->step_wake, &p->lock, &deadline) != 0) {
                break;
            }
        }
    }
    p->stop = 1u;
    (void)pthread_cond_signal(&p->job_wake);
    (void)pthread_mutex_unlock(&p->lock);
    __atomic_store_n(&p->running, 0, __ATOMIC_SEQ_CST);
    return NULL;
}

/* Der Jobfaden: rechnet, wenn der Kern Jobs meldet, und meldet danach dem
 * Schrittfaden, dass der Auftrag fertig ist (4.5, 12.11). */
static void *takt_posix_job(void *arg) {
    struct takt_posix *p = arg;
    (void)pthread_mutex_lock(&p->lock);
    for (;;) {
        while ((p->job_pending == 0u) && (p->stop == 0u)) {
            (void)pthread_cond_wait(&p->job_wake, &p->lock);
        }
        if (p->stop != 0u) {
            break;
        }
        p->job_pending = 0u;
        (void)pthread_mutex_unlock(&p->lock);
        takt_core_job_work(p->core);
        (void)pthread_mutex_lock(&p->lock);
        p->job_done = 1u;
        (void)pthread_cond_signal(&p->step_wake);
    }
    (void)pthread_mutex_unlock(&p->lock);
    return NULL;
}

/* Startet `body` auf dem Stack des Wirts, mit `priority` unter SCHED_FIFO. */
static int takt_posix_spawn(pthread_t *thread, void *(*body)(void *), struct takt_posix *p, void *stack,
                            size_t bytes, int priority) {
    pthread_attr_t attr;
    int rc = pthread_attr_init(&attr);
    if (rc != 0) {
        return rc;
    }
    rc = pthread_attr_setstack(&attr, stack, bytes);
    if ((rc == 0) && (priority > 0)) {
        struct sched_param param;
        (void)memset(&param, 0, sizeof param);
        param.sched_priority = priority;
        rc = pthread_attr_setinheritsched(&attr, PTHREAD_EXPLICIT_SCHED);
        if (rc == 0) {
            rc = pthread_attr_setschedpolicy(&attr, SCHED_FIFO);
        }
        if (rc == 0) {
            rc = pthread_attr_setschedparam(&attr, &param);
        }
    }
    if (rc == 0) {
        rc = pthread_create(thread, &attr, body, p);
    }
    (void)pthread_attr_destroy(&attr);
    return rc;
}

/* Sperre und Bedingungen; die Bedingungen warten auf `CLOCK_MONOTONIC`. */
static int takt_posix_sync_init(struct takt_posix *p) {
    pthread_mutexattr_t mutex_attr;
    pthread_condattr_t cond_attr;
    int rc = pthread_mutexattr_init(&mutex_attr);
    if (rc != 0) {
        return rc;
    }
#if defined(_POSIX_THREAD_PRIO_INHERIT) && (_POSIX_THREAD_PRIO_INHERIT > 0)
    /* Mit Prioritaeten erbt der Jobfaden die des Schritts, solange er die
     * Sperre haelt. */
    rc = pthread_mutexattr_setprotocol(&mutex_attr, PTHREAD_PRIO_INHERIT);
#endif
    if (rc == 0) {
        rc = pthread_mutex_init(&p->lock, &mutex_attr);
    }
    (void)pthread_mutexattr_destroy(&mutex_attr);
    if (rc != 0) {
        return rc;
    }
    rc = pthread_condattr_init(&cond_attr);
    if (rc == 0) {
        rc = pthread_condattr_setclock(&cond_attr, CLOCK_MONOTONIC);
        if (rc == 0) {
            rc = pthread_cond_init(&p->step_wake, &cond_attr);
        }
        if (rc == 0) {
            rc = pthread_cond_init(&p->job_wake, &cond_attr);
            if (rc != 0) {
                (void)pthread_cond_destroy(&p->step_wake);
            }
        }
        (void)pthread_condattr_destroy(&cond_attr);
    }
    if (rc != 0) {
        (void)pthread_mutex_destroy(&p->lock);
    }
    return rc;
}

static void takt_posix_sync_destroy(struct takt_posix *p) {
    (void)pthread_cond_destroy(&p->job_wake);
    (void)pthread_cond_destroy(&p->step_wake);
    (void)pthread_mutex_destroy(&p->lock);
}

int takt_posix_start(struct takt_posix *posix, takt_core *core, const struct takt_posix_config *config) {
    int rc;
    if ((posix == NULL) || (core == NULL) || (config == NULL) || (config->tick_stack == NULL) ||
        (config->job_stack == NULL) || (config->tick_ns <= 0)) {
        return EINVAL;
    }
    (void)memset(posix, 0, sizeof *posix);
    posix->core = core;
    posix->config = *config;
    rc = takt_posix_sync_init(posix);
    if (rc != 0) {
        return rc;
    }
    (void)memset(config->tick_stack, (int)TAKT_POSIX_PAINT, config->tick_stack_bytes);
    (void)memset(config->job_stack, (int)TAKT_POSIX_PAINT, config->job_stack_bytes);
    if (clock_gettime(CLOCK_MONOTONIC, &posix->origin) != 0) {
        rc = errno;
        takt_posix_sync_destroy(posix);
        return rc;
    }
    __atomic_store_n(&posix->running, 1, __ATOMIC_SEQ_CST);
    rc = takt_posix_spawn(&posix->job, takt_posix_job, posix, config->job_stack, config->job_stack_bytes,
                          config->job_priority);
    if (rc == 0) {
        rc = takt_posix_spawn(&posix->step, takt_posix_step, posix, config->tick_stack, config->tick_stack_bytes,
                              config->step_priority);
        if (rc != 0) {
            (void)pthread_mutex_lock(&posix->lock);
            posix->stop = 1u;
            (void)pthread_cond_signal(&posix->job_wake);
            (void)pthread_mutex_unlock(&posix->lock);
            (void)pthread_join(posix->job, NULL);
        }
    }
    if (rc != 0) {
        __atomic_store_n(&posix->running, 0, __ATOMIC_SEQ_CST);
        takt_posix_sync_destroy(posix);
    }
    return rc;
}

int takt_posix_running(struct takt_posix *posix) { return __atomic_load_n(&posix->running, __ATOMIC_SEQ_CST); }

/* Wie tief ein Stack reichte: alles ueber den Bytes, die unten noch die
 * Farbe tragen. */
static uint32_t takt_posix_depth(const void *stack, size_t bytes) {
    const uint8_t *s = stack;
    size_t untouched = 0u;
    while ((untouched < bytes) && (s[untouched] == (uint8_t)TAKT_POSIX_PAINT)) {
        untouched++;
    }
    return ((bytes - untouched) > UINT32_MAX) ? UINT32_MAX : (uint32_t)(bytes - untouched);
}

int takt_posix_join(struct takt_posix *posix, struct takt_stacks *stacks) {
    const int step = pthread_join(posix->step, NULL);
    const int job = pthread_join(posix->job, NULL);
    if (stacks != NULL) {
        (void)memset(stacks, 0, sizeof *stacks);
        stacks->tick = takt_posix_depth(posix->config.tick_stack, posix->config.tick_stack_bytes);
        stacks->job = takt_posix_depth(posix->config.job_stack, posix->config.job_stack_bytes);
    }
    takt_posix_sync_destroy(posix);
    return (step != 0) ? step : job;
}
