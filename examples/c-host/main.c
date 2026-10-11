/* Ein Fuellventil als Takt-Programm in einem C-Projekt (12.11, M11 Schritt 18a).
 *
 * `takt build --emit embed` (in CMake: `takt_add_program`) liefert die
 * Bibliothek `valve` mit `valve.h`, `takt_sdk.h` und der Tabelle `valve_ops`;
 * das SDK-Archiv `takt_sdk` bringt den Kern. Dieses Programm stellt, was der
 * Wirt stellt: die Geraete, die Leitung des Traces und die Schleife. Es
 * faehrt das Ventil in logischer Zeit ueber so viele Ticks, wie das erste
 * Argument sagt (ohne: 1000), und schreibt den Trace auf stdout.
 */
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
    (void)now;
    ((struct tank *)user)->valve = value;
    return 1;
}

/* Der Heartbeat des Geraets `tank` (12.4): Es lebt. */
uint8_t valve_alive_tank(void *user, int64_t now) {
    (void)user;
    (void)now;
    return 1;
}

/* Die Leitung des Traces (12.5): Der Rahmen schreibt ueber sie. */
void takt_board_trace(const char *line) { fputs(line, stdout); }
void takt_board_trace_i64(long long value) { printf("%lld ", value); }
void takt_board_trace_u64(unsigned long long value) { printf("%llu ", value); }
void takt_board_trace_f64(double value) { printf("%.17g ", value); }
void takt_board_trace_hex8(unsigned char value) { printf("0x%02x", value); }

static struct valve_arena arena;
static takt_core core;

int main(int argc, char **argv) {
    long long ticks = argc > 1 ? strtoll(argv[1], NULL, 10) : 1000;
    struct tank tank = {0};
    int32_t status = takt_core_init(&core, &valve_ops, &arena, &tank, TAKT_PROFILE_BAREMETAL);
    if (status != TAKT_OK) {
        fprintf(stderr, "takt_core_init: %d\n", (int)status);
        return 2;
    }
    /* Logische Zeit (12.11): Die Uhr springt zu jeder Frist, die der Kern
     * nennt, und jeder Job rechnet zu Ende, bevor sie weiterspringt. */
    int64_t now = 0;
    int64_t end = (int64_t)ticks * VALVE_TICK_NS;
    for (;;) {
        takt_next next = takt_core_service(&core, now);
        if (next.jobs) {
            takt_core_job_work(&core);
            continue;
        }
        if (next.ended != 0 || next.deadline_ns >= end) {
            break;
        }
        now = next.deadline_ns;
    }
    takt_core_finish(&core);
    return 0;
}
