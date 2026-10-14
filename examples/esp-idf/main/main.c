/* main.c - Takt unter ESP-IDF auf dem C6 (12.11, 13.8; M11 Schritt 21a).
 *
 * `takt_add_program(program …)` liefert das Programm (Vorgabe: das Pruefprogramm
 * der Portpruefung; `-DTAKT_PROGRAM=…` nennt ein anderes), die Komponente
 * `takt` den Port der Interruptform mit dem Adapter fuer ESP-IDF und das
 * Geraet des Journals, das SDK-Archiv fuer `riscv32imac` den Kern. Diese
 * Anwendung stellt, was der Wirt stellt (12.11): die Leitung, den Watchdog,
 * das Journal in der Partition `takt_journal`, den Stack der Jobs und die
 * Arena.
 *
 * **Die Form ist `interrupt`** (plan/m11.md 1.7, 2.5): Der Schritt rechnet in
 * der IRAM-ISR des `gptimer` auf dem Stack der Interrupts von ESP-IDF
 * (`CONFIG_FREERTOS_ISR_STACKSIZE`), auch waehrend ESP-IDF auf den Flash
 * zugreift; die Jobs rechnen in einer Aufgabe darueber, und `app_main`
 * rechnet darunter die fremde Last. Den Stack der Interrupts bemalt und
 * misst der Wirt (12.3); die Grenze jedes Stacks bewacht der Hardware-Waechter
 * des C6 (`CONFIG_ESP_SYSTEM_HW_STACK_GUARD`).
 *
 * Ein Lauf ist ein Konformitaetslauf: Er hat keinen Watchdog (12.3) und
 * fuehrt `next_run` nicht aus, sondern schreibt die Bilanz. Danach gibt der
 * Wirt die Leitung weiter ab, bis der Host das Board zurueckverlangt
 * (`TAKT`); dann setzt sich der Chip zurueck (line.c).
 *
 * Der Bau setzt `TAKT_TICKS` (so viele Ticks, 0: bis das Programm endet) und
 * `TAKT_FRESH_JOURNAL`; so baut ihn `takt_conformance::esp_idf`.
 */
#include <stddef.h>
#include <stdint.h>

#include "program.h"
#include "esp_attr.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "line.h"
#include "sdkconfig.h"
#include "takt_idf_nvm.h"
#include "takt_irq.h"
#include "takt_irq_chip.h"
#include "takt_irq_esp_idf.h"
#include "takt_sdk.h"

#ifndef TAKT_TICKS
#error "TAKT_TICKS: so viele Ticks faehrt der Lauf (0: bis das Programm endet)"
#endif
#ifndef TAKT_FRESH_JOURNAL
#error "TAKT_FRESH_JOURNAL: 1, wenn das Journal vor dem Lauf leer sein soll"
#endif

/* Der Schritt rechnet auf dem Stack der Interrupts von ESP-IDF. */
_Static_assert((uint32_t)CONFIG_FREERTOS_ISR_STACKSIZE >= PROGRAM_TICK_STACK_BYTES,
               "CONFIG_FREERTOS_ISR_STACKSIZE ist kleiner als der Schritt-Stack des Programms (12.3)");

/* Der Takt des `gptimer`: ein Schritt des Zaehlers sind 25 ns. */
#define TIMER_HZ 40000000U

/* Die Stufe des Schritts, eine der Stufen in C (1 bis 3); die Leitung hat
 * keinen Interrupt, sie rechnet im Schritt (line.c). */
#define STEP_PRIORITY 2U

/* Die Prioritaeten der Aufgaben ueber `app_main` (1), der fremden Last: die
 * Jobs, darunter das Geraet des Journals. */
#define JOB_PRIORITY 20U
#define NVM_PRIORITY 19U

/* Der Stack der Aufgabe des Journals: `esp_partition_write` und der Treiber
 * des Flash. */
#define NVM_STACK_BYTES 4096U

/* Der Ring zwischen Schritt und Leitung (12.2). */
#define RING_BYTES 4096U

/* Das Muster der Bemalung und was sie unter dem oberen Ende des Stacks der
 * Interrupts freilaesst: Dort koennen Interrupts ueber der Sperre rechnen. */
#define PAINT 0xC5U
#define PAINT_MARGIN 1024U

/* Das Journal schreibt einen Eintrag in zwei Teilen: den Kopf von 32 Byte
 * und die Nutzlast (`takt-rt-core`, `journal.rs`). */
#define JOURNAL_HEAD 32U
#define JOURNAL_BYTES ((PROGRAM_PERSIST_BOUND > JOURNAL_HEAD) ? PROGRAM_PERSIST_BOUND : JOURNAL_HEAD)

/* Der Stack der Interrupts von ESP-IDF (`portable/riscv/port.c`): auf dem
 * einkernigen C6 einer, an 16 Byte ausgerichtet, sein oberes Ende das Ende
 * des Felds, wenn die Groesse ein Vielfaches von 16 ist. `StackType_t` ist
 * dort ein Byte. */
#define ISR_STACK_BYTES ((uint32_t)configISR_STACK_SIZE)
_Static_assert((configISR_STACK_SIZE % 16) == 0, "das obere Ende des Stacks der Interrupts ist das Ende des Felds");
_Static_assert(sizeof(StackType_t) == 1U, "der Stack der Interrupts zaehlt in Byte");
extern StackType_t xIsrStack[portNUM_PROCESSORS][configISR_STACK_SIZE];

/* Die Arena unter dem Namen, unter dem `takt check-image` und der Blick ueber
 * JTAG sie finden (`arena_symbol` im Manifest); darum mit externer Bindung. */
extern struct program_arena program_arena;
struct program_arena program_arena;

/* Der Stack der Jobs unter dem Namen, unter dem `takt check-image` ihn
 * prueft (`job_stack_symbol`): so gross, wie das Programm verlangt, und an 32
 * Byte ausgerichtet. Ein Programm ohne Jobs hat keinen. */
#if PROGRAM_JOB_STACK_BYTES > 0u
extern StackType_t program_job_stack[PROGRAM_JOB_STACK_BYTES / sizeof(StackType_t)];
_Alignas(32) StackType_t program_job_stack[PROGRAM_JOB_STACK_BYTES / sizeof(StackType_t)];
#endif

/* ESP-IDF ruft sie aus seiner Hauptaufgabe. */
void app_main(void);

/* Der Watchdog des Wirts (12.11): Ein Konformitaetslauf ist kein Betrieb
 * (12.3); der Kern meldet, und die Meldung geht ausdruecklich nirgendwohin.
 * Der Schritt ruft ihn in seiner ISR. */
static IRAM_ATTR void kick(void *ctx) { (void)ctx; }

/* Der Kopf des Laufs, dann die Marke, ab der die Bilanz zaehlt. */
static void banner(void) {
    takt_board_trace("\r\ntakt auf esp32c6 in C unter ESP-IDF\r\n  Kerntakt ");
    takt_board_trace_u64((uint64_t)CONFIG_ESP_DEFAULT_CPU_FREQ_MHZ * 1000000U);
    takt_board_trace("Hz, Tick ");
    takt_board_trace_i64(PROGRAM_TICK_NS);
    takt_board_trace("ns\r\n  Form interrupt\r\n\r\n");
    takt_line_mark();
}

/* Bemalt den Stack der Interrupts von unten bis kurz unter sein oberes Ende;
 * aus `app_main`, mit gesperrten Interrupts: Keine ISR rechnet dabei auf ihm. */
static void paint_isr_stack(void) {
    const UBaseType_t mask = portSET_INTERRUPT_MASK_FROM_ISR();
    for (uint32_t i = 0U; i < (ISR_STACK_BYTES - PAINT_MARGIN); i++) {
        xIsrStack[0][i] = PAINT;
    }
    portCLEAR_INTERRUPT_MASK_FROM_ISR(mask);
}

/* Wie tief der Stack der Interrupts seit der Bemalung reichte, in Byte vom
 * oberen Ende. */
static uint32_t isr_stack_depth(void) {
    uint32_t i = 0U;
    while ((i < ISR_STACK_BYTES) && (xIsrStack[0][i] == PAINT)) {
        i++;
    }
    return ISR_STACK_BYTES - i;
}

/* Der Kern des Programms auf seiner Arena; mit `persist` ueber das Journal
 * in seiner Partition, das ein Konformitaetslauf vorher loescht (5.9, 13.8). */
static int32_t start_core(takt_core *core) {
    static const struct takt_watchdog watchdog = {NULL, kick};
    int32_t status = TAKT_ERR_LINE;
#if PROGRAM_PERSIST_BOUND > 0u
    static struct takt_idf_nvm journal;
    static struct takt_nvm nvm;
    static uint8_t buffer[JOURNAL_BYTES];
    static StackType_t stack[NVM_STACK_BYTES / sizeof(StackType_t)];
    static uint8_t current[PROGRAM_PERSIST_BOUND];
    static uint8_t stored[PROGRAM_PERSIST_BOUND];
    if (takt_idf_nvm_open(&journal, "takt_journal", PROGRAM_NVM_BLOCKING_NS, buffer, sizeof buffer, stack, sizeof stack,
                          NVM_PRIORITY, &nvm) != 0) {
        takt_board_trace("journal: die Partition `takt_journal` fehlt\r\n");
    } else if ((TAKT_FRESH_JOURNAL != 0) && (takt_idf_nvm_clear(&journal) != 0)) {
        takt_board_trace("journal: loeschen scheiterte\r\n");
    } else {
        status = takt_core_init_journaled(core, &program_ops, &program_arena, NULL, TAKT_PROFILE_SHARED, &watchdog, &nvm,
                                          current, stored, PROGRAM_PERSIST_BOUND);
    }
#else
    status = takt_core_init(core, &program_ops, &program_arena, NULL, TAKT_PROFILE_SHARED, &watchdog);
#endif
    return status;
}

/* Faehrt den Kern ueber den Port, waehrend `app_main` die fremde Last
 * rechnet, und schreibt danach die Bilanz. */
static void run(takt_core *core) {
    static struct takt_irq port;
    const struct takt_irq_config config = {TAKT_TICKS, PROGRAM_TICK_NS, TIMER_HZ, STEP_PRIORITY, 0u};
    takt_foreign foreign = {0u, 0u};
#if PROGRAM_JOB_STACK_BYTES > 0u
    takt_irq_esp_idf_jobs(program_job_stack, sizeof program_job_stack, JOB_PRIORITY);
#else
    takt_irq_esp_idf_jobs(NULL, 0u, JOB_PRIORITY);
#endif
    paint_isr_stack();
    if (takt_irq_start(&port, core, &config) != 0) {
        takt_board_trace("takt: der Port startet nicht\r\n");
    } else {
        while (takt_irq_running(&port) != 0) {
            takt_foreign_round(&foreign);
        }
        struct takt_stacks stacks = {isr_stack_depth(), PROGRAM_TICK_STACK_BYTES, 0u, 0u, PROGRAM_JOB_STACK_BYTES, 0u};
        if (takt_irq_esp_idf_job_depth(&stacks.job) != 0) {
            stacks.job = 0u;
        }
        takt_core_report(core, &stacks, &foreign);
    }
}

void app_main(void) {
    static uint8_t ring[RING_BYTES];
    static takt_core core;
    static struct takt_line line;
    line_open(&line);
    if (takt_line_init(&line, ring, RING_BYTES) == TAKT_OK) {
        banner();
        const int32_t status = start_core(&core);
        if (status != TAKT_OK) {
            takt_board_trace("takt: der Kern nimmt nicht an: ");
            takt_board_trace_i64(status);
            takt_board_trace("\r\n");
        } else {
            run(&core);
        }
    }
    /* Was im Ring blieb, geht hinaus; `flush` liest dabei den Wunsch des
     * Hosts, der den Chip zuruecksetzt. */
    for (;;) {
        takt_line_flush();
        vTaskDelay(1);
    }
}
