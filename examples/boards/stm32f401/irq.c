/* irq.c - der Port der Interruptform in C auf der Black Pill (12.11, 13.8;
 * M11 Schritt 18b).
 *
 * Ein Programm aus `takt build --emit embed --form interrupt --prefix app`
 * (Bibliothek, `app.h`, `takt_sdk.h`), das SDK-Archiv fuer
 * `thumbv7em-none-eabihf`, der Port `ports/irq` mit seinem Adapter
 * `ports/irq/stm32f4` und die Board-Unterstuetzung dieses Verzeichnisses.
 * Dieses Programm stellt, was der Wirt stellt: die Leitung, das Journal im
 * Flash und den Stack, auf dem Schritt und Jobs rechnen. Den Schritt rechnet
 * die ISR von TIM2, die Jobs PendSV; die Hauptschleife darunter ist fremde
 * Arbeit: Sie rechnet die Last der Portpruefung (`takt_foreign_round`) und
 * fuehrt die LED. Am Ende schreibt sie die Bilanz.
 *
 * Der Bau setzt `TAKT_TICKS` (so viele Ticks, 0: bis das Programm endet),
 * `TAKT_LOGICAL` (1: in logischer Zeit, 13.8), `TAKT_FRESH_JOURNAL` (1: das
 * Journal vorher loeschen) und `TAKT_TICK_STACK_PROGRAM` (der Anteil des
 * Programms am Schritt-Stack, aus dem Manifest), dem Linker
 * `board_stack_bytes`. So baut ihn `takt_conformance::board::c_host`:
 *
 *     clang --target=thumbv7em-none-eabihf -mcpu=cortex-m4 -mfpu=fpv4-sp-d16 -mfloat-abi=hard
 *         -ffreestanding -nostdlib -fuse-ld=lld -I EMBED -I ports/irq ... irq.c board.c startup.c
 *         ports/irq/takt_irq.c ports/irq/stm32f4/takt_irq_chip.c EMBED/libapp.a libtakt_sdk.a
 *         -Wl,-T,stm32f401.ld -Wl,--defsym=board_stack_bytes=N
 *
 * Ein Lauf dieses Wirts ist ein Konformitaetslauf: Er wartet auf die
 * Leitung, hat darum keinen Watchdog (12.3) und fuehrt `next_run` nicht aus,
 * sondern schreibt die Bilanz.
 *
 * Die MPU gehoert Takt (12.3, 12.11; FB-755): Nennt die Lieferform eine
 * Schutzregion (`APP_PROTECT_ALIGN`), liegt die Arena an ihr ausgerichtet,
 * der Adapter setzt die Region, und der Kern oeffnet sie nur, waehrend er
 * den Programmbereich schreibt (`Schutz: Fenster`). Die ISR der Leitung
 * laeuft im Rahmen des Adapters. Mit `TAKT_PROBE_NS` schreibt die
 * Hauptschleife so lange nach dem Start einmal in die Arena: Die MPU weist
 * den Zugriff ab, und im naechsten Tick bekommen alle Maschinen
 * `Runtime(Hardware)`.
 */
#include <stddef.h>
#include <stdint.h>

#include "app.h"
#include "board.h"
#include "takt_irq.h"
#include "takt_irq_chip.h"
#include "takt_sdk.h"

#ifndef TAKT_TICKS
#error "TAKT_TICKS: so viele Ticks faehrt der Lauf (0: bis das Programm endet)"
#endif
#ifndef TAKT_LOGICAL
#error "TAKT_LOGICAL: 1 in logischer Zeit (13.8), 0 in Echtzeit"
#endif
#ifndef TAKT_FRESH_JOURNAL
#error "TAKT_FRESH_JOURNAL: 1, wenn das Journal vor dem Lauf leer sein soll"
#endif
#ifndef TAKT_TICK_STACK_PROGRAM
#error "TAKT_TICK_STACK_PROGRAM: der Anteil des Programms am Schritt-Stack (Manifest)"
#endif
#if defined(TAKT_PROBE_NS) && !defined(APP_PROTECT_ALIGN)
#error "TAKT_PROBE_NS: nur eine Region der MPU weist den Zugriff auf die Arena ab (`protect` in der Konfiguration)"
#endif

/* Die halbe Periode der LED, die die Hauptschleife fuehrt. */
#define BLINK_NS 500000000LL

/* Der Ring zwischen Schritt und Leitung (12.2): Er faengt die Zeilen eines
 * Ticks, die die Leitung erst danach abnimmt. */
#define RING_BYTES 2048u

/* Die Senke des Programms (12.5, 12.11): ein Ring vor der Leitung des
 * Wirts, gebunden mit `takt_sink_init`; in ihr stehen Kopf, Trace und
 * Bilanz. */
static takt_sink sink;

/* Der Watchdog des Wirts (12.11): Ein Konformitaetslauf wartet auf die
 * Leitung und ist kein Betrieb (12.3); der Kern meldet, und die Meldung geht
 * ausdruecklich nirgendwohin. */
static void kick(void *ctx) { (void)ctx; }

/* Die Haken des Adapters fuer den Kern; gefuellt, wenn er die Region setzt. */
static struct takt_protection protection;

/* Die Region ueber dem Programmbereich der Arena (12.3): 1, wenn der Adapter
 * sie gesetzt hat. Vor dem Kern: Sein `init` schreibt schon den Trace von
 * Tick 0, und die Zeile `Schutz` steht davor. */
static int guard_arena(struct app_arena *arena) {
    int armed = 0;
    board_protect_stack();
#ifdef APP_PROTECT_ALIGN
    if (takt_irq_chip_protect((const uint8_t *)arena, APP_PROTECT_ALIGN, APP_PROTECT_BYTES, &protection) == 0) {
        board_isr_frame(takt_irq_chip_isr_enter, takt_irq_chip_isr_leave);
        armed = 1;
    }
#else
    (void)arena;
#endif
    return armed;
}

/* Der Kopf des Laufs, dann die Marke, ab der die Bilanz zaehlt. Die Form
 * steht darin, damit `takt bench` die Stack-Reserve zuordnet (FB-513);
 * `Fenster`, wenn die Region steht, sonst `keiner`. */
static void banner(int armed) {
    takt_trace_text(&sink, "\r\ntakt auf stm32f401 in C\r\n  Kerntakt ");
    takt_trace_u64(&sink, BOARD_CORE_HZ);
    takt_trace_text(&sink, " Hz, Tick ");
    takt_trace_i64(&sink, APP_TICK_NS);
    takt_trace_text(&sink, " ns\r\n  Form interrupt\r\n");
    takt_trace_text(&sink, (armed != 0) ? "  Schutz: Fenster" : "  Schutz: keiner");
    takt_trace_text(&sink, "\r\n\r\n");
    takt_sink_mark(&sink);
}

/* Gibt dem Kern die Haken des Adapters, wenn die Region steht; bis hierher
 * war sie offen, und `init` schrieb in der Phase des Wirts (12.3). 0, wenn
 * der Kern hat, was der Kopf nennt: ohne Region keinen Schutz, mit Region
 * den Schutz. Sonst laeuft nichts: Ein Kopf mit `Fenster` gilt immer. */
static int protect_core(takt_core *core, int armed) {
    int rc = 0;
    if ((armed != 0) && ((takt_core_protect(core, &protection) != TAKT_OK) || (takt_core_protected(core) == 0))) {
        rc = -1;
    }
    return rc;
}

/* Der Kern des Programms auf seiner Arena; mit `persist` ueber das Journal
 * im Flash, das ein Konformitaetslauf vorher loescht (5.9, 13.8). */
static int32_t start_core(takt_core *core, struct app_arena *arena) {
    static const struct takt_watchdog watchdog = {NULL, kick};
    int32_t status;
#if APP_PERSIST_BOUND > 0u
    static uint8_t current[APP_PERSIST_BOUND];
    static uint8_t stored[APP_PERSIST_BOUND];
    static const struct takt_nvm nvm = {NULL,           BOARD_NVM_SLOT,  APP_NVM_BLOCKING_NS, board_nvm_erase,
                                        board_nvm_write, board_nvm_poll, board_nvm_read};
#if TAKT_FRESH_JOURNAL
    if (board_nvm_wipe() != 0) {
        takt_trace_text(&sink, "journal: loeschen scheiterte\r\n");
    }
#endif
    status = takt_core_init_journaled(core, &app_ops, arena, NULL, &sink, TAKT_PROFILE_SHARED, &watchdog, &nvm,
                                      current, stored, APP_PERSIST_BOUND);
#else
    status = takt_core_init(core, &app_ops, arena, NULL, &sink, TAKT_PROFILE_SHARED, &watchdog);
#endif
    return status;
}

#ifdef TAKT_PROBE_NS
/* Ein Zugriff der Hauptschleife auf den Programmbereich, ausserhalb des
 * Kerns: Die MPU weist ihn ab, der Adapter uebergeht ihn, und der Kern
 * meldet `Runtime(Hardware)` (12.3). Ohne Schutz schriebe er ein Byte, das
 * der Board-Test als Abweichung saehe. */
static void probe(struct app_arena *arena) { *(volatile uint8_t *)(void *)arena = 0xA5u; }
#endif

/* Faehrt den Kern ueber den Port, waehrend die Hauptschleife ihre fremde
 * Arbeit tut, und schreibt danach die Bilanz. Schritt, Jobs und
 * Hauptschleife teilen den Stack; gemalt wird er erst hier, Aufbau und
 * Journal zaehlen nicht (12.3). */
static void run(takt_core *core, struct app_arena *arena) {
    static struct takt_irq port;
    const struct takt_irq_config config = {TAKT_TICKS, APP_TICK_NS, BOARD_TIMER_HZ, BOARD_STEP_PRIORITY, TAKT_LOGICAL};
    takt_foreign foreign = {0u, 0u};
    int64_t toggle = BLINK_NS;
    int lit = 0;
#ifdef TAKT_PROBE_NS
    int probed = 0;
#else
    (void)arena;
#endif
    board_paint_stack();
    if (takt_irq_start(&port, core, &config) != 0) {
        takt_trace_text(&sink, "takt: der Port startet nicht\r\n");
    } else {
        while (takt_irq_running(&port) != 0) {
#if TAKT_LOGICAL
            takt_irq_release(&port);
#endif
            takt_foreign_round(&foreign);
            if (takt_irq_chip_now() >= toggle) {
                lit = (lit == 0) ? 1 : 0;
                board_led(lit);
                toggle += BLINK_NS;
            }
#ifdef TAKT_PROBE_NS
            if ((probed == 0) && (takt_irq_chip_now() >= (int64_t)TAKT_PROBE_NS)) {
                probe(arena);
                probed = 1;
            }
#endif
        }
        const struct takt_stacks stacks = {board_stack_depth(), APP_TICK_STACK_BYTES + APP_JOB_STACK_BYTES,
                                           TAKT_TICK_STACK_PROGRAM, 0u, 0u, 1u};
        takt_core_report(core, &stacks, &foreign);
    }
}

int main(void) {
    static uint8_t ring[RING_BYTES];
#ifdef APP_PROTECT_ALIGN
    /* Am Anfang des RAM (`.takt_state`), an der Groesse der Region
     * ausgerichtet, die den Programmbereich deckt. */
    static _Alignas(APP_PROTECT_ALIGN) struct app_arena arena __attribute__((section(".takt_state")));
#else
    static struct app_arena arena;
#endif
    static takt_core core;
    static const struct takt_line line = {NULL, board_line_put, board_line_flush};
    int32_t status = TAKT_ERR_LINE;
    if (board_init() == 0) {
        status = takt_sink_init(&sink, &line, ring, RING_BYTES);
    }
#if TAKT_LOGICAL
    if (status == TAKT_OK) {
        status = takt_sink_lossless(&sink);
    }
#endif
    if (status == TAKT_OK) {
        const int armed = guard_arena(&arena);
        banner(armed);
        status = start_core(&core, &arena);
        if ((status == TAKT_OK) && (protect_core(&core, armed) != 0)) {
            takt_trace_text(&sink, "takt: der Kern nimmt den Schutz nicht an\r\n");
        } else if (status == TAKT_OK) {
            run(&core, &arena);
        } else {
            takt_trace_text(&sink, "takt: der Kern nimmt nicht an: ");
            takt_trace_i64(&sink, status);
            takt_trace_text(&sink, "\r\n");
        }
        /* Was ohne Lauf im Ring blieb, geht jetzt hinaus. */
        takt_sink_flush(&sink);
    }
    board_park();
    return 0;
}
