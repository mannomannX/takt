/* Takt unter Zephyr: das Pruefprogramm der Portpruefung (12.11, 13.8; M11
 * Schritt 19).
 *
 * `takt_add_program` liefert `port_test` (Bibliothek, `port_test.h`,
 * `takt_sdk.h`), das SDK-Archiv bringt den Kern, das Modul den Port. Diese
 * Anwendung stellt, was der Wirt stellt (12.11): die Leitung, den Watchdog,
 * das Geraet des Journals, beide Stacks und die Arena in ihrer Partition.
 * Vor dem Lauf belegt eine Probe im Benutzermodus, dass die Partition sie
 * abweist (Kopfzeile `takt schutz partition …`). Waehrend des Laufs rechnet
 * `main` die fremde Last, danach schreibt es die Bilanz, und `takt port-test
 * --judge` urteilt ueber alles, was ueber die Leitung kam.
 */
#include <stdint.h>

#include <zephyr/kernel.h>
#include <zephyr/sys/printk.h>

#include "handback.h"
#include "line.h"
#include "partition.h"
#include "port_test.h"
#include "takt_rtos.h"
#include "takt_sdk.h"
#include "takt_zephyr_nvm.h"

/* So viele Ticks faehrt der Port: `takt_conformance::port::TICKS`. */
#define TICKS 2000U

/* Die Senke des Programms (12.5, 12.11): ein Ring vor der Leitung des
 * Wirts, gebunden mit `takt_sink_init`; in ihr stehen Kopf, Trace und
 * Bilanz. */
static takt_sink sink;

/* Der Watchdog des Wirts (12.11): Dieses Beispiel startet keinen. Ein Wirt
 * mit einem fuettert hier den Task-Watchdog (`task_wdt_feed`) oder den der
 * Hardware (`wdt_feed`). */
static void kick(void *ctx) {
    ARG_UNUSED(ctx);
}

/* Oeffnet das Geraet des Journals (5.9): zwei Slots im Flash, in der
 * Partition `takt_journal` des Devicetree (`ports/zephyr-nvm`, FB-732). Ein
 * Bau mit `TAKT_FRESH_JOURNAL` loescht sie vor dem Lauf: Die Portpruefung
 * vergleicht mit dem Interpreter ohne gespeicherte Werte; ohne ueberlebt
 * `persist` einen Neustart. 0 oder -errno. */
static int journal_open(struct takt_nvm *nvm) {
    static struct takt_zephyr_nvm journal;
    int rc = takt_zephyr_nvm_open(&journal, PARTITION_ID(takt_journal), PORT_TEST_NVM_BLOCKING_NS, nvm);
    if ((rc == 0) && (TAKT_FRESH_JOURNAL != 0)) {
        rc = takt_zephyr_nvm_clear(&journal);
    }
    return rc;
}

/* Luft ueber der Schranke des Programms in jedem Stack, damit eine tiefere
 * Messung im Urteil steht, statt dass der Stackwaechter sie als Fault
 * meldet (12.3). */
#define STACK_SLACK 1024U

/* Die Arena in ihrer Partition (12.11): `init` beschreibt sie ganz, darum
 * im Abschnitt ohne Anfangswerte. */
K_APP_BMEM(takt_arena) static struct port_test_arena arena;

/* Was die Probe der Partition sah, als Kopfzeile vor der Marke. */
static const char *protection(void) {
    const int seen = partition_probe(&arena);
    const char *line = "takt schutz partition ungeprueft\n";
    if (seen == PARTITION_REFUSED) {
        line = "takt schutz partition verweigert\n";
    } else if (seen == PARTITION_OPEN) {
        line = "takt schutz partition offen\n";
    } else {
        /* Die Probe endete nicht: kein Urteil. */
    }
    return line;
}

/* Die Uhr des Ports als Kopfzeile: Quelle und Rate (FB-733), etwa
 * `takt uhr counter 1000000 Hz`. */
static void clock_line(void) {
    char line[48];
    uint32_t hz = 0U;
    const char *source = takt_os_clock(&hz);
    (void)snprintk(line, sizeof line, "takt uhr %s %u Hz\n", source, (unsigned int)hz);
    takt_trace_text(&sink, line);
}

/* Faehrt den Kern `core` ueber den Port, waehrend `main` die fremde Last
 * rechnet, und schreibt die Bilanz. Verlangt der Host das Board zurueck,
 * gibt `main` es sofort ab. Startet der Port nicht oder misst er die Stacks
 * nicht, steht die Bilanz trotzdem da, und das Urteil nennt, was fehlt. */
static void drive(takt_core *core) {
    /* Kernel-Stacks fuer Supervisor-Faeden (`ports/zephyr/takt_os_types.h`). */
    static K_KERNEL_STACK_DEFINE(tick_stack, PORT_TEST_TICK_STACK_BYTES + STACK_SLACK);
    static K_KERNEL_STACK_DEFINE(job_stack, PORT_TEST_JOB_STACK_BYTES + STACK_SLACK);
    static struct takt_rtos rtos;
    const struct takt_rtos_config config = {
        .ticks = TICKS,
        .tick_ns = PORT_TEST_TICK_NS,
        .tick_stack = tick_stack,
        .tick_stack_bytes = sizeof tick_stack,
        .job_stack = job_stack,
        .job_stack_bytes = sizeof job_stack,
        .step_priority = TAKT_OS_STEP_PRIORITY,
        .job_priority = TAKT_OS_JOB_PRIORITY,
    };
    takt_foreign foreign = {0U, 0U};
    struct takt_stacks stacks;
    const struct takt_stacks *measured = NULL;
    if (takt_rtos_start(&rtos, core, &config) == 0) {
#if TAKT_PROBE_IN_RUN
        /* Ein Bau mit `TAKT_PROBE_IN_RUN` probt die Partition noch einmal im
         * Lauf: Die Schutzeinheit weist den Zugriff ab, und der Kern meldet
         * `Runtime(Hardware)` (12.3, `partition_guard`). */
        (void)partition_probe(&arena);
#endif
        /* Die fremde Aufgabe: rechnet mit der FPU, solange Takt laeuft. */
        while (takt_rtos_running(&rtos) != 0) {
            takt_foreign_round(&foreign);
            handback_serve();
        }
        if (takt_rtos_join(&rtos, &stacks) == 0) {
            stacks.tick_bound = PORT_TEST_TICK_STACK_BYTES;
            stacks.job_bound = PORT_TEST_JOB_STACK_BYTES;
            measured = &stacks;
        }
    } else {
        takt_trace_text(&sink, "takt port: kein Start\n");
    }
    takt_core_report(core, measured, &foreign);
}

int main(void) {
    static uint8_t ring[4096];
    static uint8_t current[PORT_TEST_PERSIST_BOUND];
    static uint8_t stored[PORT_TEST_PERSIST_BOUND];
    static takt_core core;
    static struct takt_nvm nvm;
    static const struct takt_watchdog watchdog = {NULL, kick};
    struct takt_line line;
    if ((line_open(&line) == 0) && (takt_sink_init(&sink, &line, ring, sizeof ring) == TAKT_OK)) {
        takt_trace_text(&sink, "takt auf zephyr\n" TAKT_RTOS_PROTECTION "\n");
        takt_trace_text(&sink, protection());
        clock_line();
        takt_sink_mark(&sink);
        if ((journal_open(&nvm) == 0) &&
            (takt_core_init_journaled(&core, &port_test_ops, &arena, NULL, &sink, TAKT_PROFILE_SHARED, &watchdog, &nvm,
                                      current, stored, PORT_TEST_PERSIST_BOUND) == TAKT_OK)) {
            partition_guard(&core);
            drive(&core);
        }
    }
    handback_await();
    return 0;
}
