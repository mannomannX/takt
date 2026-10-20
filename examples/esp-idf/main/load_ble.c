/* load_ble.c - BLE neben Takt (load.h; `TAKT_LOAD=ble`, M11 Schritt 21b).
 *
 * Der C6 wirbt ohne Verbindung alle 20 ms und hoert passiv mit (NimBLE):
 * So laufen Controller, ISR und Aufgabe von BLE, ohne dass der Nutzer ein
 * zweites Geraet stellt; dazu schreibt NVS jede Sekunde (load_nvs.h).
 *
 * **Die Sendezeit ist klein:** Eine Werbung ohne Verbindung mit Flags und
 * Namen ist 26 Byte lang, bei 1 Mbit/s etwa 0,21 ms auf jedem der drei
 * Werbekanaele; alle 20 ms belegt das jeden Werbekanal zu etwa 1 %. Der
 * passive Scan sendet nichts.
 *
 * **Die Last belegt sich selbst:** Einzelne Werbungen meldet NimBLE nicht.
 * Darum wirbt die Last in Zyklen von einer Sekunde, beginnt nach jedem den
 * naechsten und zaehlt die Zyklen, die NimBLE als abgelaufen meldet.
 * `load_start` kehrt erst zurueck, wenn der erste abgelaufen ist; von da an
 * zaehlt `load_report`.
 */
#include <stdbool.h>
#include <stdint.h>

#include "esp_timer.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "host/ble_gap.h"
#include "host/ble_hs.h"
#include "load.h"
#include "load_nvs.h"
#include "nimble/nimble_port.h"
#include "nimble/nimble_port_freertos.h"
#include "program.h"

/* Der Abstand zweier Werbungen: 20 ms in Schritten zu 0,625 ms, wie BLE
 * Intervalle zaehlt. */
#define BLE_ADV_UNITS 32U

/* Ein Zyklus der Werbung in Millisekunden. */
#define BLE_CYCLE_MS 1000

/* Intervall und Fenster des passiven Scans: 100 ms und 30 ms, ebenso
 * gezaehlt. */
#define BLE_SCAN_ITVL_UNITS 160U
#define BLE_SCAN_WIN_UNITS 48U

/* So lange wartet `load_start`, bis NimBLE bereit ist und der erste Zyklus
 * abgelaufen, in Schritten von 10 ms. */
#define BLE_READY_TRIES 400U

/* Atomar: 0, bis NimBLE bereit ist; 1, wenn Werbung und Scan laufen; -1,
 * wenn eines davon nicht anlief. */
static int ble_state;

/* Atomar: die abgelaufenen Zyklen der Werbung. */
static uint32_t ble_cycles;

/* Adresse und Parameter der Werbung, gesetzt, bevor die erste beginnt; nur
 * die Aufgabe des Hosts von NimBLE liest sie danach. */
static uint8_t ble_own;
static struct ble_gap_adv_params ble_adv;

/* Die Zaehler, als die Last stand, und wann das war (us); nur `app_main`
 * schreibt und liest sie. */
static uint32_t ble_base_cycles;
static uint32_t ble_base_nvs;
static int64_t ble_since;

static int ble_on_gap(struct ble_gap_event *event, void *arg);

/* Ein Zyklus der Werbung. */
static int ble_advertise(void) {
    return ble_gap_adv_start(ble_own, NULL, BLE_CYCLE_MS, &ble_adv, ble_on_gap, NULL);
}

/* Ereignisse von GAP: Ist ein Zyklus der Werbung abgelaufen, zaehlt er, und
 * der naechste beginnt. Scheitert der Beginn, bleibt die Zahl stehen, und die
 * Pruefung sieht es. */
static int ble_on_gap(struct ble_gap_event *event, void *arg) {
    (void)arg;
    if ((event->type == (uint8_t)BLE_GAP_EVENT_ADV_COMPLETE) && (event->adv_complete.reason == BLE_HS_ETIMEOUT)) {
        (void)__atomic_add_fetch(&ble_cycles, 1U, __ATOMIC_RELAXED);
        (void)ble_advertise();
    }
    return 0;
}

/* NimBLE ist bereit: Werbung und passiver Scan. */
static void ble_on_sync(void) {
    static const uint8_t name[] = {(uint8_t)'t', (uint8_t)'a', (uint8_t)'k', (uint8_t)'t'};
    struct ble_hs_adv_fields fields = {0};
    struct ble_gap_disc_params disc = {0};
    fields.flags = BLE_HS_ADV_F_DISC_GEN | BLE_HS_ADV_F_BREDR_UNSUP;
    fields.name = name;
    fields.name_len = (uint8_t)sizeof name;
    fields.name_is_complete = 1U;
    ble_adv.conn_mode = BLE_GAP_CONN_MODE_NON;
    ble_adv.disc_mode = BLE_GAP_DISC_MODE_GEN;
    ble_adv.itvl_min = BLE_ADV_UNITS;
    ble_adv.itvl_max = BLE_ADV_UNITS;
    disc.itvl = BLE_SCAN_ITVL_UNITS;
    disc.window = BLE_SCAN_WIN_UNITS;
    disc.passive = 1U;
    const bool ok = (ble_hs_id_infer_auto(0, &ble_own) == 0) && (ble_gap_adv_set_fields(&fields) == 0) &&
                    (ble_advertise() == 0) && (ble_gap_disc(ble_own, BLE_HS_FOREVER, &disc, ble_on_gap, NULL) == 0);
    __atomic_store_n(&ble_state, ok ? 1 : -1, __ATOMIC_SEQ_CST);
}

/* Die Aufgabe des Hosts von NimBLE. */
static void ble_host(void *arg) {
    (void)arg;
    nimble_port_run();
    nimble_port_freertos_deinit();
}

/* Werbung und Scan laufen, und der erste Zyklus ist abgelaufen. */
static bool ble_ready(void) {
    const uint32_t cycles = __atomic_load_n(&ble_cycles, __ATOMIC_RELAXED);
    return (__atomic_load_n(&ble_state, __ATOMIC_SEQ_CST) == 1) && (cycles > 0U);
}

int load_start(void) {
    int rc = -1;
    if ((load_nvs_start() == 0) && (nimble_port_init() == ESP_OK)) {
        ble_hs_cfg.sync_cb = ble_on_sync;
        nimble_port_freertos_init(ble_host);
        uint32_t tries = 0U;
        while ((tries < BLE_READY_TRIES) && (__atomic_load_n(&ble_state, __ATOMIC_SEQ_CST) >= 0) && !ble_ready()) {
            vTaskDelay(pdMS_TO_TICKS(10U));
            tries++;
        }
        if (ble_ready()) {
            ble_base_cycles = __atomic_load_n(&ble_cycles, __ATOMIC_RELAXED);
            ble_base_nvs = load_nvs_commits();
            ble_since = esp_timer_get_time();
            rc = 0;
        }
    }
    return rc;
}

void load_banner(void) {
    takt_board_trace("takt last ble\r\n");
    takt_board_trace("  Last: BLE, Werbung ohne Verbindung alle 20 ms (etwa 0,21 ms je Werbekanal, 1 % des Kanals) "
                     "in Zyklen von 1 s, passiver Scan; NVS schreibt jede Sekunde\r\n");
}

void load_report(void) {
    const int64_t ms = (esp_timer_get_time() - ble_since) / 1000;
    const uint32_t cycles_now = __atomic_load_n(&ble_cycles, __ATOMIC_RELAXED);
    const uint32_t cycles = cycles_now - ble_base_cycles;
    const uint32_t commits = load_nvs_commits() - ble_base_nvs;
    takt_board_trace("takt last dauer ");
    takt_board_trace_u64((uint64_t)ms);
    takt_board_trace("werbung ");
    takt_board_trace_u64(cycles);
    takt_board_trace("nvs ");
    takt_board_trace_u64(commits);
    takt_board_trace("\r\n");
}
