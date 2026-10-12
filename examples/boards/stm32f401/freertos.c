/* freertos.c - der Port mit Faeden unter FreeRTOS auf der Black Pill (12.11,
 * 13.8; M11 Schritt 20).
 *
 * Ein Programm aus `takt build --emit embed --form rtos --prefix app`
 * (Bibliothek, `app.h`, `takt_sdk.h`), das SDK-Archiv fuer
 * `thumbv7em-none-eabihf`, der Port mit Faeden `ports/rtos` mit seinem
 * Adapter `ports/freertos`, der Kern von FreeRTOS V11.3.1 mit dem Port
 * `ARM_CM4F` (`FreeRTOSConfig.h`) und die Board-Unterstuetzung dieses
 * Verzeichnisses. Den Schritt rechnet eine Aufgabe hoechster Prioritaet, die
 * Jobs eine darunter; beide stellt der Port. Die Aufgabe des Wirts liegt
 * unter ihnen: Sie startet den Port, rechnet die fremde Last
 * (`takt_foreign_round`), fuehrt die LED und schreibt am Ende die Bilanz.
 *
 * Der Bau setzt `TAKT_TICKS` (so viele Ticks, 0: bis das Programm endet),
 * `TAKT_LOGICAL` (der Port mit Faeden faehrt in Echtzeit: 0),
 * `TAKT_FRESH_JOURNAL` und `TAKT_TICK_STACK_PROGRAM`, dem Linker
 * `board_stack_bytes`, den Stack der Interrupts. So baut ihn
 * `takt_conformance::board::c_host`.
 *
 * Ein Lauf dieses Wirts ist ein Konformitaetslauf: Er wartet auf die
 * Leitung, hat darum keinen Watchdog (12.3) und fuehrt `next_run` nicht aus,
 * sondern schreibt die Bilanz.
 */
#include <stddef.h>
#include <stdint.h>

#include "FreeRTOS.h"
#include "app.h"
#include "board.h"
#include "takt_rtos.h"
#include "takt_sdk.h"
#include "task.h"

#ifndef TAKT_TICKS
#error "TAKT_TICKS: so viele Ticks faehrt der Lauf (0: bis das Programm endet)"
#endif
#ifndef TAKT_LOGICAL
#error "TAKT_LOGICAL: 0, denn der Port mit Faeden faehrt in Echtzeit"
#endif
#if TAKT_LOGICAL
#error "der Port mit Faeden faehrt in Echtzeit; logische Zeit hat nur die Interruptform (irq.c)"
#endif
#ifndef TAKT_FRESH_JOURNAL
#error "TAKT_FRESH_JOURNAL: 1, wenn das Journal vor dem Lauf leer sein soll"
#endif
#ifndef TAKT_TICK_STACK_PROGRAM
#error "TAKT_TICK_STACK_PROGRAM: der Anteil des Programms am Schritt-Stack (Manifest)"
#endif

/* Den Haken ruft FreeRTOS, wenn ein Stack seine Grenze erreicht
 * (`configCHECK_FOR_STACK_OVERFLOW`). */
void vApplicationStackOverflowHook(TaskHandle_t task, char *name);

/* Die halbe Periode der LED, die die Aufgabe des Wirts fuehrt. */
#define BLINK_NS 500000000LL

/* Der Ring zwischen Schritt und Leitung (12.2). */
#define RING_BYTES 2048u

/* Die Stacks in Worten von FreeRTOS, aufgerundet. */
#define WORDS(bytes) (((bytes) + sizeof(StackType_t) - 1u) / sizeof(StackType_t))

/* Der Stack der Aufgabe des Wirts: die fremde Last, die Bilanz und das Ende. */
#define HOST_STACK_WORDS 512u

/* Die Prioritaet der Aufgabe des Wirts: ueber der Leerlaufaufgabe, unter den
 * Faeden des Ports. */
#define HOST_PRIORITY 1u

/* Der Watchdog des Wirts (12.11): Ein Konformitaetslauf wartet auf die
 * Leitung und ist kein Betrieb (12.3); der Kern meldet, und die Meldung geht
 * ausdruecklich nirgendwohin. */
static void kick(void *ctx) { (void)ctx; }

/* Der Kern des Programms; der Kontext des Wirts haelt ihn fuer seine Aufgabe. */
static takt_core board_core;

/* Der Kopf des Laufs, dann die Marke, ab der die Bilanz zaehlt. */
static void banner(void) {
    takt_board_trace("\r\ntakt auf stm32f401 in C unter FreeRTOS " tskKERNEL_VERSION_NUMBER "\r\n  Kerntakt ");
    takt_board_trace_u64(BOARD_CORE_HZ);
    takt_board_trace("Hz, Tick ");
    takt_board_trace_i64(APP_TICK_NS);
    takt_board_trace("ns\r\n  Form rtos\r\n\r\n");
    takt_line_mark();
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
        takt_board_trace("journal: loeschen scheiterte\r\n");
    }
#endif
    status = takt_core_init_journaled(core, &app_ops, arena, NULL, TAKT_PROFILE_SHARED, &watchdog, &nvm, current,
                                      stored, APP_PERSIST_BOUND);
#else
    status = takt_core_init(core, &app_ops, arena, NULL, TAKT_PROFILE_SHARED, &watchdog);
#endif
    return status;
}

/* Die Aufgabe des Wirts: startet den Port, rechnet die fremde Last, solange
 * er laeuft, misst danach beide Stacks und schreibt die Bilanz. */
static void host(void *arg) {
    static struct takt_rtos port;
    static StackType_t tick_stack[WORDS(APP_TICK_STACK_BYTES)];
    static StackType_t job_stack[WORDS(APP_JOB_STACK_BYTES)];
    const struct takt_rtos_config config = {TAKT_TICKS,         APP_TICK_NS,      tick_stack,
                                            sizeof tick_stack,  job_stack,        sizeof job_stack,
                                            TAKT_OS_STEP_PRIORITY, TAKT_OS_JOB_PRIORITY};
    takt_foreign foreign = {0u, 0u};
    struct takt_stacks stacks = {0u, 0u, 0u, 0u, 0u, 0u};
    (void)arg;
    const TickType_t start = xTaskGetTickCount();
    if (takt_rtos_start(&port, &board_core, &config) != 0) {
        takt_board_trace("takt: der Port startet nicht\r\n");
    } else {
        int64_t toggle = BLINK_NS;
        int lit = 0;
        while (takt_rtos_running(&port) != 0) {
            takt_foreign_round(&foreign);
            const TickType_t passed = xTaskGetTickCount() - start;
            if (((int64_t)passed * (1000000000LL / (int64_t)configTICK_RATE_HZ)) >= toggle) {
                lit = (lit == 0) ? 1 : 0;
                board_led(lit);
                toggle += BLINK_NS;
            }
        }
        if (takt_rtos_join(&port, &stacks) != 0) {
            takt_board_trace("takt: die Faeden des Ports enden nicht sauber\r\n");
        }
        stacks.tick_bound = APP_TICK_STACK_BYTES;
        stacks.tick_program = TAKT_TICK_STACK_PROGRAM;
        stacks.job_bound = APP_JOB_STACK_BYTES;
        takt_core_report(&board_core, &stacks, &foreign);
    }
    takt_line_flush();
    board_park();
}

/* Ein Stack einer Aufgabe reichte in seinen Waechter (12.3): ein Fehler der
 * TCB, das Board haelt an. */
void vApplicationStackOverflowHook(TaskHandle_t task, char *name) {
    (void)task;
    (void)name;
    board_halt();
}

int main(void) {
    static struct app_arena arena;
    static const struct takt_line line = {NULL, board_line_put, board_line_flush};
    static StackType_t host_stack[HOST_STACK_WORDS];
    static StaticTask_t host_tcb;
    int32_t status = TAKT_ERR_LINE;
    if (board_init() == 0) {
        static uint8_t ring[RING_BYTES];
        status = takt_line_init(&line, ring, RING_BYTES);
    }
    if (status == TAKT_OK) {
        banner();
        status = start_core(&board_core, &arena);
        if (status != TAKT_OK) {
            takt_board_trace("takt: der Kern nimmt nicht an: ");
            takt_board_trace_i64(status);
            takt_board_trace("\r\n");
        } else if (xTaskCreateStatic(host, "wirt", HOST_STACK_WORDS, NULL, HOST_PRIORITY, host_stack, &host_tcb) ==
                   NULL) {
            takt_board_trace("takt: die Aufgabe des Wirts entsteht nicht\r\n");
        } else {
            /* Der Stack der Interrupts hat seinen Waechter; die der Aufgaben
             * prueft FreeRTOS. Kehrt nur zurueck, wenn FreeRTOS nicht startet. */
            board_protect_stack();
            vTaskStartScheduler();
            takt_board_trace("takt: FreeRTOS startet nicht\r\n");
        }
        takt_line_flush();
    }
    board_park();
    return 0;
}
