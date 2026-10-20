/* load_nvs.h - NVS-Last neben Takt (M11 Schritt 21b): Eine Aufgabe schreibt
 * jede Sekunde einen Datensatz in die Partition `nvs` und bestaetigt ihn.
 * Jeder Schreibvorgang haelt den Planer an und sperrt jeden Interrupt, der
 * nicht IRAM-sicher ist (plan/m11.md 1.7); ist eine Seite voll, loescht NVS
 * einen Sektor. Der Schritt rechnet derweil in seiner IRAM-ISR weiter. */
#ifndef LOAD_NVS_H
#define LOAD_NVS_H

#include <stdint.h>

/* Die Prioritaet der Aufgaben der Last: ueber `app_main` (1), das die
 * fremde Rechenlast traegt, unter den Jobs (20) und dem Journal (19). */
#define LOAD_PRIORITY 5U

/* Richtet NVS ein und startet die Aufgabe; 0, oder -1. */
int load_nvs_start(void);

/* Wie viele Datensaetze NVS bisher bestaetigte. */
uint32_t load_nvs_commits(void);

#endif /* LOAD_NVS_H */
