/* takt_os_types.h - der Zustand des Zephyr-Adapters (takt_os.h, M11 Schritt
 * 19): die Uhr in Systemticks ab `takt_os_init` oder, wenn der Devicetree
 * einen Zaehler als `takt,timer` waehlt, in dessen Takt
 * (`CONFIG_TAKT_TIMER_COUNTER`, FB-733); je Ereignis ein Semaphor mit
 * Grenze 1, je Faden sein Objekt im Zephyr-Kern. Das Modul (`zephyr/` an der
 * Wurzel von Takt) baut den Adapter mit `ports/rtos/takt_rtos.c`, wenn
 * `CONFIG_TAKT` gesetzt ist.
 *
 * **Die Faeden** sind Supervisor-Faeden: Mit `CONFIG_USERSPACE` erreicht ein
 * Faden im Benutzermodus die Arena nicht, wenn sie in einer eigenen
 * Partition liegt, die keine seiner Domaenen enthaelt (12.11, `Schutz:
 * Partition`); gegen privilegierten Code schuetzt sie nicht.
 *
 * **Die Stacks** definiert der Wirt darum als Kernel-Stacks mit
 * `K_KERNEL_STACK_DEFINE` (ohne die Zweierpotenzen, die eine MPU fuer Stacks
 * im Benutzermodus verlangt) und gibt dem Port das ganze Objekt: `stack` ist
 * das Symbol, `bytes` seine Groesse (`sizeof`), samt dem Bereich, den der
 * Zephyr-Kern unten fuer den Stackwaechter haelt
 * (`K_KERNEL_STACK_RESERVED`). So bemalt und misst der Port den Stack bis zu
 * seinem oberen Ende; den Faden legt der Adapter mit
 * `K_KERNEL_STACK_SIZEOF` an. Was der Zephyr-Kern oben im Stack fuer sich
 * nimmt (TLS, `CONFIG_STACK_POINTER_RANDOM`), zaehlt als Tiefe mit: eine
 * Ueberschaetzung, die sichere Richtung. Der Zephyr-Kern darf den Stack beim
 * Anlegen nicht selbst bemalen (`CONFIG_INIT_STACKS`,
 * `CONFIG_STACK_SENTINEL`): Dann saehe der Port seine Farbe nicht mehr.
 *
 * **Die Prioritaeten** zaehlt Takt aufwaerts (groesser ist wichtiger, 0 die
 * des Systems), der Zephyr-Kern abwaerts (kleiner ist wichtiger,
 * kooperativ unter 0). Eine Prioritaet `p` >= 1 von Takt ist im Zephyr-Kern
 * `K_LOWEST_APPLICATION_THREAD_PRIO + 1 - p`: `p` = 1 die niedrigste einer
 * Anwendung, `TAKT_OS_PRIORITY(K_HIGHEST_THREAD_PRIO)` die hoechste
 * kooperative. 0 nimmt die des Fadens, der den Port startet.
 * `TAKT_OS_PRIORITY` rechnet eine Prioritaet des Zephyr-Kerns um. Der
 * Jobfaden muss verdraengbar sein (Zephyr-Prioritaet >= 0), sonst
 * unterbraeche ihn der Schritt nicht (12.11). */
#ifndef TAKT_OS_TYPES_H
#define TAKT_OS_TYPES_H

#include <stdint.h>

#include <zephyr/kernel.h>
#if defined(CONFIG_TAKT_TIMER_COUNTER)
#include <zephyr/device.h>
#endif

/* Die Prioritaet `zephyr` des Zephyr-Kerns als Prioritaet von Takt. */
#define TAKT_OS_PRIORITY(zephyr) (K_LOWEST_APPLICATION_THREAD_PRIO + 1 - (zephyr))

/* Die Prioritaeten aus der Konfiguration des Moduls (Kconfig): der
 * Schrittfaden unter den kooperativen, der Jobfaden unter den
 * verdraengbaren. */
#define TAKT_OS_STEP_PRIORITY TAKT_OS_PRIORITY(K_PRIO_COOP(CONFIG_TAKT_STEP_PRIORITY))
#define TAKT_OS_JOB_PRIORITY TAKT_OS_PRIORITY(K_PRIO_PREEMPT(CONFIG_TAKT_JOB_PRIORITY))

/* Mit `CONFIG_USERSPACE` erreicht ein Faden im Benutzermodus nur die
 * Partitionen seiner Domaene, die Arena also nicht (12.11). */
#if defined(CONFIG_USERSPACE)
#define TAKT_OS_PARTITION 1
#else
#define TAKT_OS_PARTITION 0
#endif

/* Ein Faden: sein Objekt und was er ausfuehrt. */
struct takt_os_thread {
    struct k_thread thread;
    void (*body)(void *arg);
    void *arg;
};

struct takt_os {
#if defined(CONFIG_TAKT_TIMER_COUNTER)
    const struct device *timer; /* der Zaehler, den `takt,timer` waehlt */
    uint64_t ticks;             /* seine Ticks seit `takt_os_init`, auf 64 Bit erweitert */
    uint32_t last;              /* sein Stand bei der letzten Erweiterung */
    uint32_t top;               /* sein hoechster Stand vor dem Umlauf */
    atomic_t alarmed;           /* der Alarm gab das Ereignis, kein `give` */
#else
    int64_t origin; /* der Systemtick von `takt_os_init` */
#endif
    struct k_sem events[2];
    struct takt_os_thread threads[2];
};

/* Die Uhr des Adapters fuer die Kopfzeile des Wirts: `counter` oder
 * `systemtick`, und in `*hz` ihre Rate. */
const char *takt_os_clock(uint32_t *hz);

#endif /* TAKT_OS_TYPES_H */
