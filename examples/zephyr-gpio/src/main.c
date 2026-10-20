/* Takt unter Zephyr mit Treibern aus dem Devicetree (8.10, 12.11; M11
 * Schritt 19): die Messschleife `takt/wire.takt` auf der Drahtbruecke des
 * Boards.
 *
 * `takt_add_program` liefert `wire` (Bibliothek, `wire.h`, `takt_sdk.h`) und
 * unter Zephyr die Treiber beider Pins (`wire_zephyr.c`, `wire_zephyr.h`);
 * das SDK-Archiv bringt den Kern, das Modul den Port. Diese Anwendung stellt
 * den Rest, den der Wirt stellt (12.11): die Senke vor der Leitung, den
 * Watchdog und beide Stacks. Der Trace zeichnet jeden Pegel auf, den der Eingang las; der
 * Board-Test vergleicht ihn mit dem Interpreter und mit dem Pegel, den der
 * Ausgang im Tick davor schrieb.
 */
#include <stdint.h>

#include <zephyr/kernel.h>

#include "handback.h"
#include "line.h"
#include "takt_rtos.h"
#include "takt_sdk.h"
#include "wire.h"
#include "wire_zephyr.h"

/* So viele Ticks faehrt der Port. */
#define TICKS 2000U

/* So lange schlaeft `main` zwischen zwei Blicken auf den Lauf. */
#define POLL_MS 10

/* Luft ueber der Schranke des Programms in jedem Stack, damit eine tiefere
 * Messung in der Bilanz steht, statt dass der Stackwaechter sie als Fault
 * meldet (12.3). */
#define STACK_SLACK 1024U

/* Die Senke des Programms (12.5, 12.11): ein Ring vor der Leitung des
 * Wirts, gebunden mit `takt_sink_init`; in ihr stehen Kopf, Trace und
 * Bilanz. */
static takt_sink sink;

/* Der Watchdog des Wirts (12.11): Dieses Beispiel startet keinen. */
static void kick(void *ctx) {
    ARG_UNUSED(ctx);
}

/* Faehrt den Kern `core` ueber den Port und schreibt die Bilanz. Verlangt
 * der Host das Board zurueck, gibt `main` es sofort ab. */
static void drive(takt_core *core) {
    /* Kernel-Stacks fuer Supervisor-Faeden (`ports/zephyr/takt_os_types.h`). */
    static K_KERNEL_STACK_DEFINE(tick_stack, WIRE_TICK_STACK_BYTES + STACK_SLACK);
    static K_KERNEL_STACK_DEFINE(job_stack, WIRE_JOB_STACK_BYTES + STACK_SLACK);
    static struct takt_rtos rtos;
    const struct takt_rtos_config config = {
        .ticks = TICKS,
        .tick_ns = WIRE_TICK_NS,
        .tick_stack = tick_stack,
        .tick_stack_bytes = sizeof tick_stack,
        .job_stack = job_stack,
        .job_stack_bytes = sizeof job_stack,
        .step_priority = TAKT_OS_STEP_PRIORITY,
        .job_priority = TAKT_OS_JOB_PRIORITY,
    };
    struct takt_stacks stacks;
    const struct takt_stacks *measured = NULL;
    if (takt_rtos_start(&rtos, core, &config) == 0) {
        while (takt_rtos_running(&rtos) != 0) {
            handback_serve();
            (void)k_msleep(POLL_MS);
        }
        if (takt_rtos_join(&rtos, &stacks) == 0) {
            stacks.tick_bound = WIRE_TICK_STACK_BYTES;
            stacks.job_bound = WIRE_JOB_STACK_BYTES;
            measured = &stacks;
        }
    } else {
        takt_trace_text(&sink, "takt port: kein Start\n");
    }
    takt_core_report(core, measured, NULL);
}

int main(void) {
    static uint8_t ring[4096];
    static struct wire_arena arena;
    static takt_core core;
    static const struct takt_watchdog watchdog = {NULL, kick};
    struct takt_line line;
    const char *channel = NULL;
    if ((line_open(&line) == 0) && (takt_sink_init(&sink, &line, ring, sizeof ring) == TAKT_OK)) {
        takt_trace_text(&sink, "takt auf zephyr mit pins aus dem devicetree\n" TAKT_RTOS_PROTECTION "\n");
        if (wire_zephyr_init(&channel) != 0) {
            /* Die Pins vor dem Kern (8.10): Ein Kanal ohne Pin beginnt nicht. */
            takt_trace_text(&sink, "takt pins: nicht eingerichtet: ");
            takt_trace_text(&sink, channel);
            takt_trace_text(&sink, "\n");
        } else {
            takt_sink_mark(&sink);
            if (takt_core_init(&core, &wire_ops, &arena, NULL, &sink, TAKT_PROFILE_SHARED, &watchdog) == TAKT_OK) {
                drive(&core);
            }
        }
    }
    handback_await();
    return 0;
}
