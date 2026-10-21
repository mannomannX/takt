/* takt_sdk.h - Takt als Baustein in C (12.11): der Kern ohne Warten im
 * SDK-Archiv `libtakt_sdk.a` (`takt_sdk.lib` fuer MSVC).
 *
 * Eine Programmbibliothek aus `takt build --emit embed` bringt ihre
 * Einstiege als Tabelle `P_ops`. Der Wirt stellt Arena, Treiberobjekt und
 * den Speicher des Kerns, baut ihn mit `takt_core_init` (mit `persist`:
 * `takt_core_init_journaled` mit Geraet und Puffern) und ruft dann
 * `takt_core_service` zu jeder Frist, die der Kern nennt; wartet ein Job,
 * ruft ein Kontext niedrigerer Prioritaet `takt_core_job_work` und danach
 * der Schrittkontext `takt_core_service` erneut (12.11).
 *
 * Den Trace schreibt der Rahmen ueber die Haken `takt_trace_*` (12.5), jeden
 * Aufruf mit der Senke, die sein Kern bei `takt_core_init` bekam; die Haken
 * stellt das Archiv. Eine Senke (`takt_sink`) ist ein Ring vor einer Leitung
 * des Wirts, gebunden mit `takt_sink_init`. In ihr stehen je Tick die
 * Zeitzeile, die Marke (`takt_sink_mark`) und am Ende die Bilanz
 * (`takt_core_report`), die `takt port-test --judge` liest; davor darf der
 * Wirt einen Kopf schreiben (`takt_trace_text`). Die Haken deklariert der
 * Kopf des Programms (`P.h`), eine Deklaration fuer alle Programme; ein
 * Haken haengt nichts an, Leerzeichen und Zeilenende schreibt, wer ruft.
 *
 * Der Kern wartet nie und nimmt keinen Heap; `takt_core_service` laeuft nie
 * zweimal zugleich. `ops`, Arena, Treiberobjekt und Senke leben so lange wie
 * der Kern. Jeder Kern bekommt eine Senke; zwei Kerne mit zwei Senken
 * schreiben nie ineinander. Teilen Kerne eine Senke, schreiben nie zwei
 * Kontexte zugleich in sie.
 */
#ifndef TAKT_SDK_H
#define TAKT_SDK_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Die Fassung dieser Schnittstelle; eine Tabelle anderer Fassung nimmt
 * `takt_core_init` nicht an. Fassung 2: Der Wirt nennt seinen Watchdog.
 * Fassung 3: Die Tabelle traegt `operator_abort` (`takt_core_abort`).
 * Fassung 4: die Senke je Kern (`takt_sink`), `init` und `init_with` mit ihr,
 * die Haken `takt_trace_*` mit ihr; `takt_line_*` entfaellt. */
#define TAKT_SDK_ABI 4u

/* So viele Byte haelt `takt_core`; das Archiv prueft beim Bau, dass sein
 * Kern hineinpasst. */
#define TAKT_CORE_BYTES 1024u

/* So viele Byte haelt `takt_sink`, ebenso geprueft. */
#define TAKT_SINK_BYTES 128u

/* Die Namen vor Fassung 4 gibt es nicht mehr. Ein Wirt nach altem SDK
 * scheitert hier beim Uebersetzen, statt still eine Funktion ohne
 * Deklaration zu rufen. Statt `takt_line_init`, `_mark`, `_flush` und
 * `_lossless` stehen `takt_sink_init`, `_mark`, `_flush` und `_lossless`,
 * statt `takt_board_trace…` die Haken `takt_trace_text`, `takt_trace_i64`,
 * `takt_trace_u64`, `takt_trace_f64` und `takt_trace_hex8`, jeweils mit der
 * Senke als erstem Argument. */
#if defined(__GNUC__) || defined(__clang__)
#pragma GCC poison takt_line_init takt_line_mark takt_line_flush takt_line_lossless
#pragma GCC poison takt_board_trace takt_board_trace_i64 takt_board_trace_u64 takt_board_trace_f64 takt_board_trace_hex8
#endif

/* Die Einstiege einer Programmbibliothek (`P_ops`); `arena` ist ihre Arena. */
struct takt_ops {
    uint32_t abi;
    uint8_t persists;
    uint8_t overrun_alert;
    int64_t tick_ns;
    uint64_t logic_hash;
    int64_t persist_min_interval_ns;
    uint32_t persist_bound;
    void (*init)(void *arena, void *user, void *sink);
    int32_t (*init_with)(void *arena, void *user, void *sink, const void *bytes, int32_t len);
    void (*tick)(void *arena, int64_t k);
    void (*commit)(void *arena);
    uint8_t (*idle)(void *arena);
    int64_t (*deadline)(void *arena);
    void (*advance)(void *arena, int64_t n);
    int32_t (*tune)(void *arena, int64_t k, uint32_t param, const void *value, int32_t len);
    uint8_t (*woken)(void *arena, int64_t k);
    uint8_t (*wake_sources)(void);
    int32_t (*persist_snapshot)(void *arena, void *out, int32_t cap);
    int32_t (*persist_restore)(void *arena, const void *bytes, int32_t len);
    void (*dump)(void *arena, int32_t all);
    void (*pc)(void *arena, int32_t all);
    int32_t (*next_run)(void *arena, int64_t *delay);
    void (*end)(void *arena);
    void (*overrun)(void *arena);
    void (*hardware)(void *arena);
    void (*operator_abort)(void *arena);
    void (*tolerance)(int64_t *ns, uint32_t *runs);
    uint8_t (*output_timing)(void);
    int32_t (*job_dispatch)(void *arena);
    void (*job_work)(void *arena);
};

/* Das Geraet des Journals (5.9, 12.3): zwei Slots gleicher Groesse;
 * ein Vorgang beginnt und wird abgefragt, `read` darf vor dem ersten Tick
 * blockieren. `ctx` geht an jeden Rueckruf. */
struct takt_nvm {
    void *ctx;
    uint32_t slot_size;
    int64_t blocking_ns; /* so lange haelt ein Vorgang den Kern an; < 0: in der Hardware */
    uint8_t (*begin_erase)(void *ctx, uint8_t slot);
    uint8_t (*begin_write)(void *ctx, uint8_t slot, uint32_t offset, const uint8_t *bytes, uint32_t len);
    int32_t (*poll)(void *ctx); /* TAKT_NVM_* */
    uint8_t (*read)(void *ctx, uint8_t slot, uint32_t offset, uint8_t *into, uint32_t len);
};

/* Was `poll` des Geraets meldet. */
#define TAKT_NVM_IDLE 0
#define TAKT_NVM_BUSY 1
#define TAKT_NVM_DONE 2
#define TAKT_NVM_FAILED 3

/* Der Speicher des Kerns, im Wirt gestellt. */
typedef struct takt_core {
    _Alignas(8) unsigned char opaque[TAKT_CORE_BYTES];
} takt_core;

/* Der Speicher einer Senke, im Wirt gestellt (`takt_sink_init`). */
typedef struct takt_sink {
    _Alignas(8) unsigned char opaque[TAKT_SINK_BYTES];
} takt_sink;

/* Das Laufzeitprofil (12.8). */
#define TAKT_PROFILE_BAREMETAL 0
#define TAKT_PROFILE_SHARED 1
#define TAKT_PROFILE_LINUX_RT 2

/* Rueckgaben von `takt_core_init` und `takt_core_init_journaled`. */
#define TAKT_OK 0
#define TAKT_ERR_NULL (-1)
#define TAKT_ERR_ABI (-2)
#define TAKT_ERR_MEMORY (-3) /* mit `persist` der Einstieg mit Journal, ohne der andere (5.9) */
#define TAKT_ERR_PROFILE (-4)
#define TAKT_ERR_BOUND (-5) /* ein Puffer ist zu klein: `P_PERSIST_BOUND`, ein leerer Ring */
#define TAKT_ERR_LINE (-6)  /* keine Senke (`takt_sink_init`) */

/* Das Ende eines Laufs (12.7) in `takt_next.ended`; 0, solange er geht. */
#define TAKT_NEXT_NOW 1
#define TAKT_NEXT_AFTER 2
#define TAKT_NEXT_ON_WAKE 3
#define TAKT_NEXT_ON_START 4

/* Was `takt_core_service` dem Wirt sagt. */
typedef struct takt_next {
    int64_t deadline_ns;       /* wann der Kern wieder gerufen werden will */
    int64_t next_run_delay_ns; /* bei TAKT_NEXT_AFTER die Dauer */
    int32_t ended;             /* 0 oder TAKT_NEXT_* */
    uint8_t jobs;              /* ein Job wartet: `takt_core_job_work` */
} takt_next;

/* Der Watchdog des Wirts (12.3, 12.11): Der Kern meldet ihm nach jedem
 * Schritt und an jeder Tickgrenze im Schlaf, dass er lebt. Ein Wirt ohne
 * Watchdog gibt ausdruecklich einen `kick`, der nichts tut. */
struct takt_watchdog {
    void *ctx;
    void (*kick)(void *ctx);
};

/* Baut den Kern eines Programms ohne `persist`: `ops` seine Einstiege
 * (`P_ops`), `arena` seine Arena, `user` das Treiberobjekt, `sink` die
 * Senke seines Trace aus `takt_sink_init`, ohne sie TAKT_ERR_LINE: Die Senke
 * stellt der Wirt ausdruecklich (12.11). `profile` ist TAKT_PROFILE_*,
 * `watchdog` der Watchdog des Wirts. Danach steht das Programm in Tick 0
 * (9.4). */
int32_t takt_core_init(takt_core *core, const struct takt_ops *ops, void *arena, void *user, takt_sink *sink,
                       int32_t profile, const struct takt_watchdog *watchdog);

/* Wie `takt_core_init`, fuer ein Programm mit `persist`: Es beginnt ueber
 * sein Journal auf `nvm`, mit zwei Puffern von je `bound` Byte
 * (`P_PERSIST_BOUND`), die nur der Kern benutzt (5.9). */
int32_t takt_core_init_journaled(takt_core *core, const struct takt_ops *ops, void *arena, void *user,
                                 takt_sink *sink, int32_t profile, const struct takt_watchdog *watchdog,
                                 const struct takt_nvm *nvm, uint8_t *current, uint8_t *stored, uint32_t bound);

/* Rechnet alle Tickgrenzen bis `now_ns` (12.1). */
takt_next takt_core_service(takt_core *core, int64_t now_ns);

/* Die Tunables des Wirts (8.4): `next` nennt je Aufruf eine Aenderung der
 * Grenze `k` in Aufzeichnungsreihenfolge (Parameter, Wert, Laenge; der Wert
 * gilt bis zum naechsten Aufruf) und liefert 1, am Ende 0. `idle` sagt, ob
 * die Quelle ruht, und darf NULL sein. */
struct takt_tunables {
    void *ctx;
    int32_t (*next)(void *ctx, uint64_t k, uint32_t *param, const uint8_t **value, uint32_t *len);
    uint8_t (*idle)(void *ctx);
};

/* Wie `takt_core_service`, mit den Tunables des Wirts: Ihre Aenderungen gehen
 * vor dem Schritt ihrer Grenze in das Programm, und im Schlaf weckt eine
 * Aenderung (9.9). `tunables` darf NULL sein. */
takt_next takt_core_service_with(takt_core *core, int64_t now_ns, const struct takt_tunables *tunables);

/* Ein Weckereignis des Wirts (9.9): Schlaeft der Kern, endet der Schlaf an
 * der naechsten Grenze, die `takt_core_service` erreicht. Aus jedem Kontext,
 * auch aus einer ISR oder einem Signal und waehrend eines Schritts. */
void takt_core_wake(takt_core *core);

/* Ein Operator-Abort (5.4): Der Kern meldet ihn vor dem Schritt der
 * naechsten Grenze, die `takt_core_service` erreicht, allen Maschinen;
 * schlaeft er, endet der Schlaf dort (9.9). Aus jedem Kontext, auch aus einer
 * ISR oder waehrend eines Schritts. */
void takt_core_abort(takt_core *core);

/* Eine Schutzverletzung (12.3): Die Schutzeinheit hat einen Zugriff der TCB
 * auf den Programmzustand abgewiesen. Der Kern meldet `Runtime(Hardware)`
 * vor dem Schritt der naechsten Grenze, die `takt_core_service` erreicht,
 * allen Maschinen; schlaeft er, endet der Schlaf dort (9.9). Aus jedem
 * Kontext, auch aus einem Fault-Handler oder einer ISR und waehrend eines
 * Schritts. */
void takt_core_hardware_fault(takt_core *core);

/* Rechnet die verteilten Jobs (4.5), unterbrechbar durch `takt_core_service`. */
void takt_core_job_work(takt_core *core);

/* Beendet den Lauf an einer Grenze des Wirts (12.7): das Journal synchron,
 * und ein Schlaf ueber das Ende gibt die Zeile des Ticks davor frei (9.9). */
void takt_core_finish(takt_core *core);

/* Die Leitung des Wirts (12.2, 12.5): `try_write` nimmt ein Byte, wenn Platz
 * ist, und wartet nie (0: kein Platz); `flush` schickt ab, was sie
 * gesammelt hat, und darf NULL sein. `ctx` geht an jeden Rueckruf. */
struct takt_line {
    void *ctx;
    uint8_t (*try_write)(void *ctx, uint8_t byte);
    void (*flush)(void *ctx);
};

/* Bindet die Leitung `line` an die Senke `sink`; `ring` haelt `len` Byte
 * zwischen Schritt und Leitung. Ist er voll und nimmt die Leitung nichts,
 * wird verworfen und gezaehlt (12.2). Vor dem ersten Schritt eines Kerns,
 * der sie bekommt. */
int32_t takt_sink_init(takt_sink *sink, const struct takt_line *line, uint8_t *ring, uint32_t len);

/* Verlustfrei (13.8): Ist der Ring voll, wartet der Schreibende, bis die
 * Leitung ihn geleert hat, statt zu verwerfen. Nur fuer Laeufe in logischer
 * Zeit; im Betrieb verbietet 12.2 das Warten. Nach `takt_sink_init`, vor
 * dem ersten Schritt; TAKT_ERR_LINE ohne Senke. */
int32_t takt_sink_lossless(takt_sink *sink);

/* Gibt an die Leitung, was der Ring haelt und sie ohne Warten nimmt, fuer
 * Zeilen des Wirts ausserhalb des Laufs. Kein Schritt laeuft. */
void takt_sink_flush(takt_sink *sink);

/* Die Marke `takt trace`, ab der die Bilanz die gesendeten Byte zaehlt;
 * davor darf ein Kopf stehen (`takt_trace_text`). Kein Schritt laeuft. */
void takt_sink_mark(takt_sink *sink);


/* Die Stacks eines Laufs (12.3), wie der Port sie misst: die Tiefe unter
 * Last und die Schranke des Programms (`P_TICK_STACK_BYTES`,
 * `P_JOB_STACK_BYTES`); 0 heisst nicht gemessen. */
struct takt_stacks {
    uint32_t tick;
    uint32_t tick_bound;
    uint32_t tick_program;
    uint32_t job;
    uint32_t job_bound;
    uint8_t job_shared; /* die Jobs rechnen auf dem Schritt-Stack */
};

/* Die fremde Last der Portpruefung (12.11, 13.8): Der Wirt ruft
 * `takt_foreign_round` in seiner fremden Aufgabe, so oft er kann. Rechnet
 * eine Runde falsch, hat der Port die FPU-Register nicht gesichert. */
typedef struct takt_foreign {
    uint64_t rounds;
    uint64_t errors;
} takt_foreign;

void takt_foreign_round(takt_foreign *foreign);

/* Beendet den Lauf wie `takt_core_finish` und schreibt die Bilanz (13.8) in
 * die Senke des Kerns, davor die Zeile der fremden Last, wenn `foreign` nicht
 * NULL ist, danach `takt end`; leert den Ring. Kein Schritt laeuft; `stacks`
 * darf NULL sein. */
void takt_core_report(takt_core *core, const struct takt_stacks *stacks, const takt_foreign *foreign);

#ifdef __cplusplus
}
#endif

#endif /* TAKT_SDK_H */
