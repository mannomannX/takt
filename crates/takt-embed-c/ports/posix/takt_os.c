/* takt_os.c - der POSIX-Adapter des Ports mit Faeden (takt_os.h). */
#define _POSIX_C_SOURCE 200809L

#include "takt_os.h"

#include <errno.h>
#include <sched.h>
#include <string.h>
#include <unistd.h>

#define TAKT_NS_PER_S 1000000000LL

/* Der Augenblick `ns` nach `takt_os_init` auf `CLOCK_MONOTONIC`; `ns` >= 0. */
static struct timespec takt_os_at(const struct takt_os *os, int64_t ns) {
    struct timespec at = os->origin;
    const int64_t nsec = (int64_t)at.tv_nsec + (ns % TAKT_NS_PER_S);
    const int64_t sec = (ns / TAKT_NS_PER_S) + (nsec / TAKT_NS_PER_S);
    const int64_t rest = nsec % TAKT_NS_PER_S;
    at.tv_sec += (time_t)sec;
    at.tv_nsec = (long)rest;
    return at;
}

/* Die Sperre eines Ereignisses; mit Prioritaeten erbt, wer sie haelt, die
 * des Wartenden. */
static int takt_os_lock_init(pthread_mutex_t *lock) {
    pthread_mutexattr_t attr;
    int rc = pthread_mutexattr_init(&attr);
    if (rc == 0) {
#if defined(_POSIX_THREAD_PRIO_INHERIT) && (_POSIX_THREAD_PRIO_INHERIT > 0)
        rc = pthread_mutexattr_setprotocol(&attr, PTHREAD_PRIO_INHERIT);
        if (rc == 0) {
            rc = pthread_mutex_init(lock, &attr);
        }
#else
        rc = pthread_mutex_init(lock, &attr);
#endif
        (void)pthread_mutexattr_destroy(&attr);
    }
    return rc;
}

/* Sperre und Bedingung eines Ereignisses; die Bedingung wartet auf
 * `CLOCK_MONOTONIC`. */
static int takt_os_event_init(struct takt_os_event *e) {
    pthread_condattr_t attr;
    int rc = takt_os_lock_init(&e->lock);
    if (rc == 0) {
        rc = pthread_condattr_init(&attr);
        if (rc == 0) {
            rc = pthread_condattr_setclock(&attr, CLOCK_MONOTONIC);
            if (rc == 0) {
                rc = pthread_cond_init(&e->cond, &attr);
            }
            (void)pthread_condattr_destroy(&attr);
        }
        if (rc != 0) {
            (void)pthread_mutex_destroy(&e->lock);
        }
    }
    e->given = 0;
    return rc;
}

static void takt_os_event_destroy(struct takt_os_event *e) {
    (void)pthread_cond_destroy(&e->cond);
    (void)pthread_mutex_destroy(&e->lock);
}

int takt_os_init(struct takt_os *os) {
    int rc;
    (void)memset(os, 0, sizeof *os);
    rc = takt_os_event_init(&os->events[TAKT_OS_STEP]);
    if (rc == 0) {
        rc = takt_os_event_init(&os->events[TAKT_OS_JOB]);
        if (rc != 0) {
            takt_os_event_destroy(&os->events[TAKT_OS_STEP]);
        }
    }
    if ((rc == 0) && (clock_gettime(CLOCK_MONOTONIC, &os->origin) != 0)) {
        rc = errno;
        takt_os_destroy(os);
    }
    return rc;
}

void takt_os_destroy(struct takt_os *os) {
    takt_os_event_destroy(&os->events[TAKT_OS_JOB]);
    takt_os_event_destroy(&os->events[TAKT_OS_STEP]);
}

/* `CLOCK_MONOTONIC` hat `takt_os_init` geprueft; danach scheitert die Uhr
 * nicht mehr. */
int64_t takt_os_now(const struct takt_os *os) {
    struct timespec now = os->origin;
    (void)clock_gettime(CLOCK_MONOTONIC, &now);
    return (((int64_t)now.tv_sec - (int64_t)os->origin.tv_sec) * TAKT_NS_PER_S) +
           ((int64_t)now.tv_nsec - (int64_t)os->origin.tv_nsec);
}

void takt_os_give(struct takt_os *os, int event) {
    struct takt_os_event *e = &os->events[event];
    (void)pthread_mutex_lock(&e->lock);
    e->given = 1;
    (void)pthread_cond_signal(&e->cond);
    (void)pthread_mutex_unlock(&e->lock);
}

void takt_os_take(struct takt_os *os, int event, int64_t deadline_ns) {
    struct takt_os_event *e = &os->events[event];
    (void)pthread_mutex_lock(&e->lock);
    if (deadline_ns == TAKT_OS_FOREVER) {
        while (e->given == 0) {
            (void)pthread_cond_wait(&e->cond, &e->lock);
        }
    } else {
        const struct timespec at = takt_os_at(os, deadline_ns);
        int waiting = 1;
        while ((e->given == 0) && (waiting != 0)) {
            if (pthread_cond_timedwait(&e->cond, &e->lock, &at) != 0) {
                waiting = 0;
            }
        }
    }
    e->given = 0;
    (void)pthread_mutex_unlock(&e->lock);
}

/* Der Einstieg jedes Fadens: was `takt_os_spawn` ihm gab. */
static void *takt_os_run(void *arg) {
    const struct takt_os_thread *t = arg;
    t->body(t->arg);
    return NULL;
}

int takt_os_spawn(struct takt_os *os, int which, void (*body)(void *arg), void *arg, void *stack, size_t bytes,
                  int priority) {
    struct takt_os_thread *t = &os->threads[which];
    pthread_attr_t attr;
    int rc = pthread_attr_init(&attr);
    t->body = body;
    t->arg = arg;
    if (rc == 0) {
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
            rc = pthread_create(&t->id, &attr, takt_os_run, t);
        }
        (void)pthread_attr_destroy(&attr);
    }
    return rc;
}

int takt_os_join(struct takt_os *os, int which) { return pthread_join(os->threads[which].id, NULL); }
