//! Die Treiber aus dem Devicetree (8.10, 12.11; M11 Schritt 19, FB-734):
//! `P_zephyr.c` und `P_zephyr.h`, die `takt build --emit embed --zephyr`
//! neben die Lieferform legt.
//!
//! **Ein Pin je Kanal.** Ein Kanal an einem Geraet mit `zephyr-gpio` nennt
//! mit `port` das Label eines Knotens, dessen Eigenschaft `gpios` Pin und
//! Polaritaet traegt. Der erzeugte Code liest und setzt den Pin ueber seine
//! `gpio_dt_spec`; die Polaritaet wendet Zephyr an, `gpio_pin_get_dt` und
//! `gpio_pin_set_dt` sprechen den logischen Pegel.
//!
//! **Fehlt der Knoten, scheitert der Bau** an einer Zusicherung, die Kanal
//! und Label nennt: Den Devicetree kennt erst der Bau unter Zephyr. Sie ist
//! `#if !DT_NODE_HAS_PROP(…)` mit `#error`, nicht `BUILD_ASSERT`: Der
//! Praeprozessor haelt vor `GPIO_DT_SPEC_GET` an, dessen Folgefehler sonst
//! vor der Zusicherung stuenden. Form und Rohtyp prueft vorher Pruefung 60
//! (`takt_sema::calibrated::check_zephyr`).
//!
//! **Den Rest stellt der Wirt:** jeden Treiber, der kein Pin ist. Der
//! Heartbeat eines Geraets entsteht hier nur, wenn jeder seiner Kanaele ein
//! Pin ist; sonst bliebe ein Teil des Geraets ungefragt.

use std::fmt::Write as _;

use takt_llvm::symbols::Prefix;
use takt_mir::expr::ExprKind;
use takt_mir::hardware::{Hardware, HwChannel, ZEPHYR_GPIO, is_devicetree_label};
use takt_mir::program::{Binding, Channel, Program};

use crate::drivers::{Driver, Kind, c_signature};

/// Die Dateien neben der Lieferform.
#[derive(Clone, Debug)]
pub struct Files {
    /// `P_zephyr.c`: die Pins, ihre Treiber, die Heartbeats und
    /// `P_zephyr_init`.
    pub source: String,
    /// `P_zephyr.h`: die Deklaration von `P_zephyr_init` fuer den Wirt.
    pub header: String,
}

/// Ein Pin: der Treiber seines Kanals und der Knoten im Devicetree.
struct Pin<'a> {
    driver: &'a Driver,
    /// Der Name des Kanals im Programm, fuer Zusicherung und Fehler.
    channel: String,
    /// Das Label des Knotens.
    label: String,
    /// Wie ein Ausgang steht, bis der erste Tick ihn setzt: sein `safe`-Wert
    /// (12.4). Ein Eingang hat keinen.
    safe: Option<bool>,
}

impl Pin<'_> {
    /// Der Name der `gpio_dt_spec` im erzeugten Text.
    fn spec(&self, x: &Prefix) -> String {
        x.name(&format!("pin_{}", self.driver.method))
    }
}

/// Die Dateien fuer das Programm `p` mit den Treibern `drivers` gegen die
/// Konfiguration `hw`; ein Fehler nennt den Kanal, dessen Pin sich nicht
/// erzeugen laesst.
pub fn files(p: &Program, hw: &Hardware, drivers: &[Driver], x: &Prefix) -> Result<Files, String> {
    let pins = pins(p, hw, drivers)?;
    let alive = alive(p, drivers, &pins);
    Ok(Files { source: source(&pins, &alive, x), header: header(x) })
}

/// Der Kanal des Programms an einer Adresse.
fn channel_at<'p>(p: &'p Program, address: &str) -> Option<&'p Channel> {
    p.channels.iter().find(|c| matches!(&c.binding, Binding::Hw(a) if a.text() == address))
}

/// Die Pins: je Treiber eines Kanals an einem Geraet mit `zephyr-gpio`.
fn pins<'a>(p: &Program, hw: &Hardware, drivers: &'a [Driver]) -> Result<Vec<Pin<'a>>, String> {
    let mut out = Vec::new();
    for d in drivers {
        if matches!(d.kind, Kind::Alive | Kind::Command | Kind::Sys) {
            continue;
        }
        let Some(entry) = hw.channel(&d.address) else { continue };
        if hw.driver_of(entry) != Some(ZEPHYR_GPIO) {
            continue;
        }
        let channel = channel_at(p, &d.address);
        let name = channel.map_or_else(|| d.address.clone(), |c| c.name.clone());
        let at = format!("`{name}` an `{}`", d.address);
        let label = entry
            .port
            .clone()
            .filter(|l| is_devicetree_label(l))
            .ok_or_else(|| format!("{at}: `port` nennt kein Label des Devicetree (Pruefung 60)"))?;
        let scalar_bool = matches!(d.kind, Kind::Input | Kind::Output) && d.value.is_some_and(|v| v.rust == "bool");
        if !scalar_bool {
            return Err(format!("{at}: `{ZEPHYR_GPIO}` bedient Kanaele mit Rohtyp `bool` (Pruefung 60)"));
        }
        let safe = match d.kind {
            Kind::Output => Some(safe_of(channel, entry).ok_or_else(|| format!("{at}: kein `safe`-Wert (12.4)"))?),
            _ => None,
        };
        out.push(Pin { driver: d, channel: name, label, safe });
    }
    Ok(out)
}

/// Der `safe`-Wert eines Ausgangs: aus dem Programm, sonst aus der
/// Konfiguration; Pruefung 60 haelt beide gleich.
fn safe_of(channel: Option<&Channel>, entry: &HwChannel) -> Option<bool> {
    let declared = channel.and_then(|c| c.attrs.safe.as_ref()).and_then(|e| match e.kind {
        ExprKind::Bool(b) => Some(b),
        _ => None,
    });
    declared.or_else(|| entry.safe.as_deref().and_then(|s| s.parse().ok()))
}

/// Die Heartbeats, die hier entstehen: je Geraet, dessen gebundene Kanaele
/// alle Pins sind, mit den Pins, die es ausmachen.
fn alive<'a>(p: &Program, drivers: &'a [Driver], pins: &[Pin<'_>]) -> Vec<(&'a Driver, Vec<usize>)> {
    let mut out = Vec::new();
    for d in drivers.iter().filter(|d| d.kind == Kind::Alive) {
        let bound: Vec<&Channel> = p
            .channels
            .iter()
            .filter(|c| matches!(c.binding, Binding::Hw(_)) && takt_hal::edge::driver_of(c) == d.address)
            .collect();
        let mine: Vec<usize> = pins
            .iter()
            .enumerate()
            .filter(|(_, pin)| {
                bound.iter().any(|c| matches!(&c.binding, Binding::Hw(a) if a.text() == pin.driver.address))
            })
            .map(|(i, _)| i)
            .collect();
        if !mine.is_empty() && mine.len() == bound.len() {
            out.push((d, mine));
        }
    }
    out
}

/// `P_zephyr.c`.
fn source(pins: &[Pin<'_>], alive: &[(&Driver, Vec<usize>)], x: &Prefix) -> String {
    let upper = x.as_str().to_uppercase();
    let inputs = pins.iter().any(|p| p.driver.kind == Kind::Input);
    let mut s = String::new();
    let _ = writeln!(
        s,
        "/* {x}_zephyr.c - die Treiber aus dem Devicetree fuer `{x}` (8.10, 12.11), erzeugt\n * von `takt build \
         --emit embed --zephyr`: je Kanal an einem Geraet mit `{ZEPHYR_GPIO}` der\n * Pin des Knotens, den \
         `port` nennt, mit dessen Polaritaet. Jeden anderen Treiber\n * stellt der Wirt. */"
    );
    if !pins.is_empty() {
        let _ = writeln!(s, "#include <errno.h>");
    }
    let _ = writeln!(s, "#include <stddef.h>\n#include <stdint.h>\n");
    if !pins.is_empty() {
        let _ = writeln!(s, "#include <zephyr/devicetree.h>\n#include <zephyr/drivers/gpio.h>\n");
    }
    let _ = writeln!(s, "#include \"{x}.h\"\n#include \"{x}_zephyr.h\"\n");
    if inputs {
        let _ = writeln!(
            s,
            "/* Eine Abtastung ohne Wert (3.5, 12.6 Zeile 2): `Quality.BAD` mit dem Grund\n * `Reason.DRIVER`. */\n\
             #define {upper}_ZEPHYR_BAD 3U\n#define {upper}_ZEPHYR_DRIVER 3U\n"
        );
    }
    for pin in pins {
        let (channel, address, label) = (&pin.channel, &pin.driver.address, &pin.label);
        let _ = writeln!(s, "/* Kanal `{channel}` an `{address}`: der Knoten `{label}`. */");
        let _ = writeln!(s, "#if !DT_NODE_HAS_PROP(DT_NODELABEL({label}), gpios)");
        let _ = writeln!(
            s,
            "#error \"takt: Kanal `{channel}` an `{address}`: kein Knoten `{label}` mit `gpios` im Devicetree\""
        );
        let _ = writeln!(s, "#endif");
        let _ = writeln!(
            s,
            "static const struct gpio_dt_spec {} = GPIO_DT_SPEC_GET(DT_NODELABEL({label}), gpios);\n",
            pin.spec(x)
        );
    }
    for pin in pins {
        driver(&mut s, pin, x);
    }
    for (d, mine) in alive {
        let ready: Vec<String> = mine.iter().map(|&i| format!("gpio_is_ready_dt(&{})", pins[i].spec(x))).collect();
        let _ = writeln!(s, "/* {}: Jeder seiner Pins ist bereit. */", d.doc);
        let _ = writeln!(s, "{} {{", c_signature(d, x));
        let _ = writeln!(s, "    (void)user;\n    (void)now;");
        let _ = writeln!(s, "    return ({}) ? 1U : 0U;\n}}\n", ready.join(" && "));
    }
    init(&mut s, pins, x);
    s
}

/// Der Treiber eines Pins: Ein Eingang liest den Pin im Tick, ein Ausgang
/// setzt ihn.
fn driver(s: &mut String, pin: &Pin<'_>, x: &Prefix) {
    let (spec, upper) = (pin.spec(x), x.as_str().to_uppercase());
    let _ = writeln!(s, "/* {}, hw(\"{}\"). */", pin.driver.doc, pin.driver.address);
    let _ = writeln!(s, "{} {{", c_signature(pin.driver, x));
    match pin.driver.kind {
        Kind::Input => {
            let _ = writeln!(
                s,
                "    /* Qualitaet, Grund, Alter und Zeitstempel bleiben, wie der Rahmen sie\n     * vorgibt (12.6 \
                 Zeile 2): gelesen an der Tickgrenze. Scheitert das Lesen,\n     * ist die Abtastung `BAD` (8.10). */"
            );
            let _ = writeln!(s, "    const int level = gpio_pin_get_dt(&{spec});");
            let _ = writeln!(s, "    uint8_t taken = 1U;");
            let _ = writeln!(s, "    (void)user;\n    (void)now;\n    (void)age;\n    (void)t;");
            let _ = writeln!(s, "    if (level < 0) {{");
            let _ = writeln!(s, "        *quality = {upper}_ZEPHYR_BAD;\n        *reason = {upper}_ZEPHYR_DRIVER;");
            let _ = writeln!(s, "        taken = 2U;\n    }} else {{");
            let _ = writeln!(s, "        *value = (level > 0) ? 1U : 0U;\n    }}");
            let _ = writeln!(s, "    return taken;");
        }
        _ => {
            let _ = writeln!(s, "    (void)user;\n    (void)now;");
            let _ = writeln!(s, "    return (gpio_pin_set_dt(&{spec}, (value != 0U) ? 1 : 0) == 0) ? 1U : 0U;");
        }
    }
    let _ = writeln!(s, "}}\n");
}

/// `P_zephyr_init`: richtet jeden Pin ein, einen Eingang als Eingang, einen
/// Ausgang auf seinem `safe`-Wert, bis der erste Tick ihn setzt (12.4). Die
/// Tabelle der Pins steht in der Funktion, die sie liest.
fn init(s: &mut String, pins: &[Pin<'_>], x: &Prefix) {
    let name = x.name("zephyr_init");
    let tag = x.name("zephyr_pin");
    if pins.is_empty() {
        let _ = writeln!(s, "int {name}(const char **channel) {{\n    *channel = NULL;\n    return 0;\n}}");
        return;
    }
    let _ = writeln!(s, "/* Ein Pin, wie `{name}` ihn einrichtet. */");
    let _ = writeln!(s, "struct {tag} {{\n    const struct gpio_dt_spec *spec;\n    gpio_flags_t flags;");
    let _ = writeln!(s, "    const char *channel;\n}};\n");
    let _ = writeln!(s, "int {name}(const char **channel) {{");
    let _ = writeln!(s, "    static const struct {tag} pins[] = {{");
    for pin in pins {
        let flags = match pin.safe {
            None => "GPIO_INPUT",
            Some(true) => "GPIO_OUTPUT_ACTIVE",
            Some(false) => "GPIO_OUTPUT_INACTIVE",
        };
        let _ = writeln!(s, "        {{&{}, {flags}, \"{}\"}},", pin.spec(x), pin.channel);
    }
    let _ = writeln!(s, "    }};");
    let _ = writeln!(s, "    int rc = 0;\n    size_t i = 0U;\n    *channel = NULL;");
    let _ = writeln!(s, "    while ((rc == 0) && (i < (sizeof pins / sizeof pins[0]))) {{");
    let _ = writeln!(s, "        const struct {tag} *pin = &pins[i];");
    let configure = "gpio_pin_configure_dt(pin->spec, pin->flags)";
    let _ = writeln!(s, "        rc = gpio_is_ready_dt(pin->spec) ? {configure} : -ENODEV;");
    let _ = writeln!(s, "        if (rc != 0) {{\n            *channel = pin->channel;\n        }}");
    let _ = writeln!(s, "        i++;\n    }}\n    return rc;\n}}");
}

/// `P_zephyr.h`.
fn header(x: &Prefix) -> String {
    let guard = format!("{}_ZEPHYR_H", x.as_str().to_uppercase());
    let name = x.name("zephyr_init");
    format!(
        "/* {x}_zephyr.h - die Treiber aus dem Devicetree fuer `{x}` (8.10, 12.11),\n * erzeugt von `takt build \
         --emit embed --zephyr`. */\n#ifndef {guard}\n#define {guard}\n\n/* Richtet die Pins ein, bevor der \
         Wirt `takt_core_init` ruft: 0, oder\n * -errno, und `*channel` nennt den Kanal, den es nicht \
         einrichten konnte. */\nint {name}(const char **channel);\n\n#endif /* {guard} */\n"
    )
}
