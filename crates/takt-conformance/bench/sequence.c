/* takt bench (13.8): Sequenzschritt mit Fault-Pfad — die C-Referenz zu
 * `sequence.takt`, als Zustandsautomat, wie 6.2 die Sequenz entzuckert:
 * `S0` faehrt die Stufe an und prueft sie einmal im Eintrittstick, nach
 * einem Tick folgt `S1`, das zaehlt und die Sequenz neu betritt. Eine
 * verletzte Erwartung fuehrt ueber den Fault-Pfad nach `SAFE`, das nach
 * einem Tick zurueckkehrt. Die Reihenfolge je Tick ist die aus 9.3: erst
 * der `loop:` der Maschine, dann die Uebergaenge, beim Wechsel `enter:` und
 * der `loop:` des neuen Zustands im Modus ENTRY. */
#include <stdint.h>

enum state { START, S0, S1, SAFE };

static enum state state = START;
static uint32_t in_state;
static int32_t stage, passed, faults;
static int32_t digest;

/* Der Fault-Pfad (5.2 Regel 5): `SAFE` betreten. */
static void fault(void) {
    state = SAFE;
    in_state = 0;
    faults = (faults + 1) % 65536;
}

/* `TEST` betreten, also `S0`: `enter:`, dann `loop:` im Modus ENTRY mit
 * der einmaligen Pruefung (6.2 `expect`). */
static void enter_s0(void) {
    state = S0;
    in_state = 0;
    stage = (stage + 1) % 7;
    if (stage == 6) {
        fault();
    }
}

static void enter_s1(void) {
    state = S1;
    in_state = 0;
    passed = (passed + 1) % 65536;
}

void takt_bench_reference(void) {
    digest = (passed * 7 + faults) % 65536;
    switch (state) {
    case START:
        enter_s0();
        return;
    case S0:
        /* `after 1 ms: -> S1` bei einem Tick von 1 ms. */
        if (++in_state >= 1) {
            enter_s1();
        }
        return;
    case S1:
        enter_s0();
        return;
    case SAFE:
        if (++in_state >= 1) {
            enter_s0();
        }
        return;
    }
}

unsigned long long takt_bench_reference_digest(void) {
    return (unsigned long long)digest;
}
