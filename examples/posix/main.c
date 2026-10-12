/* Der POSIX-Port mit dem Pruefprogramm der Portpruefung (12.11, 13.8; M11
 * Schritt 18c).
 *
 * `takt port-test --emit embed --form rtos --hardware HW --out DIR` liefert
 * `port_test` (Bibliothek, `port_test.h`, `takt_sdk.h`), auf einem Wirt mit
 * einer Nachricht je Job, die laenger als ein Tick rechnet. Das SDK-Archiv
 * bringt den Kern, `ports/posix` den Port: einen Schrittfaden und einen
 * Jobfaden. Dieses Programm stellt, was der Wirt stellt: die Leitung, das
 * Geraet des Journals und beide Stacks. Waehrend des Laufs rechnet es die
 * fremde Last. Es faehrt so viele Ticks, wie das erste Argument sagt (ohne:
 * 2000), und schreibt den Lauf auf stdout; `takt port-test --judge` urteilt.
 *
 *     clang -std=c11 -O2 -pthread -I DIR -I ports/posix main.c \
 *         ports/posix/takt_posix.c DIR/libport_test.a libtakt_sdk.a -lm
 */
#define _POSIX_C_SOURCE 200809L

#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "port_test.h"
#include "takt_posix.h"
#include "takt_sdk.h"

/* Die Leitung (12.2): sammelt in einem Puffer, den `flush` ohne zu warten
 * nach stdout schreibt; was die Pipe nicht nimmt, bleibt fuer das naechste
 * Mal. */
struct out {
    uint8_t bytes[4096];
    size_t len;
};

static uint8_t put(void *ctx, uint8_t byte) {
    struct out *o = ctx;
    if (o->len == sizeof o->bytes) {
        return 0;
    }
    o->bytes[o->len] = byte;
    o->len++;
    return 1;
}

static void flush(void *ctx) {
    struct out *o = ctx;
    size_t done = 0;
    while (done < o->len) {
        ssize_t n = write(STDOUT_FILENO, &o->bytes[done], o->len - done);
        if (n <= 0) {
            break;
        }
        done += (size_t)n;
    }
    memmove(o->bytes, &o->bytes[done], o->len - done);
    o->len -= done;
}

/* Das Geraet des Journals (5.9): ein Flash im Speicher, zwei Slots, jeder
 * Vorgang sofort fertig. */
#define SLOT 4096u
static uint8_t slots[2][SLOT];
static int32_t pending = TAKT_NVM_IDLE;

static uint8_t erase(void *ctx, uint8_t slot) {
    (void)ctx;
    memset(slots[slot & 1u], 0xff, SLOT);
    pending = TAKT_NVM_DONE;
    return 1;
}

static uint8_t program(void *ctx, uint8_t slot, uint32_t offset, const uint8_t *bytes, uint32_t len) {
    (void)ctx;
    if (offset > SLOT || len > SLOT - offset) {
        pending = TAKT_NVM_FAILED;
        return 1;
    }
    memcpy(&slots[slot & 1u][offset], bytes, len);
    pending = TAKT_NVM_DONE;
    return 1;
}

static int32_t poll_nvm(void *ctx) {
    (void)ctx;
    int32_t state = pending;
    pending = TAKT_NVM_IDLE;
    return state;
}

static uint8_t read_nvm(void *ctx, uint8_t slot, uint32_t offset, uint8_t *into, uint32_t len) {
    (void)ctx;
    if (offset > SLOT || len > SLOT - offset) {
        return 0;
    }
    memcpy(into, &slots[slot & 1u][offset], len);
    return 1;
}

/* Die Stacks beider Faeden; reichlich ueber PTHREAD_STACK_MIN, gemessen wird
 * die Tiefe (12.3). */
#define STACK_BYTES (256u * 1024u)
static _Alignas(64) uint8_t tick_stack[STACK_BYTES];
static _Alignas(64) uint8_t job_stack[STACK_BYTES];

static struct out out;
static uint8_t ring[1u << 16];
static const struct takt_nvm nvm = {NULL, SLOT, -1, erase, program, poll_nvm, read_nvm};
static uint8_t current[PORT_TEST_PERSIST_BOUND], stored[PORT_TEST_PERSIST_BOUND];
static struct port_test_arena arena;
static takt_core core;
static struct takt_posix posix;

int main(int argc, char **argv) {
    const uint64_t ticks = argc > 1 ? strtoull(argv[1], NULL, 10) : 2000u;
    const int flags = fcntl(STDOUT_FILENO, F_GETFL);
    if (flags < 0 || fcntl(STDOUT_FILENO, F_SETFL, flags | O_NONBLOCK) != 0) {
        perror("fcntl");
        return 2;
    }
    const struct takt_line line = {&out, put, flush};
    int32_t status = takt_line_init(&line, ring, sizeof ring);
    if (status != TAKT_OK) {
        fprintf(stderr, "takt_line_init: %d\n", (int)status);
        return 2;
    }
    takt_board_trace("takt auf posix\n");
    takt_line_mark();
    memset(slots, 0xff, sizeof slots);
    status = takt_core_init_journaled(&core, &port_test_ops, &arena, NULL, TAKT_PROFILE_SHARED, &nvm, current, stored,
                                      PORT_TEST_PERSIST_BOUND);
    if (status != TAKT_OK) {
        fprintf(stderr, "takt_core_init_journaled: %d\n", (int)status);
        return 2;
    }
    const struct takt_posix_config config = {
        .ticks = ticks,
        .tick_ns = PORT_TEST_TICK_NS,
        .tick_stack = tick_stack,
        .tick_stack_bytes = sizeof tick_stack,
        .job_stack = job_stack,
        .job_stack_bytes = sizeof job_stack,
        .step_priority = 0,
        .job_priority = 0,
    };
    int rc = takt_posix_start(&posix, &core, &config);
    if (rc != 0) {
        fprintf(stderr, "takt_posix_start: %d\n", rc);
        return 2;
    }
    /* Die fremde Aufgabe: rechnet mit der FPU, solange Takt laeuft. */
    takt_foreign foreign = {0, 0};
    while (takt_posix_running(&posix)) {
        takt_foreign_round(&foreign);
    }
    struct takt_stacks stacks;
    rc = takt_posix_join(&posix, &stacks);
    if (rc != 0) {
        fprintf(stderr, "takt_posix_join: %d\n", rc);
        return 2;
    }
    stacks.tick_bound = PORT_TEST_TICK_STACK_BYTES;
    stacks.job_bound = PORT_TEST_JOB_STACK_BYTES;
    takt_core_report(&core, &stacks, &foreign);
    return 0;
}
