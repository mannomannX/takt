/* Takt unter Zephyr: das Pruefprogramm der Portpruefung (12.11, 13.8; M11
 * Schritt 19).
 *
 * `takt_add_program` liefert `port_test` (Bibliothek, `port_test.h`,
 * `takt_sdk.h`), das SDK-Archiv bringt den Kern, das Modul den Port. Diese
 * Anwendung stellt, was der Wirt stellt (12.11): die Leitung, den Watchdog,
 * das Geraet des Journals und beide Stacks. Waehrend des Laufs rechnet
 * `main` die fremde Last, danach schreibt es die Bilanz, und `takt port-test
 * --judge` urteilt ueber alles, was ueber die Leitung kam.
 */
#include <stdint.h>
#include <string.h>

#include <zephyr/kernel.h>

#include "handback.h"
#include "line.h"
#include "port_test.h"
#include "takt_rtos.h"
#include "takt_sdk.h"

/* So viele Ticks faehrt der Port: `takt_conformance::port::TICKS`. */
#define TICKS 2000U

/* Der Watchdog des Wirts (12.11): Dieses Beispiel startet keinen. Ein Wirt
 * mit einem fuettert hier den Task-Watchdog (`task_wdt_feed`) oder den der
 * Hardware (`wdt_feed`). */
static void kick(void *ctx) {
    ARG_UNUSED(ctx);
}

/* Das Geraet des Journals (5.9): ein Flash im RAM, zwei Slots, jeder Vorgang
 * sofort fertig. */
#define SLOT 1024U
static uint8_t slots[2][SLOT];
static int32_t pending = TAKT_NVM_IDLE;

static uint8_t erase(void *ctx, uint8_t slot) {
    ARG_UNUSED(ctx);
    (void)memset(slots[slot & 1U], 0xff, SLOT);
    pending = TAKT_NVM_DONE;
    return 1U;
}

static uint8_t program(void *ctx, uint8_t slot, uint32_t offset, const uint8_t *bytes, uint32_t len) {
    ARG_UNUSED(ctx);
    if ((offset > SLOT) || (len > (SLOT - offset))) {
        pending = TAKT_NVM_FAILED;
    } else {
        (void)memcpy(&slots[slot & 1U][offset], bytes, len);
        pending = TAKT_NVM_DONE;
    }
    return 1U;
}

static int32_t poll_nvm(void *ctx) {
    const int32_t state = pending;
    ARG_UNUSED(ctx);
    pending = TAKT_NVM_IDLE;
    return state;
}

static uint8_t read_nvm(void *ctx, uint8_t slot, uint32_t offset, uint8_t *into, uint32_t len) {
    uint8_t ok = 0U;
    ARG_UNUSED(ctx);
    if ((offset <= SLOT) && (len <= (SLOT - offset))) {
        (void)memcpy(into, &slots[slot & 1U][offset], len);
        ok = 1U;
    }
    return ok;
}

/* Die Stacks beider Faeden: die Schranke des Programms und darueber Luft,
 * damit eine tiefere Messung im Urteil steht, statt dass der Stackwaechter
 * sie als Fault meldet (12.3). */
#define STACK_SLACK 1024U
K_THREAD_STACK_DEFINE(tick_stack, PORT_TEST_TICK_STACK_BYTES + STACK_SLACK);
K_THREAD_STACK_DEFINE(job_stack, PORT_TEST_JOB_STACK_BYTES + STACK_SLACK);

/* Faehrt den Kern `core` ueber den Port, waehrend `main` die fremde Last
 * rechnet, und schreibt die Bilanz. Verlangt der Host das Board zurueck,
 * gibt `main` es sofort ab. */
static void drive(takt_core *core) {
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
    if (takt_rtos_start(&rtos, core, &config) == 0) {
        /* Die fremde Aufgabe: rechnet mit der FPU, solange Takt laeuft. */
        takt_foreign foreign = {0U, 0U};
        struct takt_stacks stacks;
        while (takt_rtos_running(&rtos) != 0) {
            takt_foreign_round(&foreign);
            handback_serve();
        }
        if (takt_rtos_join(&rtos, &stacks) == 0) {
            stacks.tick_bound = PORT_TEST_TICK_STACK_BYTES;
            stacks.job_bound = PORT_TEST_JOB_STACK_BYTES;
            takt_core_report(core, &stacks, &foreign);
        }
    }
}

int main(void) {
    static uint8_t ring[4096];
    static uint8_t current[PORT_TEST_PERSIST_BOUND];
    static uint8_t stored[PORT_TEST_PERSIST_BOUND];
    static struct port_test_arena arena;
    static takt_core core;
    static const struct takt_nvm nvm = {NULL, SLOT, -1, erase, program, poll_nvm, read_nvm};
    static const struct takt_watchdog watchdog = {NULL, kick};
    struct takt_line line;
    if ((line_open(&line) == 0) && (takt_line_init(&line, ring, sizeof ring) == TAKT_OK)) {
        takt_board_trace("takt auf zephyr\n");
        takt_line_mark();
        (void)memset(slots, 0xff, sizeof slots);
        if (takt_core_init_journaled(&core, &port_test_ops, &arena, NULL, TAKT_PROFILE_SHARED, &watchdog, &nvm, current,
                                     stored, PORT_TEST_PERSIST_BOUND) == TAKT_OK) {
            drive(&core);
        }
    }
    handback_await();
    return 0;
}
