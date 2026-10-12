/* FreeRTOSConfig.h - FreeRTOS auf der Black Pill fuer den Port mit Faeden
 * (M11 Schritt 20, `freertos.c`), mit dem Port `ARM_CM4F` des Kerns V11.3.1.
 *
 * Alles statisch, kein Heap (12.11): Aufgaben, Semaphore und die
 * Leerlaufaufgabe liegen im Speicher des Wirts oder des Kerns. Der Takt des
 * Systems ist eine Millisekunde, der Tick der meisten Programme; der Adapter
 * des Ports misst darin (`ports/freertos`). FreeRTOS fuellt jeden neuen Stack
 * mit 0xA5 und prueft beim Wechsel die Grenze jedes Stacks
 * (`configCHECK_FOR_STACK_OVERFLOW` 2): der Waechter unter den Stacks der
 * Aufgaben (12.3).
 *
 * Die Ausnahmen PendSV und SysTick haben die niedrigste Prioritaet; die
 * Leitung (USART1, 0x10) liegt ueber `configMAX_SYSCALL_INTERRUPT_PRIORITY`
 * und ruft FreeRTOS nie. Die Handler des Ports tragen die Namen von CMSIS,
 * unter denen die Vektortabelle (`startup.c`) sie fuehrt. */
#ifndef FREERTOS_CONFIG_H
#define FREERTOS_CONFIG_H

/* Was die Bedingung eines Asserts nicht haelt, haelt das Board an
 * (`board_halt`). */
#include "board.h"

#define configUSE_PREEMPTION 1
#define configUSE_PORT_OPTIMISED_TASK_SELECTION 1
#define configCPU_CLOCK_HZ BOARD_CORE_HZ
#define configTICK_RATE_HZ 1000U
#define configTICK_TYPE_WIDTH_IN_BITS TICK_TYPE_WIDTH_32_BITS
#define configMAX_PRIORITIES 5
#define configMINIMAL_STACK_SIZE 128
#define configMAX_TASK_NAME_LEN 10
#define configIDLE_SHOULD_YIELD 1
#define configUSE_MUTEXES 0
#define configUSE_TIMERS 0
#define configUSE_IDLE_HOOK 0
#define configUSE_TICK_HOOK 0
#define configSUPPORT_STATIC_ALLOCATION 1
#define configSUPPORT_DYNAMIC_ALLOCATION 0
#define configKERNEL_PROVIDED_STATIC_MEMORY 1
#define configCHECK_FOR_STACK_OVERFLOW 2

#define INCLUDE_vTaskDelete 1
#define INCLUDE_vTaskSuspend 1
#define INCLUDE_uxTaskGetStackHighWaterMark2 1

/* Die Prioritaeten im NVIC des STM32F4: vier Bit, oben im Byte. */
#define configPRIO_BITS 4
#define configKERNEL_INTERRUPT_PRIORITY 0xF0
#define configMAX_SYSCALL_INTERRUPT_PRIORITY 0x20

#define configASSERT(x)    \
    if ((x) == 0) {        \
        board_halt();      \
    }

#define vPortSVCHandler SVC_Handler
#define xPortPendSVHandler PendSV_Handler
#define xPortSysTickHandler SysTick_Handler

#endif /* FREERTOS_CONFIG_H */
