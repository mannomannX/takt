/* freertos_mpu.h - die Partition der Arena unter FreeRTOS-MPU (12.3,
 * 12.11; M11 Schritt 20b), fuer den Wirt `freertos.c`.
 *
 * Die Regionen der MPU gehoeren FreeRTOS: Privilegierter Code schreibt
 * ueberall, wo keine Region es verbietet, eine Aufgabe ohne Privileg nur in
 * ihre Regionen. Die Arena liegt in keiner davon. Weist die MPU einen
 * Zugriff auf sie ab, kam er also von einer Aufgabe ohne Privileg; der
 * Handler von MemManage haelt das fest, meldet es dem Kern, sobald der
 * laeuft (`takt_core_hardware_fault`), und uebergeht den Befehl. Jede andere
 * Verletzung haelt das Board an.
 */
#ifndef FREERTOS_MPU_H
#define FREERTOS_MPU_H

#include <stddef.h>

#include "takt_sdk.h"

/* Die Arena `arena` mit `bytes` Byte und der Kern `core`, dem der Handler
 * abgewiesene Zugriffe meldet; vor der ersten Aufgabe ohne Privileg. */
void mpu_arena(takt_core *core, const void *arena, size_t bytes);

/* Der Kern laeuft: Ab jetzt meldet der Handler jeden abgewiesenen Zugriff. */
void mpu_core_running(void);

/* 1, wenn die MPU einen Zugriff auf die Arena abwies, sonst 0. */
int mpu_refused(void);

#endif /* FREERTOS_MPU_H */
