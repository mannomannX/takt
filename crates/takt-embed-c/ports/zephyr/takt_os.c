/* takt_os.c - der Zephyr-Adapter des Ports mit Faeden (takt_os.h,
 * takt_os_types.h).
 *
 * **Zwei Uhren.** Waehlt der Devicetree einen Zaehler als `takt,timer`
 * (`CONFIG_TAKT_TIMER_COUNTER`, FB-733), rechnen Uhr und Frist in dessen
 * Takt ueber die `counter`-API; sonst in Systemticks. `takt_os_clock` nennt
 * dem Wirt, welche gilt.
 *
 * **Der Systemtick:** Darin fuehrt der Zephyr-Kern seine Zeitgeber; der
 * Ursprung ist der Tick von `takt_os_init`. Eine absolute Frist wird auf den
 * naechsten Tick aufgerundet: Der Schritt kommt nie vor ihr, und hoechstens
 * einen Systemtick (`CONFIG_SYS_CLOCK_TICKS_PER_SEC`) nach ihr; ist die
 * Tickperiode des Programms ein Vielfaches des Systemticks, liegt jede Frist
 * auf einem Tick, und die Rundung verspaetet nichts. Abgerundet kaeme der
 * Schritt bis zu einen Tick zu frueh, `takt_core_service` rechnete keine
 * Grenze und nennte dieselbe Frist, und der Schrittfaden, der hoechste von
 * allen, kreiste bis zum Tick der Frist. Die Ticks sind 64 Bit breit
 * (`CONFIG_TIMEOUT_64BIT`, das Kconfig verlangt): Jede Frist wartet der
 * Zephyr-Kern in einem Stueck, und die Uhr laeuft nicht ueber. Nicht
 * `k_cycle_get_64`: Der Zaehler des Systemzeitgebers und die Ticks haben
 * nicht auf jedem Ziel denselben Ursprung — auf dem ESP32 zaehlt er ab dem
 * Einschalten, die Ticks ab dem Start des Treibers —, und eine Frist aus
 * Zyklen (`K_TIMEOUT_ABS_CYC`) laege um den Abstand daneben.
 *
 * **Der Zaehler:** Nur der Schrittfaden liest die Uhr und wartet mit Frist
 * (takt_os.h); er erweitert den Zaehlerstand dabei ohne Atomik auf 64 Bit.
 * Eine Frist wartet er auf einen absoluten Alarm, aufgerundet wie oben, in
 * Stuecken unter dem halben Umlauf, damit die Erweiterung keinen verpasst;
 * der Rueckruf des Alarms gibt das Ereignis des Schritts. Nach dem
 * Aufwachen loescht der Schrittfaden den Alarm. Gibt ein Rueckruf, der
 * zwischen Aufwachen und Loeschen kam, das Ereignis noch einmal, kostet das
 * nur ein zusaetzliches Aufwachen und verschiebt keine Frist: Der
 * Schrittfaden fragt danach `takt_core_service`, und der nennt die Frist
 * neu. */
#include "takt_os.h"
#include "takt_rtos.h"

#include <errno.h>

#include <zephyr/devicetree.h>
#if defined(CONFIG_TAKT_TIMER_COUNTER)
#include <zephyr/drivers/counter.h>
#endif

BUILD_ASSERT(!DT_HAS_CHOSEN(takt_timer) || IS_ENABLED(CONFIG_TAKT_TIMER_COUNTER),
             "takt: der Devicetree waehlt `takt,timer`, aber CONFIG_COUNTER fehlt (zephyr/Kconfig)");

#if defined(CONFIG_TAKT_TIMER_COUNTER)

/* Nanosekunden je Sekunde. */
static const uint64_t takt_os_ns_per_s = 1000000000ULL;

/* Die Ticks seit `takt_os_init`, auf 64 Bit erweitert. Ein Stand, den der
 * Zaehler nicht liefert, gilt als der letzte: Die Uhr steht dann, sie
 * springt nicht. */
static uint64_t takt_os_ticks(struct takt_os *os) {
    uint32_t now = os->last;
    if (counter_get_value(os->timer, &now) != 0) {
        now = os->last;
    }
    const uint64_t span = (uint64_t)os->top + 1U;
    const uint64_t delta = (now >= os->last) ? (uint64_t)(now - os->last) : ((span - os->last) + now);
    os->ticks += delta;
    os->last = now;
    return os->ticks;
}

/* Nanosekunden `ns` als Ticks, aufgerundet: Eine Frist kommt nie zu frueh.
 * Erst die ganzen Sekunden, damit nichts ueberlaeuft. */
static uint64_t takt_os_ns_to_ticks(uint64_t ns, uint64_t hz) {
    const uint64_t rest = (ns % takt_os_ns_per_s) * hz;
    return ((ns / takt_os_ns_per_s) * hz) + ((rest + takt_os_ns_per_s - 1U) / takt_os_ns_per_s);
}

/* Ticks als Nanosekunden, abgerundet. */
static uint64_t takt_os_ticks_to_ns(uint64_t ticks, uint64_t hz) {
    return ((ticks / hz) * takt_os_ns_per_s) + (((ticks % hz) * takt_os_ns_per_s) / hz);
}

/* Der Rueckruf des Alarms, aus der ISR des Zaehlers: Er merkt sich, dass die
 * Frist kam, und gibt das Ereignis des Schritts. */
static void takt_os_alarm(const struct device *dev, uint8_t channel, uint32_t ticks, void *user_data) {
    struct takt_os *os = user_data;
    ARG_UNUSED(dev);
    ARG_UNUSED(channel);
    ARG_UNUSED(ticks);
    (void)atomic_set(&os->alarmed, 1);
    k_sem_give(&os->events[TAKT_OS_STEP]);
}

/* Der Zaehler laeuft frei bis `top`; ein Alarm weiter als die Haelfte vorn
 * gilt als verpasst und kommt sofort (`COUNTER_ALARM_CFG_EXPIRE_WHEN_LATE`). */
int takt_os_init(struct takt_os *os) {
    int rc = k_sem_init(&os->events[TAKT_OS_STEP], 0U, 1U);
    if (rc == 0) {
        rc = k_sem_init(&os->events[TAKT_OS_JOB], 0U, 1U);
    }
    os->timer = DEVICE_DT_GET(DT_CHOSEN(takt_timer));
    os->ticks = 0U;
    (void)atomic_set(&os->alarmed, 0);
    if ((rc == 0) && !device_is_ready(os->timer)) {
        rc = -ENODEV;
    }
    if (rc == 0) {
        os->top = counter_get_top_value(os->timer);
        rc = counter_set_guard_period(os->timer, os->top / 2U, COUNTER_GUARD_PERIOD_LATE_TO_SET);
    }
    if (rc == 0) {
        rc = counter_start(os->timer);
    }
    if (rc == 0) {
        rc = counter_get_value(os->timer, &os->last);
    }
    return rc;
}

int64_t takt_os_now(struct takt_os *os) {
    const uint64_t ns = takt_os_ticks_to_ns(takt_os_ticks(os), counter_get_frequency(os->timer));
    return (ns > (uint64_t)INT64_MAX) ? INT64_MAX : (int64_t)ns;
}

/* Mit Frist (nur der Schrittfaden): ein absoluter Alarm auf den
 * aufgerundeten Zaehlerstand, hoechstens ein Viertel des Umlaufs voraus;
 * kam statt seiner ein `give`, ist das Warten zu Ende, sonst das naechste
 * Stueck. Eine erreichte Frist nimmt ein gesetztes Ereignis noch mit. Setzt
 * der Zaehler keinen Alarm, wartet `take` nicht: Der Schrittfaden fragt
 * `takt_core_service` erneut, richtig, aber ohne zu schlafen. */
void takt_os_take(struct takt_os *os, int event, int64_t deadline_ns) {
    if (deadline_ns == TAKT_OS_FOREVER) {
        (void)k_sem_take(&os->events[event], K_FOREVER);
    } else {
        const uint64_t due =
            takt_os_ns_to_ticks((uint64_t)((deadline_ns > 0) ? deadline_ns : 0), counter_get_frequency(os->timer));
        const uint64_t chunk = (uint64_t)os->top / 4U;
        const uint8_t channel = 0U; /* der Kanal des Alarms */
        int waiting = 1;
        while (waiting != 0) {
            const uint64_t now = takt_os_ticks(os);
            if (due <= now) {
                (void)k_sem_take(&os->events[event], K_NO_WAIT);
                waiting = 0;
            } else {
                const uint64_t left = due - now;
                const uint64_t step = (left > chunk) ? chunk : left;
                const struct counter_alarm_cfg alarm = {
                    .callback = takt_os_alarm,
                    .ticks = (uint32_t)(((uint64_t)os->last + step) % ((uint64_t)os->top + 1U)),
                    .user_data = os,
                    .flags = COUNTER_ALARM_CFG_ABSOLUTE | COUNTER_ALARM_CFG_EXPIRE_WHEN_LATE,
                };
                (void)atomic_set(&os->alarmed, 0);
                const int rc = counter_set_channel_alarm(os->timer, channel, &alarm);
                if ((rc == 0) || (rc == -ETIME)) {
                    (void)k_sem_take(&os->events[event], K_FOREVER);
                    (void)counter_cancel_channel_alarm(os->timer, channel);
                }
                waiting = ((rc == 0) && (atomic_get(&os->alarmed) != 0) && (left > chunk)) ? 1 : 0;
            }
        }
    }
}

const char *takt_os_clock(uint32_t *hz) {
    *hz = counter_get_frequency(DEVICE_DT_GET(DT_CHOSEN(takt_timer)));
    return "counter";
}

#else /* der Systemtick */

int takt_os_init(struct takt_os *os) {
    int rc = k_sem_init(&os->events[TAKT_OS_STEP], 0U, 1U);
    if (rc == 0) {
        rc = k_sem_init(&os->events[TAKT_OS_JOB], 0U, 1U);
    }
    os->origin = k_uptime_ticks();
    return rc;
}

int64_t takt_os_now(struct takt_os *os) {
    const int64_t ticks = k_uptime_ticks() - os->origin;
    const uint64_t ns = k_ticks_to_ns_floor64((uint64_t)ticks);
    return (ns > (uint64_t)INT64_MAX) ? INT64_MAX : (int64_t)ns;
}

void takt_os_take(struct takt_os *os, int event, int64_t deadline_ns) {
    k_timeout_t until = K_FOREVER;
    if (deadline_ns != TAKT_OS_FOREVER) {
        const uint64_t ticks = k_ns_to_ticks_ceil64((uint64_t)((deadline_ns > 0) ? deadline_ns : 0));
        until = K_TIMEOUT_ABS_TICKS(os->origin + (int64_t)ticks);
    }
    (void)k_sem_take(&os->events[event], until);
}

const char *takt_os_clock(uint32_t *hz) {
    *hz = CONFIG_SYS_CLOCK_TICKS_PER_SEC;
    return "systemtick";
}

#endif

/* Semaphore und Faeden des Zephyr-Kerns geben nichts frei. */
void takt_os_destroy(struct takt_os *os) {
    ARG_UNUSED(os);
}

/* Aus einem Faden oder einer ISR; die Grenze 1 macht zwei `give` zu einem. */
void takt_os_give(struct takt_os *os, int event) {
    k_sem_give(&os->events[event]);
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
