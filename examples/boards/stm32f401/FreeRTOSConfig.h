/* FreeRTOSConfig.h - FreeRTOS auf der Black Pill fuer den Port mit Faeden
 * (M11 Schritt 20, `freertos.c`), mit dem Port `ARM_CM4F` des Kerns V11.3.1
 * oder, wenn der Bau `TAKT_FREERTOS_MPU` auf 1 setzt, mit FreeRTOS-MPU
 * (Port `ARM_CM4_MPU`, Schritt 20b).
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

#ifndef TAKT_FREERTOS_MPU
#error "TAKT_FREERTOS_MPU: 1 mit FreeRTOS-MPU (Port ARM_CM4_MPU), sonst 0"
#endif

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
#define configUSE_EVENT_GROUPS 0
#define configUSE_STREAM_BUFFERS 0
#define configUSE_IDLE_HOOK 0
#define configUSE_TICK_HOOK 0
#define configSUPPORT_STATIC_ALLOCATION 1
#define configSUPPORT_DYNAMIC_ALLOCATION 0
#define configCHECK_FOR_STACK_OVERFLOW 2

#define INCLUDE_vTaskDelete 1
#define INCLUDE_vTaskSuspend 1
#define INCLUDE_uxTaskGetStackHighWaterMark2 1

#if TAKT_FREERTOS_MPU
/* Die Leerlaufaufgabe stellt der Wirt; unter FreeRTOS-MPU darf es der Kern
 * nicht (`vApplicationGetIdleTaskMemory`). */
#define configKERNEL_PROVIDED_STATIC_MEMORY 0
/* Die Systemaufrufe einer Aufgabe ohne Privileg rechnen auf einem eigenen
 * Stack in ihrem TCB, in Worten. */
#define configSYSTEM_CALL_STACK_SIZE 128
/* So viele Objekte des Kerns reichen die Hueller weiter: vier Aufgaben und
 * vier Semaphore des Ports, mit Abstand. */
#define configPROTECTED_KERNEL_OBJECT_POOL_SIZE 16
/* Systemaufrufe nur aus den Huellern des Kerns; kein kritischer Abschnitt
 * ohne Privileg. */
#define configENFORCE_SYSTEM_CALLS_FROM_KERNEL_ONLY 1
#define configALLOW_UNPRIVILEGED_CRITICAL_SECTIONS 0
#else
#define configKERNEL_PROVIDED_STATIC_MEMORY 1
#endif

/* Die Prioritaeten im NVIC des STM32F4: vier Bit, oben im Byte. */
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
