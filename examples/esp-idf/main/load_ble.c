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
 */
#include <stdbool.h>
#include <stdint.h>

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

/* Intervall und Fenster des passiven Scans: 100 ms und 30 ms, ebenso
 * gezaehlt. */
#define BLE_SCAN_ITVL_UNITS 160U
#define BLE_SCAN_WIN_UNITS 48U

/* So lange wartet `load_start`, bis NimBLE bereit ist, in Schritten von
 * 10 ms. */
#define BLE_SYNC_TRIES 200U

/* Atomar: 0, bis NimBLE bereit ist; 1, wenn Werbung und Scan laufen; -1,
 * wenn eines davon nicht anlief. */
static int ble_state;

/* Ereignisse von GAP: Werbung und Scan laufen ohne Antwort. */
static int ble_on_gap(struct ble_gap_event *event, void *arg) {
    (void)event;
    (void)arg;
    return 0;
}

/* NimBLE ist bereit: Werbung und passiver Scan. */
static void ble_on_sync(void) {
    static const uint8_t name[] = {(uint8_t)'t', (uint8_t)'a', (uint8_t)'k', (uint8_t)'t'};
    struct ble_hs_adv_fields fields = {0};
    struct ble_gap_adv_params adv = {0};
    struct ble_gap_disc_params disc = {0};
    uint8_t own = 0U;
    fields.flags = BLE_HS_ADV_F_DISC_GEN | BLE_HS_ADV_F_BREDR_UNSUP;
    fields.name = name;
    fields.name_len = (uint8_t)sizeof name;
    fields.name_is_complete = 1U;
    adv.conn_mode = BLE_GAP_CONN_MODE_NON;
    adv.disc_mode = BLE_GAP_DISC_MODE_GEN;
    adv.itvl_min = BLE_ADV_UNITS;
    adv.itvl_max = BLE_ADV_UNITS;
    disc.itvl = BLE_SCAN_ITVL_UNITS;
    disc.window = BLE_SCAN_WIN_UNITS;
    disc.passive = 1U;
    const bool ok = (ble_hs_id_infer_auto(0, &own) == 0) && (ble_gap_adv_set_fields(&fields) == 0) &&
                    (ble_gap_adv_start(own, NULL, BLE_HS_FOREVER, &adv, ble_on_gap, NULL) == 0) &&
                    (ble_gap_disc(own, BLE_HS_FOREVER, &disc, ble_on_gap, NULL) == 0);
    __atomic_store_n(&ble_state, ok ? 1 : -1, __ATOMIC_SEQ_CST);
}

/* Die Aufgabe des Hosts von NimBLE. */
static void ble_host(void *arg) {
    (void)arg;
    nimble_port_run();
    nimble_port_freertos_deinit();
}

int load_start(void) {
    int rc = -1;
    if ((load_nvs_start() == 0) && (nimble_port_init() == ESP_OK)) {
        ble_hs_cfg.sync_cb = ble_on_sync;
        nimble_port_freertos_init(ble_host);
        for (uint32_t i = 0U; (i < BLE_SYNC_TRIES) && (__atomic_load_n(&ble_state, __ATOMIC_SEQ_CST) == 0); i++) {
            vTaskDelay(pdMS_TO_TICKS(10U));
        }
        if (__atomic_load_n(&ble_state, __ATOMIC_SEQ_CST) == 1) {
            rc = 0;
        }
    }
    return rc;
}

void load_banner(void) {
    takt_board_trace("takt last ble\r\n");
    takt_board_trace("  Last: BLE, Werbung ohne Verbindung alle 20 ms (etwa 0,21 ms je Werbekanal, 1 % des Kanals), "
                     "passiver Scan; NVS schreibt jede Sekunde\r\n");
}
