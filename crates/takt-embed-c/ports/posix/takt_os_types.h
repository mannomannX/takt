/* takt_os_types.h - der Zustand des POSIX-Adapters (takt_os.h): die Uhr ab
 * `takt_os_init` auf `CLOCK_MONOTONIC`, je Ereignis Sperre, Bedingung und
 * Flag, je Faden sein Griff. Geprueft unter Linux; ebenso fuer NuttX, RTEMS
 * und QNX gedacht, die dieselbe Schnittstelle bieten. */
#ifndef TAKT_OS_TYPES_H
#define TAKT_OS_TYPES_H

#include <pthread.h>
#include <time.h>

/* Faeden eines Prozesses teilen seinen Speicher (12.11). */
#define TAKT_OS_PARTITION 0

/* Ein binaeres Ereignis: das Flag unter der Sperre, die Bedingung wartet auf
 * `CLOCK_MONOTONIC`. */
struct takt_os_event {
    pthread_mutex_t lock;
    pthread_cond_t cond;
    int given;
};

/* Ein Faden: sein Griff und was er ausfuehrt. */
struct takt_os_thread {
    pthread_t id;
    void (*body)(void *arg);
    void *arg;
};

struct takt_os {
    struct timespec origin;
    struct takt_os_event events[2];
    struct takt_os_thread threads[2];
};

#endif /* TAKT_OS_TYPES_H */
