/* partition.h - die Arena in einer Partition (12.11, `Schutz: Partition`;
 * M11 Schritt 19b). Mit `CONFIG_USERSPACE` liegt die Arena in einer eigenen
 * Partition, die keine Domaene eines Fadens im Benutzermodus enthaelt; die
 * Faeden des Ports sind Supervisor-Faeden und erreichen sie. Die Probe
 * belegt, dass ein Faden im Benutzermodus sie nicht beschreiben kann. Im
 * Lauf meldet jede solche Schutzverletzung dem Kern einen Fault (12.3,
 * `takt_core_hardware_fault`). */
#ifndef PARTITION_H
#define PARTITION_H

#include <zephyr/app_memory/app_memdomain.h>

#include "takt_sdk.h"

/* Die Partition der Arena; `K_APP_DMEM(takt_arena)` legt ein Objekt hinein. */
extern struct k_mem_partition takt_arena;

/* Was die Probe sah. */
#define PARTITION_REFUSED 1 /* der Zugriff fiel, der Faden wurde beendet */
#define PARTITION_OPEN 0    /* der Faden schrieb */

/* Laesst einen Faden im Benutzermodus ein Byte an `target` schreiben und
 * nennt, was geschah; < 0, wenn der Faden nicht endete. Vor
 * `takt_core_init`: Gelaenge der Schreibzugriff, beschriebe `init` die Arena
 * ohnehin neu (12.11). */
int partition_probe(void *target);

/* Ab hier meldet ein Faden im Benutzermodus, dessen Zugriff die
 * Schutzeinheit abweist, dem Kern `core` einen Fault: `Runtime(Hardware)` an
 * seiner naechsten Grenze (12.3); der Zephyr-Kern beendet den Faden. Vorher
 * haelt ein solcher Fehler das Board an, ausser bei der Probe. Nach
 * `takt_core_init`, vor dem Start des Ports. */
void partition_guard(takt_core *core);

#endif /* PARTITION_H */
