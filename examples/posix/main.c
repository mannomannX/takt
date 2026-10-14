/* Der POSIX-Port mit dem Pruefprogramm der Portpruefung (12.11, 13.8; M11
 * Schritt 18c).
 *
 * `takt port-test --emit embed --form rtos --hardware HW --out DIR` liefert
 * `port_test` (Bibliothek, `port_test.h`, `takt_sdk.h`), auf einem Wirt mit
 * einer Nachricht je Job, die laenger als ein Tick rechnet. Das SDK-Archiv
 * bringt den Kern, `ports/rtos` den Port fuer Systeme mit Faeden (einen
 * Schrittfaden und einen Jobfaden), `ports/posix` seinen Adapter fuer POSIX.
 * Dieses Programm stellt, was der Wirt stellt: die Leitung, das Geraet des
 * Journals und beide Stacks. Waehrend des Laufs rechnet es die
 * fremde Last. Es faehrt so viele Ticks, wie das erste Argument sagt (ohne:
 * 2000), und schreibt den Lauf auf stdout; `takt port-test --judge` urteilt.
 *
 *     clang -std=c11 -O2 -pthread -I DIR -I ports/rtos -I ports/posix main.c \
 *         ports/rtos/takt_rtos.c ports/posix/takt_os.c DIR/libport_test.a libtakt_sdk.a -lm
 */
#define _POSIX_C_SOURCE 200809L

#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "port_test.h"
#include "takt_rtos.h"
#include "takt_sdk.h"

/* Die Leitung (12.2): sammelt in einem Puffer, den `flush` ohne zu warten
 * nach stdout schreibt; was die Pipe nicht nimmt, bleibt fuer das naechste
 * Mal. */
struct out {
    uint8_t bytes[4096];
    size_t len;
};

/* Der Watchdog des Wirts (12.11): Dieses Beispiel hat keinen. Ein Wirt unter
 * Linux meldete hier etwa `WATCHDOG=1` an systemd. */
static void kick(void *ctx) { (void)ctx; }

static uint8_t put(void *ctx, uint8_t byte) {
    struct out *o = ctx;
    uint8_t taken = 0u;
    if (o->len < sizeof o->bytes) {
        o->bytes[o->len] = byte;
        o->len++;
        taken = 1u;
    }
    return taken;
}

static void flush(void *ctx) {
    struct out *o = ctx;
    size_t done = 0u;
    while (done < o->len) {
        const ssize_t n = write(STDOUT_FILENO, &o->bytes[done], o->len - done);
        if (n <= 0) {
            break;
        }
        done += (size_t)n;
    }
    (void)memmove(o->bytes, &o->bytes[done], o->len - done);
    o->len -= done;
}

/* Das Geraet des Journals (5.9): ein Flash im Speicher, zwei Slots, jeder
 * Vorgang sofort fertig. */
#define SLOT 4096u
static uint8_t slots[2][SLOT];
static int32_t pending = TAKT_NVM_IDLE;

static uint8_t erase(void *ctx, uint8_t slot) {
    (void)ctx;
    (void)memset(slots[slot & 1u], 0xff, SLOT);
    pending = TAKT_NVM_DONE;
    return 1u;
}

static uint8_t program(void *ctx, uint8_t slot, uint32_t offset, const uint8_t *bytes, uint32_t len) {
    (void)ctx;
    if ((offset > SLOT) || (len > (SLOT - offset))) {
        pending = TAKT_NVM_FAILED;
    } else {
        (void)memcpy(&slots[slot & 1u][offset], bytes, len);
        pending = TAKT_NVM_DONE;
    }
    return 1u;
}

static int32_t poll_nvm(void *ctx) {
    (void)ctx;
    const int32_t state = pending;
    pending = TAKT_NVM_IDLE;
    return state;
}

static uint8_t read_nvm(void *ctx, uint8_t slot, uint32_t offset, uint8_t *into, uint32_t len) {
    uint8_t ok = 0u;
    (void)ctx;
    if ((offset <= SLOT) && (len <= (SLOT - offset))) {
        (void)memcpy(into, &slots[slot & 1u][offset], len);
        ok = 1u;
    }
    return ok;
}

/* Die Zahl der Ticks aus `text` nach `ticks` (CERT C ERR34-C); 0, wenn sie
 * eine ist. */
static int ticks_of(const char *text, uint64_t *ticks) {
    char *end = NULL;
    int rc = 1;
    errno = 0;
    const unsigned long long value = strtoull(text, &end, 10);
    if ((errno == 0) && (end != text) && (*end == '\0') && (text[0] != '-')) {
        *ticks = (uint64_t)value;
        rc = 0;
    }
    return rc;
}

/* Die Stacks beider Faeden; reichlich ueber PTHREAD_STACK_MIN, gemessen wird
 * die Tiefe (12.3). */
#define STACK_BYTES (256u * 1024u)

/* Faehrt den Kern `core` ueber den Port, waehrend dieser Faden die fremde
 * Last rechnet, und schreibt die Bilanz; 0 oder 2. */
static int drive(takt_core *core, uint64_t ticks) {
    static struct takt_rtos port;
    static _Alignas(64) uint8_t tick_stack[STACK_BYTES];
    static _Alignas(64) uint8_t job_stack[STACK_BYTES];
    const struct takt_rtos_config config = {
        .ticks = ticks,
        .tick_ns = PORT_TEST_TICK_NS,
        .tick_stack = tick_stack,
        .tick_stack_bytes = sizeof tick_stack,
        .job_stack = job_stack,
        .job_stack_bytes = sizeof job_stack,
        .step_priority = 0,
        .job_priority = 0,
    };
    int rc = takt_rtos_start(&port, core, &config);
    if (rc != 0) {
        (void)fprintf(stderr, "takt_rtos_start: %d\n", rc);
    } else {
        /* Die fremde Aufgabe: rechnet mit der FPU, solange Takt laeuft. */
        takt_foreign foreign = {0u, 0u};
        struct takt_stacks stacks;
        while (takt_rtos_running(&port) != 0) {
            takt_foreign_round(&foreign);
        }
        rc = takt_rtos_join(&port, &stacks);
        if (rc != 0) {
            (void)fprintf(stderr, "takt_rtos_join: %d\n", rc);
        } else {
            stacks.tick_bound = PORT_TEST_TICK_STACK_BYTES;
            stacks.job_bound = PORT_TEST_JOB_STACK_BYTES;
            takt_core_report(core, &stacks, &foreign);
        }
    }
    return (rc == 0) ? 0 : 2;
}

/* Was der Wirt fuer den Lauf stellt: Senke, Journal und Kern; 0 oder 2. */
static int run(uint64_t ticks) {
    static struct out out;
    static uint8_t ring[65536];
    static uint8_t current[PORT_TEST_PERSIST_BOUND];
    static uint8_t stored[PORT_TEST_PERSIST_BOUND];
    static struct port_test_arena arena;
    static takt_core core;
    static takt_sink sink;
    static const struct takt_nvm nvm = {NULL, SLOT, -1, erase, program, poll_nvm, read_nvm};
    static const struct takt_watchdog watchdog = {NULL, kick};
    const struct takt_line line = {&out, put, flush};
    int rc = 2;
    int32_t status = takt_sink_init(&sink, &line, ring, sizeof ring);
    if (status != TAKT_OK) {
        (void)fprintf(stderr, "takt_sink_init: %d\n", (int)status);
    } else {
        takt_trace_text(&sink, "takt auf posix\n");
        takt_sink_mark(&sink);
        (void)memset(slots, 0xff, sizeof slots);
        status = takt_core_init_journaled(&core, &port_test_ops, &arena, NULL, &sink, TAKT_PROFILE_SHARED, &watchdog,
                                          &nvm, current, stored, PORT_TEST_PERSIST_BOUND);
        if (status != TAKT_OK) {
            (void)fprintf(stderr, "takt_core_init_journaled: %d\n", (int)status);
        } else {
            rc = drive(&core, ticks);
        }
    }
    return rc;
}

int main(int argc, char **argv) {
    uint64_t ticks = 2000u;
    const int flags = fcntl(STDOUT_FILENO, F_GETFL);
    int rc = 2;
    if ((argc > 1) && (ticks_of(argv[1], &ticks) != 0)) {
        (void)fprintf(stderr, "takt: keine Zahl von Ticks: %s\n", argv[1]);
    } else if ((flags < 0) || (fcntl(STDOUT_FILENO, F_SETFL, flags | O_NONBLOCK) != 0)) {
        perror("fcntl");
    } else {
        rc = run(ticks);
    }
    return rc;
}
