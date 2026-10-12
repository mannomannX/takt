/* startup.c - Start und Vektortabelle der Black Pill fuer einen C-Wirt
 * (M11 Schritt 18b), mit dem Linker-Skript `stm32f401.ld`.
 *
 * Das Sprungbrett in Sektor 1 startet die Anwendung ueber ihre
 * Vektortabelle: Stackzeiger und Resetvektor. `Reset_Handler` stellt VTOR
 * auf diese Tabelle (nach dem Reset zeigt es auf Sektor 0, den
 * HID-Bootloader), gibt die FPU frei, kopiert `.data`, nullt `.bss` und ruft
 * `main`.
 *
 * Jeder Handler, den ein Port oder der Wirt nicht stellt, haelt das Board an
 * und laesst es fuer `TAKT` erreichbar (`board_halt`). Die Namen sind die von
 * CMSIS: Der Port der Interruptform stellt `TIM2_IRQHandler` und
 * `PendSV_Handler`, die Leitung `USART1_IRQHandler`, ein RTOS `SVC_Handler`,
 * `PendSV_Handler` und `SysTick_Handler`.
 */
#include <stddef.h>
#include <stdint.h>

#include "board.h"

/* Aus dem Linker-Skript. */
extern uint32_t board_data_load[];
extern uint32_t board_data_start[];
extern uint32_t board_data_end[];
extern uint32_t board_bss_start[];
extern uint32_t board_bss_end[];
extern uint8_t board_stack_top[];

int main(void);

void Reset_Handler(void);
void Default_Handler(void);
void NMI_Handler(void) __attribute__((weak, alias("Default_Handler")));
void HardFault_Handler(void) __attribute__((weak, alias("Default_Handler")));
void MemManage_Handler(void) __attribute__((weak, alias("Default_Handler")));
void BusFault_Handler(void) __attribute__((weak, alias("Default_Handler")));
void UsageFault_Handler(void) __attribute__((weak, alias("Default_Handler")));
void SVC_Handler(void) __attribute__((weak, alias("Default_Handler")));
void DebugMon_Handler(void) __attribute__((weak, alias("Default_Handler")));
void PendSV_Handler(void) __attribute__((weak, alias("Default_Handler")));
void SysTick_Handler(void) __attribute__((weak, alias("Default_Handler")));
void TIM2_IRQHandler(void) __attribute__((weak, alias("Default_Handler")));
void USART1_IRQHandler(void) __attribute__((weak, alias("Default_Handler")));

/* VTOR und CPACR des Cortex-M4. */
#define SCB_VTOR 0xE000ED08u
#define SCB_CPACR 0xE000ED88u
#define SCB_CPACR_FPU 0x00F00000u /* CP10 und CP11: volle Rechte */

/* Die Interrupts des STM32F401 (RM0368, Abschnitt 10, die Vektortabelle). */
#define BOARD_IRQS 85u

typedef void (*board_handler)(void);

/* Die Vektortabelle des Cortex-M4: Stackzeiger, Reset, die Ausnahmen 2 bis
 * 15 (0 an den reservierten Stellen), dann die Interrupts des Chips. */
struct board_vectors {
    const void *stack;
    board_handler reset;
    board_handler exceptions[14];
    board_handler irqs[BOARD_IRQS];
};

__attribute__((section(".vectors"), used)) const struct board_vectors board_vector_table = {
    board_stack_top,
    Reset_Handler,
    {NMI_Handler, HardFault_Handler, MemManage_Handler, BusFault_Handler, UsageFault_Handler, NULL, NULL, NULL, NULL,
     SVC_Handler, DebugMon_Handler, NULL, PendSV_Handler, SysTick_Handler},
    {
        /* 0 bis 7: WWDG, PVD, TAMP_STAMP, RTC_WKUP, FLASH, RCC, EXTI0, EXTI1 */
        Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler,
        Default_Handler, Default_Handler,
        /* 8 bis 15: EXTI2, EXTI3, EXTI4, DMA1_Stream0 bis 4 */
        Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler,
        Default_Handler, Default_Handler,
        /* 16 bis 23: DMA1_Stream5, DMA1_Stream6, ADC, vier reserviert, EXTI9_5 */
        Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler,
        Default_Handler, Default_Handler,
        /* 24 bis 31: TIM1_BRK_TIM9, TIM1_UP_TIM10, TIM1_TRG_COM_TIM11, TIM1_CC, TIM2, TIM3, TIM4, I2C1_EV */
        Default_Handler, Default_Handler, Default_Handler, Default_Handler, TIM2_IRQHandler, Default_Handler,
        Default_Handler, Default_Handler,
        /* 32 bis 39: I2C1_ER, I2C2_EV, I2C2_ER, SPI1, SPI2, USART1, USART2, reserviert */
        Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler, USART1_IRQHandler,
        Default_Handler, Default_Handler,
        /* 40 bis 47: EXTI15_10, RTC_Alarm, OTG_FS_WKUP, vier reserviert, DMA1_Stream7 */
        Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler,
        Default_Handler, Default_Handler,
        /* 48 bis 55: reserviert, SDIO, TIM5, SPI3, vier reserviert */
        Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler,
        Default_Handler, Default_Handler,
        /* 56 bis 63: DMA2_Stream0 bis 4, zwei reserviert, reserviert */
        Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler,
        Default_Handler, Default_Handler,
        /* 64 bis 71: reserviert, reserviert, reserviert, OTG_FS, DMA2_Stream5 bis 7, USART6 */
        Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler,
        Default_Handler, Default_Handler,
        /* 72 bis 79: I2C3_EV, I2C3_ER, sechs reserviert */
        Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler,
        Default_Handler, Default_Handler,
        /* 80 bis 84: reserviert, FPU, zwei reserviert, SPI4 */
        Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler,
    },
};

/* Ein Register an seiner Adresse aus dem Handbuch. */
static volatile uint32_t *board_start_reg(uintptr_t address) { return (volatile uint32_t *)address; }

void Reset_Handler(void) {
    const uintptr_t data = ((uintptr_t)board_data_end - (uintptr_t)board_data_start) / sizeof(uint32_t);
    const uintptr_t bss = ((uintptr_t)board_bss_end - (uintptr_t)board_bss_start) / sizeof(uint32_t);
    *board_start_reg(SCB_VTOR) = (uint32_t)(uintptr_t)&board_vector_table;
    *board_start_reg(SCB_CPACR) |= SCB_CPACR_FPU;
    __asm__ volatile("dsb\n\tisb" : : : "memory");
    for (uintptr_t i = 0u; i < data; i++) {
        board_data_start[i] = board_data_load[i];
    }
    for (uintptr_t i = 0u; i < bss; i++) {
        board_bss_start[i] = 0u;
    }
    (void)main();
    board_halt();
}

void Default_Handler(void) { board_halt(); }
