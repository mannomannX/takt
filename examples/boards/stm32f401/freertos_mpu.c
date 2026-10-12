/* freertos_mpu.c - die Partition der Arena unter FreeRTOS-MPU (12.3,
 * 12.11; M11 Schritt 20b): der Handler von MemManage und die
 * Leerlaufaufgabe, deren Speicher unter FreeRTOS-MPU der Wirt stellt
 * (freertos_mpu.h). */
#include <stddef.h>
#include <stdint.h>

#include "FreeRTOS.h"
#include "board.h"
#include "freertos_mpu.h"
#include "task.h"

void vApplicationGetIdleTaskMemory(StaticTask_t **tcb, StackType_t **stack, configSTACK_DEPTH_TYPE *words);
void MemManage_Handler(void) __attribute__((naked));

/* SCB des Cortex-M4: der Grund einer Verletzung der MPU und ihre Adresse. */
#define SCB_CFSR 0xE000ED28u
#define SCB_MMFAR 0xE000ED34u
#define SCB_MMFSR 0x000000FFu
#define SCB_MMFSR_REFUSED 0x00000082u /* DACCVIOL mit gueltiger Adresse (MMARVALID) */

/* Die Arena als Adressbereich und der Kern; `mpu_arena` setzt sie, bevor
 * eine Aufgabe ohne Privileg laeuft, danach liest sie nur der Handler. */
static uintptr_t mpu_first;
static size_t mpu_bytes;
static takt_core *mpu_core;

/* Atomar: Die MPU wies einen Zugriff auf die Arena ab; der Kern laeuft und
 * kann ihn melden. */
static int mpu_seen;
static int mpu_running;

static volatile uint32_t *mpu_reg(uintptr_t address) { return (volatile uint32_t *)address; }

/* Das erste Halbwort eines Befehls an `address`. */
static uint16_t mpu_halfword(uintptr_t address) { return *(const volatile uint16_t *)address; }

void mpu_arena(takt_core *core, const void *arena, size_t bytes) {
    mpu_first = (uintptr_t)arena;
    mpu_bytes = bytes;
    mpu_core = core;
}

void mpu_core_running(void) { __atomic_store_n(&mpu_running, 1, __ATOMIC_SEQ_CST); }

int mpu_refused(void) { return __atomic_load_n(&mpu_seen, __ATOMIC_SEQ_CST); }

/* Weist die MPU einen Datenzugriff auf die Arena ab, haelt der Handler ihn
 * fest, meldet ihn dem Kern, wenn der laeuft, und uebergeht den Befehl: Der
 * Zugriff findet nicht statt. Jede andere Verletzung ist ein Fehler der TCB
 * oder ein Stack an seiner Grenze, und das Board haelt an (12.3). `frame`
 * ist der gestapelte Rahmen (r0 bis r3, r12, lr, pc, xpsr). */
__attribute__((used)) static void mpu_refuse(uint32_t *frame) {
    const uint32_t mmfsr = *mpu_reg(SCB_CFSR) & SCB_MMFSR;
    const uintptr_t address = (uintptr_t)*mpu_reg(SCB_MMFAR);
    if ((mmfsr == SCB_MMFSR_REFUSED) && (address >= mpu_first) && ((address - mpu_first) < mpu_bytes)) {
        __atomic_store_n(&mpu_seen, 1, __ATOMIC_SEQ_CST);
        if (__atomic_load_n(&mpu_running, __ATOMIC_SEQ_CST) != 0) {
            takt_core_hardware_fault(mpu_core);
        }
        /* Ein Befehl von Thumb-2 belegt vier Byte, wenn die oberen fuenf Bit
         * seines ersten Halbworts 0b11101, 0b11110 oder 0b11111 sind. */
        const uint32_t pc = frame[6];
        frame[6] = pc + ((((uint32_t)mpu_halfword((uintptr_t)pc) >> 11) >= 0x1Du) ? 4u : 2u);
        *mpu_reg(SCB_CFSR) = mmfsr;
    } else {
        board_halt();
    }
}

/* Der Einstieg holt den Zeiger auf den gestapelten Rahmen, vom Stack, der
 * beim Eintritt aktiv war, und springt in den Handler; dessen Rueckkehr ist
 * die aus der Ausnahme, denn `lr` traegt noch EXC_RETURN. */
void MemManage_Handler(void) {
    __asm__ volatile("tst lr, #4\n\t"
                     "ite eq\n\t"
                     "mrseq r0, msp\n\t"
                     "mrsne r0, psp\n\t"
                     "b mpu_refuse\n\t");
}

void vApplicationGetIdleTaskMemory(StaticTask_t **tcb, StackType_t **stack, configSTACK_DEPTH_TYPE *words) {
    static StaticTask_t idle_tcb;
    static StackType_t idle_stack[configMINIMAL_STACK_SIZE];
    *tcb = &idle_tcb;
    *stack = idle_stack;
    *words = configMINIMAL_STACK_SIZE;
}
