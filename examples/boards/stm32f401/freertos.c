/* freertos.c - der Port mit Faeden unter FreeRTOS auf der Black Pill (12.11,
 * 13.8; M11 Schritt 20), mit oder ohne FreeRTOS-MPU (Schritt 20b).
 *
 * Ein Programm aus `takt build --emit embed --form rtos --prefix app`
 * (Bibliothek, `app.h`, `takt_sdk.h`), das SDK-Archiv fuer
 * `thumbv7em-none-eabihf`, der Port mit Faeden `ports/rtos` mit seinem
 * Adapter `ports/freertos`, der Kern von FreeRTOS V11.3.1 mit dem Port
 * `ARM_CM4F` oder `ARM_CM4_MPU` (`FreeRTOSConfig.h`) und die
 * Board-Unterstuetzung dieses Verzeichnisses. Den Schritt rechnet eine
 * Aufgabe hoechster Prioritaet, die Jobs eine darunter; beide stellt der
 * Port. Darunter zwei Aufgaben des Wirts: Die Aufgabe des Wirts schreibt den
 * Kopf, startet Kern und Port und schreibt am Ende die Bilanz; die fremde
 * Aufgabe rechnet die fremde Last (`takt_foreign_round`) und fuehrt die LED,
 * solange Takt laeuft.
 *
 * **Unter FreeRTOS-MPU** (`TAKT_FREERTOS_MPU`) hat die fremde Aufgabe kein
 * Privileg: Sie sieht ihren Stack (Region 3), ihren Austauschblock mit dem
 * Wirt (Region 0), den Flash und die Peripherie, die Arena nicht (12.11,
 * `Schutz: Partition`). Bevor der Kern beginnt, schreibt sie einmal in die
 * Arena; die MPU weist den Zugriff ab, der Handler von MemManage uebergeht
 * den Befehl, und der Kopf nennt das Ergebnis (`takt schutz partition
 * verweigert`). Mit `TAKT_PROBE_NS` schreibt sie so lange nach dem Start der
 * Last noch einmal hinein: Der Handler meldet die Verletzung dem Kern
 * (`takt_core_hardware_fault`), und im naechsten Tick bekommen alle
 * Maschinen `Runtime(Hardware)` (12.3). Den Waechter unter dem Stack der
 * Interrupts haelt dort ein Watchpoint der DWT, denn die Regionen der MPU
 * gehoeren FreeRTOS (`stm32f401-mpu.ld`).
 *
 * Der Bau setzt `TAKT_TICKS` (so viele Ticks, 0: bis das Programm endet),
 * `TAKT_LOGICAL` (der Port mit Faeden faehrt in Echtzeit: 0),
 * `TAKT_FRESH_JOURNAL`, `TAKT_TICK_STACK_PROGRAM` und `TAKT_FREERTOS_MPU`,
 * dem Linker `board_stack_bytes`, den Stack der Interrupts. So baut ihn
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
#if TAKT_FREERTOS_MPU
#include "freertos_mpu.h"
#endif

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
#if defined(TAKT_PROBE_NS) && !TAKT_FREERTOS_MPU
#error "TAKT_PROBE_NS: nur unter FreeRTOS-MPU weist eine Region den Zugriff auf die Arena ab"
#endif

/* Den Haken ruft FreeRTOS, wenn ein Stack seine Grenze erreicht
 * (`configCHECK_FOR_STACK_OVERFLOW`). */
void vApplicationStackOverflowHook(TaskHandle_t task, char *name);

/* Die halbe Periode der LED, die die fremde Aufgabe fuehrt. */
#define BLINK_NS 500000000LL

/* Der Ring zwischen Schritt und Leitung (12.2). */
#define RING_BYTES 2048u

/* Die Stacks in Worten von FreeRTOS, aufgerundet. */
#define WORDS(bytes) (((bytes) + sizeof(StackType_t) - 1u) / sizeof(StackType_t))

/* Der Stack der Aufgabe des Wirts: Kopf, Start, Bilanz und Ende. */
#define HOST_STACK_WORDS 512u

/* Der Stack der fremden Aufgabe: die fremde Last. Eine Zweierpotenz an ihrer
 * Grenze, denn unter FreeRTOS-MPU ist er ihre Region 3. */
#define FOREIGN_STACK_BYTES 2048u

/* Die Prioritaet der beiden Aufgaben des Wirts: ueber der Leerlaufaufgabe,
 * unter den Faeden des Ports, untereinander gleich. Wartet der Wirt nicht,
 * teilen sie sich den Kern Tick um Tick. */
#define HOST_PRIORITY 1u

/* So viele Ticks des Systems schlaeft der Wirt, bevor er wieder fragt, ob
 * der Port noch laeuft: 10 ms. */
#define HOST_POLL_TICKS 10u

/* Der Austauschblock zwischen dem Wirt und der fremden Aufgabe, so gross wie
 * ihre Region 0 und an ihr ausgerichtet: Kein anderes Objekt teilt die
 * Region. */
#define EXCHANGE_BYTES 32u

struct exchange {
    takt_foreign foreign;     /* die fremde Last; der Wirt liest sie erst nach ihrem Ende */
    TaskHandle_t host;        /* wen die fremde Aufgabe benachrichtigt */
    volatile uint8_t *target; /* unter FreeRTOS-MPU das Ziel der Probe: die Arena */
    int running;              /* atomar: Die fremde Last laeuft, solange Takt laeuft. */
    uint8_t fill[EXCHANGE_BYTES - sizeof(takt_foreign) - sizeof(TaskHandle_t) - sizeof(uint8_t *) - sizeof(int)];
};

_Static_assert(sizeof(struct exchange) == EXCHANGE_BYTES, "der Austauschblock fuellt seine Region");

static _Alignas(EXCHANGE_BYTES) struct exchange exchange;

/* Der Watchdog des Wirts (12.11): Ein Konformitaetslauf wartet auf die
 * Leitung und ist kein Betrieb (12.3); der Kern meldet, und die Meldung geht
 * ausdruecklich nirgendwohin. */
static void kick(void *ctx) { (void)ctx; }

#if TAKT_FREERTOS_MPU
/* Ob der Watchpoint unter dem Stack der Interrupts wirkt (`board_watch_stack`);
 * `main` setzt es vor FreeRTOS, der Kopf nennt es. */
static int guard_state = BOARD_WATCH_NO_COMPARATOR;

/* Ein Byte in die Arena, das die MPU einer Aufgabe ohne Privileg abweisen
 * muss (12.11, freertos_mpu.h). */
static void probe(const struct exchange *x) { *x->target = 0xA5u; }

/* Die Kopfzeile des Waechters: Wirkt er nicht, sagt sie warum, statt ihn
 * still zu verlieren (12.3). */
static const char *guard_line(int state) {
    const char *line = "takt waechter fehlt: kein Komparator der DWT\r\n";
    if (state == BOARD_WATCH_ARMED) {
        line = "takt waechter watchpoint\r\n";
    } else if (state == BOARD_WATCH_DEBUGGER) {
        line = "takt waechter fehlt: ein Debugger haelt an (DHCSR.C_DEBUGEN)\r\n";
    } else if (state == BOARD_WATCH_NO_MONITOR) {
        line = "takt waechter fehlt: der Debug-Monitor laesst sich nicht einschalten\r\n";
    } else {
        /* kein Komparator */
    }
    return line;
}
#endif

/* Der Kopf des Laufs, dann die Marke, ab der die Bilanz zaehlt. */
static void banner(void) {
    takt_board_trace("\r\ntakt auf stm32f401 in C unter FreeRTOS " tskKERNEL_VERSION_NUMBER "\r\n  Kerntakt ");
    takt_board_trace_u64(BOARD_CORE_HZ);
    takt_board_trace("Hz, Tick ");
    takt_board_trace_i64(APP_TICK_NS);
    takt_board_trace("ns\r\n  Form rtos\r\n" TAKT_RTOS_PROTECTION "\r\n\r\n");
#if TAKT_FREERTOS_MPU
    takt_board_trace(guard_line(guard_state));
    takt_board_trace((mpu_refused() != 0) ? "takt schutz partition verweigert\r\n" : "takt schutz partition offen\r\n");
#endif
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

/* Die fremde Aufgabe: unter FreeRTOS-MPU zuerst die Probe, dann bereit; die
 * fremde Last, sobald der Wirt sie startet, bis er sie anhaelt. Danach
 * wartet sie fuer immer. Sie kennt nur ihren Austauschblock. */
static void foreign(void *arg) {
    struct exchange *const x = &exchange;
    int64_t toggle = BLINK_NS;
    int lit = 0;
#ifdef TAKT_PROBE_NS
    int probed = 0;
#endif
    (void)arg;
#if TAKT_FREERTOS_MPU
    probe(x);
#endif
    (void)xTaskNotifyGive(x->host);
    (void)ulTaskNotifyTake(pdTRUE, portMAX_DELAY);
    const TickType_t start = xTaskGetTickCount();
    while (__atomic_load_n(&x->running, __ATOMIC_SEQ_CST) != 0) {
        takt_foreign_round(&x->foreign);
        const TickType_t passed = xTaskGetTickCount() - start;
        const int64_t passed_ns = (int64_t)passed * (1000000000LL / (int64_t)configTICK_RATE_HZ);
        if (passed_ns >= toggle) {
            lit = (lit == 0) ? 1 : 0;
            board_led(lit);
            toggle += BLINK_NS;
        }
#ifdef TAKT_PROBE_NS
        if ((probed == 0) && (passed_ns >= (int64_t)TAKT_PROBE_NS)) {
            probe(x);
            probed = 1;
        }
#endif
    }
    (void)xTaskNotifyGive(x->host);
    for (;;) {
        (void)ulTaskNotifyTake(pdTRUE, portMAX_DELAY);
    }
}

/* Legt die fremde Aufgabe an: unter FreeRTOS-MPU ohne Privileg, mit ihrem
 * Stack und dem Austauschblock als einzigen Regionen im RAM. */
static TaskHandle_t spawn_foreign(void) {
    static _Alignas(FOREIGN_STACK_BYTES) StackType_t stack[FOREIGN_STACK_BYTES / sizeof(StackType_t)];
    static StaticTask_t tcb;
    TaskHandle_t task = NULL;
#if TAKT_FREERTOS_MPU
    TaskParameters_t params = {
        foreign,
        "fremd",
        (configSTACK_DEPTH_TYPE)(FOREIGN_STACK_BYTES / sizeof(StackType_t)),
        NULL,
        HOST_PRIORITY,
        stack,
        {{&exchange, EXCHANGE_BYTES,
          portMPU_REGION_READ_WRITE | portMPU_REGION_EXECUTE_NEVER | portMPU_REGION_CACHEABLE_BUFFERABLE},
         {NULL, 0u, 0u},
         {NULL, 0u, 0u}},
        &tcb,
    };
    if (xTaskCreateRestrictedStatic(&params, &task) != pdPASS) {
        task = NULL;
    }
#else
    task = xTaskCreateStatic(foreign, "fremd", (configSTACK_DEPTH_TYPE)(FOREIGN_STACK_BYTES / sizeof(StackType_t)),
                             NULL, HOST_PRIORITY, stack, &tcb);
#endif
    return task;
}

/* Faehrt den Kern `core` auf `arena` ueber den Port, waehrend die fremde
 * Aufgabe `worker` ihre Last rechnet, misst danach beide Stacks und schreibt
 * die Bilanz. */
static void drive(TaskHandle_t worker, takt_core *core, struct app_arena *arena) {
    static struct takt_rtos port;
    static StackType_t tick_stack[WORDS(APP_TICK_STACK_BYTES)];
    static StackType_t job_stack[WORDS(APP_JOB_STACK_BYTES)];
    const struct takt_rtos_config config = {TAKT_TICKS,         APP_TICK_NS,      tick_stack,
                                            sizeof tick_stack,  job_stack,        sizeof job_stack,
                                            TAKT_OS_STEP_PRIORITY, TAKT_OS_JOB_PRIORITY};
    struct takt_stacks stacks = {0u, 0u, 0u, 0u, 0u, 0u};
    const int32_t status = start_core(core, arena);
    if (status != TAKT_OK) {
        takt_board_trace("takt: der Kern nimmt nicht an: ");
        takt_board_trace_i64(status);
        takt_board_trace("\r\n");
    } else if (takt_rtos_start(&port, core, &config) != 0) {
        takt_board_trace("takt: der Port startet nicht\r\n");
    } else {
#if TAKT_FREERTOS_MPU
        mpu_core_running();
#endif
        __atomic_store_n(&exchange.running, 1, __ATOMIC_SEQ_CST);
        (void)xTaskNotifyGive(worker);
        /* `takt_rtos_join` erst, wenn der Port nicht mehr laeuft
         * (takt_rtos.h); bis dahin rechnet die fremde Aufgabe. */
        while (takt_rtos_running(&port) != 0) {
            vTaskDelay(HOST_POLL_TICKS);
        }
        if (takt_rtos_join(&port, &stacks) != 0) {
            takt_board_trace("takt: die Faeden des Ports enden nicht sauber\r\n");
        }
        __atomic_store_n(&exchange.running, 0, __ATOMIC_SEQ_CST);
        (void)ulTaskNotifyTake(pdTRUE, portMAX_DELAY);
        stacks.tick_bound = APP_TICK_STACK_BYTES;
        stacks.tick_program = TAKT_TICK_STACK_PROGRAM;
        stacks.job_bound = APP_JOB_STACK_BYTES;
        takt_core_report(core, &stacks, &exchange.foreign);
    }
}

/* Die Aufgabe des Wirts: legt die fremde Aufgabe an, wartet, bis sie bereit
 * ist, schreibt den Kopf und faehrt den Lauf. Sie haelt den Kern und seine
 * Arena. */
static void host(void *arg) {
    static takt_core core;
    static struct app_arena arena;
    (void)arg;
    exchange.host = xTaskGetCurrentTaskHandle();
#if TAKT_FREERTOS_MPU
    exchange.target = (volatile uint8_t *)&arena;
    mpu_arena(&core, &arena, sizeof arena);
#endif
    const TaskHandle_t worker = spawn_foreign();
    if (worker == NULL) {
        takt_board_trace("takt: die fremde Aufgabe entsteht nicht\r\n");
    } else {
        (void)ulTaskNotifyTake(pdTRUE, portMAX_DELAY);
        banner();
        drive(worker, &core, &arena);
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
    static const struct takt_line line = {NULL, board_line_put, board_line_flush};
    static StackType_t host_stack[HOST_STACK_WORDS];
    static StaticTask_t host_tcb;
    int32_t status = TAKT_ERR_LINE;
    if (board_init() == 0) {
        static uint8_t ring[RING_BYTES];
        status = takt_line_init(&line, ring, RING_BYTES);
    }
    if (status == TAKT_OK) {
        /* Der Waechter unter dem Stack der Interrupts (12.3): unter
         * FreeRTOS-MPU ein Watchpoint, denn die Regionen gehoeren FreeRTOS,
         * sonst eine Region des Wirts. */
#if TAKT_FREERTOS_MPU
        guard_state = board_watch_stack();
#else
        board_protect_stack();
#endif
        if (xTaskCreateStatic(host, "wirt", HOST_STACK_WORDS, NULL, HOST_PRIORITY | portPRIVILEGE_BIT, host_stack,
                              &host_tcb) == NULL) {
            takt_board_trace("takt: die Aufgabe des Wirts entsteht nicht\r\n");
        } else {
            /* Kehrt nur zurueck, wenn FreeRTOS nicht startet. */
            vTaskStartScheduler();
            takt_board_trace("takt: FreeRTOS startet nicht\r\n");
        }
        takt_line_flush();
    }
    board_park();
    return 0;
}
