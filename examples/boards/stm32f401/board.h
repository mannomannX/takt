/* board.h - die WeAct Black Pill (STM32F401CCU6, 25-MHz-Quarz) fuer einen
 * C-Wirt (12.11, M11 Schritt 18b): Takt, die Leitung mit dem Rueckweg zum
 * Host, das Journal im Flash, die LED und der Stack. Was ein Register
 * anfasst, steht in `board.c`, der Start in `startup.c`; ein Wirt je Form
 * (`irq.c`) bindet es an den Kern des SDK und an seinen Port.
 *
 * Die Zahlen sind die des Bring-ups in Rust (`takt-board-stm32f401`): 84 MHz
 * aus dem Quarz, USART1 mit 921600 Baud an PA9/PA10, das Journal in Sektor
 * 2 und 3, die LED an PC13 (aktiv low). Der Host verlangt das Board mit
 * `TAKT` auf der Leitung zurueck (FB-275): Der Wirt merkt sich den Wunsch im
 * Backup-Register 4 und setzt den Chip zurueck, und das Sprungbrett in
 * Sektor 1 gibt es an den DFU-Bootloader des ROM.
 */
#ifndef BOARD_H
#define BOARD_H

#include <stdint.h>

/* Das obere Ende des Stacks, aus dem Linker-Skript `stm32f401.ld`. */
extern uint32_t board_stack_top;

/* Der Takt des Kerns nach `board_init`. */
#define BOARD_CORE_HZ 84000000u

/* Der Takt der Zeitgeber an APB1: zweimal 42 MHz. */
#define BOARD_TIMER_HZ 84000000u

/* Die Prioritaet des Schritts im NVIC: unter der Leitung (`USART1`, 0x10),
 * die im Konformitaetslauf den vollen Ring leert, waehrend der Schritt auf
 * sie wartet; ueber dem Job-Interrupt und der Hauptschleife. */
#define BOARD_STEP_PRIORITY 0x20u

/* Ein Slot des Journals ist ein Sektor des Flash. */
#define BOARD_NVM_SLOT 16384u

/* Takt und Leitung; 0, oder -1, wenn eines nicht kam (die LED leuchtet). */
int board_init(void);

/* Die Leitung (`struct takt_line`): ein Byte, wenn im FIFO Platz ist, ohne
 * zu warten; `flush` weckt den Sende-Interrupt und fuehrt den Wunsch des
 * Hosts aus, das Board zurueckzugeben. */
uint8_t board_line_put(void *ctx, uint8_t byte);
void board_line_flush(void *ctx);

/* Das Journal (`struct takt_nvm`, 5.9): Slot 0 ist Sektor 2, Slot 1 Sektor
 * 3. Jeder Vorgang haelt den Kern an, bis er fertig ist (12.3); `poll` meldet
 * seinen Ausgang. */
uint8_t board_nvm_erase(void *ctx, uint8_t slot);
uint8_t board_nvm_write(void *ctx, uint8_t slot, uint32_t offset, const uint8_t *bytes, uint32_t len);
int32_t board_nvm_poll(void *ctx);
uint8_t board_nvm_read(void *ctx, uint8_t slot, uint32_t offset, uint8_t *into, uint32_t len);

/* Loescht beide Slots: Ein Konformitaetslauf beginnt wie der Interpreter
 * ohne Speicher (13.8). 0 oder -1. */
int board_nvm_wipe(void);

/* Die LED an PC13. */
void board_led(int on);

/* Der Waechter unter dem Stack (12.3): eine Region der MPU ohne Zugriff. */
void board_protect_stack(void);

/* Der Rahmen jeder ISR dieses Boards (12.3): `enter` am Anfang, `leave` mit
 * seiner Rueckgabe am Ende, etwa `takt_irq_chip_isr_enter` und `_leave`. So
 * ist der Programmbereich der Arena fuer die ISR nur lesbar, auch wenn sie
 * den Schritt unterbricht. Ohne Rahmen laeuft sie, wie sie vorfindet. */
void board_isr_frame(uint32_t (*enter)(void), void (*leave)(uint32_t before));

/* Derselbe Waechter ohne Region, wenn die MPU einem RTOS gehoert (12.11):
 * ein Watchpoint der DWT auf jedes Lesen und Schreiben darin. Ein Treffer
 * loest DebugMonitor aus, und das Board haelt an. Der Monitor wirkt nur,
 * solange kein Debugger das Halten eingeschaltet hat (DHCSR.C_DEBUGEN):
 * Sonst hielte ein Treffer den Kern an, statt den Handler zu rufen. Was
 * gilt, sagt der Wert, und der Wirt nennt es im Kopf (12.3). */
#define BOARD_WATCH_ARMED 0
#define BOARD_WATCH_NO_COMPARATOR (-1)
#define BOARD_WATCH_NO_MONITOR (-2)
#define BOARD_WATCH_DEBUGGER (-3)
int board_watch_stack(void);

/* Bemalt den Stack vom Waechter bis kurz unter den Stackzeiger (12.3). */
void board_paint_stack(void);

/* Wie tief der Stack seit `board_paint_stack` reichte, in Byte vom oberen
 * Ende. */
uint32_t board_stack_depth(void);

/* Nach dem Lauf: Das Board schlaeft zwischen den Interrupts, die Leitung
 * sendet aus, und `TAKT` gibt es an den Host zurueck. Kehrt nicht zurueck. */
void board_park(void);

/* Haelt an, nach einer Panik des SDK oder einer Ausnahme, und bleibt fuer
 * `TAKT` erreichbar. Kehrt nicht zurueck. */
void board_halt(void);

/* Nach einer Ausnahme: nennt auf der Leitung Grund und Adresse (CFSR, HFSR,
 * DFSR, MMFAR, BFAR), dann `board_halt`. Kehrt nicht zurueck. */
void board_fault(void);

#endif /* BOARD_H */
