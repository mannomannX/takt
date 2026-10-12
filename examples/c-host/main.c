/* Ein Fuellventil als Takt-Programm in einem C-Projekt (12.11, M11 Schritt 18a).
 *
 * `takt build --emit embed` (in CMake: `takt_add_program`) liefert die
 * Bibliothek `valve` mit `valve.h`, `takt_sdk.h` und der Tabelle `valve_ops`;
 * das SDK-Archiv `takt_sdk` bringt den Kern. Dieses Programm stellt, was der
 * Wirt stellt: die Geraete, die Leitung des Traces und die Schleife. Es
 * faehrt das Ventil in logischer Zeit ueber so viele Ticks, wie das erste
 * Argument sagt (ohne: 1000), und schreibt den Trace auf stdout.
 */
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

#include "takt_sdk.h"
#include "valve.h"

/* Der Tank: das Ventil, wie das Programm es zuletzt stellte. */
struct tank {
    uint8_t valve;
};

/* Der Treiber des Ausgangs `valve` (12.6): nimmt den Wert an. */
uint8_t valve_out_tank_valve(void *user, int64_t now, uint8_t value) {
    struct tank *tank = user;
    (void)now;
    tank->valve = value;
    return 1u;
}

/* Der Heartbeat des Geraets `tank` (12.4): Es lebt. */
uint8_t valve_alive_tank(void *user, int64_t now) {
    (void)user;
    (void)now;
    return 1u;
}

/* Die Leitung des Traces (12.5): stdout. Den Trace schreibt der Rahmen ueber
 * das SDK; dieses nimmt jedes Byte, das `fputc` annimmt. */
static uint8_t put(void *ctx, uint8_t byte) {
    FILE *out = ctx;
    return (fputc((int)byte, out) != EOF) ? 1u : 0u;
}

/* Die Zahl der Ticks aus `text` (CERT C ERR34-C); -1, wenn sie keine ist
 * oder die Zeit des Laufs nicht in 64 Bit passt. */
static int64_t ticks_of(const char *text) {
    char *end = NULL;
    int64_t ticks = -1;
    errno = 0;
    const long long value = strtoll(text, &end, 10);
    if ((errno == 0) && (end != text) && (*end == '\0') && (value >= 0) && (value <= (INT64_MAX / VALVE_TICK_NS))) {
        ticks = (int64_t)value;
    }
    return ticks;
}

/* Faehrt den Kern `core` in logischer Zeit (12.11) bis `end`: Die Uhr
 * springt zu jeder Frist, die der Kern nennt, und jeder Job rechnet zu Ende,
 * bevor sie weiterspringt. Danach die Bilanz. */
static void drive(takt_core *core, int64_t end) {
    int64_t now = 0;
    for (;;) {
        const takt_next next = takt_core_service(core, now);
        if (next.jobs != 0u) {
            takt_core_job_work(core);
        } else if ((next.ended != 0) || (next.deadline_ns >= end)) {
            break;
        } else {
            now = next.deadline_ns;
        }
    }
    takt_core_report(core, NULL, NULL);
}

int main(int argc, char **argv) {
    const int64_t ticks = (argc > 1) ? ticks_of(argv[1]) : 1000;
    int rc = 2;
    if (ticks < 0) {
        (void)fprintf(stderr, "takt: keine Zahl von Ticks: %s\n", argv[1]);
    } else {
        static uint8_t ring[4096];
        static struct valve_arena arena;
        static takt_core core;
        struct tank tank = {0u};
        const struct takt_line line = {stdout, put, NULL};
        int32_t status = takt_line_init(&line, ring, sizeof ring);
        if (status == TAKT_OK) {
            status = takt_core_init(&core, &valve_ops, &arena, &tank, TAKT_PROFILE_BAREMETAL);
        }
        if (status != TAKT_OK) {
            (void)fprintf(stderr, "takt: Leitung oder Kern: %d\n", (int)status);
        } else {
            drive(&core, ticks * VALVE_TICK_NS);
            rc = 0;
        }
    }
    return rc;
}
