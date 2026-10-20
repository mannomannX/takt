//! Die Treiber aus dem Devicetree (8.10, 12.11; M11 Schritt 19, FB-734,
//! FB-735): `P_zephyr.c` und `P_zephyr.h`, die `takt build --emit embed
//! --zephyr` neben die Lieferform legt.
//!
//! **Ein Pin je Kanal.** Ein Kanal an einem Geraet mit `zephyr-gpio` nennt
//! mit `port` das Label eines Knotens, dessen Eigenschaft `gpios` Pin und
//! Polaritaet traegt. Der erzeugte Code liest und setzt den Pin ueber seine
//! `gpio_dt_spec`; die Polaritaet wendet Zephyr an, `gpio_pin_get_dt` und
//! `gpio_pin_set_dt` sprechen den logischen Pegel.
//!
//! **Ein analoger Eingang je Kanal** (FB-735). Ein Kanal an einem Geraet mit
//! `zephyr-adc` nennt das Label eines Knotens mit `io-channels`; Aufloesung,
//! Verstaerkung und Referenz stehen im Devicetree. Der Treiber wandelt im
//! Tick (`adc_read_dt`) und rechnet den Rohwert in ganze Mikrovolt um
//! (`adc_raw_to_microvolts_dt`). Ohne `calibration` liefert er sie, mit
//! `linear(a, b)` der Konfiguration `a · µV + b` in Fliesskomma; die Zahlen
//! stehen als die Literale der Konfiguration im Code, ohne eigene
//! Umwandlung.
//!
//! **Fehlt der Knoten, scheitert der Bau** an einer Zusicherung, die Kanal
//! und Label nennt: Den Devicetree kennt erst der Bau unter Zephyr. Sie ist
//! `#if !DT_NODE_HAS_PROP(…)` mit `#error`, nicht `BUILD_ASSERT`: Der
//! Praeprozessor haelt vor `GPIO_DT_SPEC_GET` und `ADC_DT_SPEC_GET` an, deren
//! Folgefehler sonst vor der Zusicherung stuenden. Form, Richtung und Typ
//! prueft vorher Pruefung 60 (`takt_sema::calibrated::check_zephyr`).
//!
//! **Den Rest stellt der Wirt:** jeden Treiber, der weder Pin noch analoger
//! Eingang ist. Der Heartbeat eines Geraets entsteht hier nur, wenn jeder
//! seiner Kanaele ein Pin ist; sonst bliebe ein Teil des Geraets ungefragt.
//! Ein Geraet mit `zephyr-adc` hat nur Eingaenge und damit keinen (12.4).

use std::fmt::Write as _;

use takt_llvm::symbols::Prefix;
use takt_mir::expr::ExprKind;
use takt_mir::hardware::{Calibration, Hardware, HwChannel, ZEPHYR_ADC, ZEPHYR_GPIO, calibration, is_devicetree_label};
use takt_mir::program::{Binding, Channel, Program};

use crate::drivers::{Driver, Kind, c_signature};

/// Die Dateien neben der Lieferform.
#[derive(Clone, Debug)]
pub struct Files {
    /// `P_zephyr.c`: die Pins und analogen Eingaenge, ihre Treiber, die
    /// Heartbeats und `P_zephyr_init`.
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

/// Ein analoger Eingang: der Treiber seines Kanals, der Knoten im
/// Devicetree und die Kalibrierung der Konfiguration.
struct Adc<'a> {
    driver: &'a Driver,
    /// Der Name des Kanals im Programm, fuer Zusicherung und Fehler.
    channel: String,
    /// Das Label des Knotens mit `io-channels`.
    label: String,
    /// `linear(a, b)` als Text der Literale; ohne sie ganze Mikrovolt.
    linear: Option<(String, String)>,
}

impl Adc<'_> {
    /// Der Name der `adc_dt_spec` im erzeugten Text.
    fn spec(&self, x: &Prefix) -> String {
        x.name(&format!("adc_{}", self.driver.method))
    }
}

/// Die Dateien fuer das Programm `p` mit den Treibern `drivers` gegen die
/// Konfiguration `hw`; ein Fehler nennt den Kanal, dessen Treiber sich nicht
/// erzeugen laesst.
pub fn files(p: &Program, hw: &Hardware, drivers: &[Driver], x: &Prefix) -> Result<Files, String> {
    let pins = pins(p, hw, drivers)?;
    let adcs = adcs(p, hw, drivers)?;
    let alive = alive(p, drivers, &pins, x);
    Ok(Files { source: source(&pins, &adcs, &alive, x), header: header(x) })
}

/// Der Kanal des Programms an einer Adresse.
fn channel_at<'p>(p: &'p Program, address: &str) -> Option<&'p Channel> {
    p.channels.iter().find(|c| matches!(&c.binding, Binding::Hw(a) if a.text() == address))
}

/// Die Treiber der Kanaele an Geraeten mit dem Treibertyp `kind`, je mit dem
/// Kanal, seinem Namen fuer Meldungen und dem Label des Knotens.
fn bound<'a>(
    p: &Program,
    hw: &'a Hardware,
    drivers: &'a [Driver],
    kind: &str,
) -> Result<Vec<(&'a Driver, &'a HwChannel, String, String)>, String> {
    let mut out = Vec::new();
    for d in drivers {
        if matches!(d.kind, Kind::Alive | Kind::Command | Kind::Sys) {
            continue;
        }
        let Some(entry) = hw.channel(&d.address) else { continue };
        if hw.driver_of(entry) != Some(kind) {
            continue;
        }
        let name = channel_at(p, &d.address).map_or_else(|| d.address.clone(), |c| c.name.clone());
        let label = entry.port.clone().filter(|l| is_devicetree_label(l)).ok_or_else(|| {
            format!("`{name}` an `{}`: `port` nennt kein Label des Devicetree (Pruefung 60)", d.address)
        })?;
        out.push((d, entry, name, label));
    }
    Ok(out)
}

/// Die Pins: je Treiber eines Kanals an einem Geraet mit `zephyr-gpio`.
fn pins<'a>(p: &Program, hw: &'a Hardware, drivers: &'a [Driver]) -> Result<Vec<Pin<'a>>, String> {
    let mut out = Vec::new();
    for (d, entry, name, label) in bound(p, hw, drivers, ZEPHYR_GPIO)? {
        let at = format!("`{name}` an `{}`", d.address);
        let scalar_bool = matches!(d.kind, Kind::Input | Kind::Output) && d.value.is_some_and(|v| v.rust == "bool");
        if !scalar_bool {
            return Err(format!("{at}: `{ZEPHYR_GPIO}` bedient Kanaele mit Rohtyp `bool` (Pruefung 60)"));
        }
        let safe = match d.kind {
            Kind::Output => Some(
                safe_of(channel_at(p, &d.address), entry).ok_or_else(|| format!("{at}: kein `safe`-Wert (12.4)"))?,
            ),
            _ => None,
        };
        out.push(Pin { driver: d, channel: name, label, safe });
    }
    Ok(out)
}

/// Die analogen Eingaenge: je Treiber eines Kanals an einem Geraet mit
/// `zephyr-adc`.
fn adcs<'a>(p: &Program, hw: &'a Hardware, drivers: &'a [Driver]) -> Result<Vec<Adc<'a>>, String> {
    let mut out = Vec::new();
    for (d, entry, name, label) in bound(p, hw, drivers, ZEPHYR_ADC)? {
        let at = format!("`{name}` an `{}`", d.address);
        let value = d
            .value
            .filter(|_| d.kind == Kind::Input)
            .ok_or_else(|| format!("{at}: `{ZEPHYR_ADC}` liest nur Eingaenge mit einem Wert (Pruefung 60)"))?;
        let floating = matches!(value.c, "float" | "double");
        let linear = match entry.calibration.as_deref().map(calibration) {
            None if !floating && value.rust != "bool" => None,
            None => {
                return Err(format!("{at}: ohne `calibration` liefert `{ZEPHYR_ADC}` ganze Mikrovolt (Pruefung 60)"));
            }
            Some(Some(Calibration::Linear { gain, offset })) if floating => {
                Some((gain.to_string(), offset.to_string()))
            }
            Some(Some(Calibration::Linear { .. })) => {
                return Err(format!("{at}: mit `linear(a, b)` ist der Kanal ein `float` (Pruefung 60)"));
            }
            Some(Some(Calibration::Table)) => {
                return Err(format!("{at}: `table(…)` rechnet `{ZEPHYR_ADC}` noch nicht (FB-780)"));
            }
            Some(None) => return Err(format!("{at}: die Kalibrierung ist weder `linear(a, b)` noch `table(…)`")),
        };
        out.push(Adc { driver: d, channel: name, label, linear });
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
/// alle Pins sind, mit der Frage, ob jeder bereit ist. Einen Heartbeat hat
/// nur ein Geraet mit Ausgaengen (12.4), eines mit `zephyr-adc` also keinen.
fn alive<'a>(p: &Program, drivers: &'a [Driver], pins: &[Pin<'_>], x: &Prefix) -> Vec<(&'a Driver, Vec<String>)> {
    let ready = |address: &str| {
        pins.iter().find(|pin| pin.driver.address == address).map(|pin| format!("gpio_is_ready_dt(&{})", pin.spec(x)))
    };
    let mut out = Vec::new();
    for d in drivers.iter().filter(|d| d.kind == Kind::Alive) {
        let bound: Vec<&Channel> = p
            .channels
            .iter()
            .filter(|c| matches!(c.binding, Binding::Hw(_)) && takt_hal::edge::driver_of(c) == d.address)
            .collect();
        let checks: Vec<String> = bound
            .iter()
            .filter_map(|c| match &c.binding {
                Binding::Hw(a) => ready(&a.text()),
                _ => None,
            })
            .collect();
        if !checks.is_empty() && checks.len() == bound.len() {
            out.push((d, checks));
        }
    }
    out
}

/// `P_zephyr.c`.
fn source(pins: &[Pin<'_>], adcs: &[Adc<'_>], alive: &[(&Driver, Vec<String>)], x: &Prefix) -> String {
    let upper = x.as_str().to_uppercase();
    let inputs = !adcs.is_empty() || pins.iter().any(|p| p.driver.kind == Kind::Input);
    let mut s = String::new();
    let _ = writeln!(
        s,
        "/* {x}_zephyr.c - die Treiber aus dem Devicetree fuer `{x}` (8.10, 12.11), erzeugt\n * von `takt build \
         --emit embed --zephyr`: je Kanal an einem Geraet mit `{ZEPHYR_GPIO}` der\n * Pin des Knotens, den \
         `port` nennt, mit dessen Polaritaet, und je Kanal an einem\n * Geraet mit `{ZEPHYR_ADC}` der analoge \
         Eingang des Knotens. Jeden anderen\n * Treiber stellt der Wirt. */"
    );
    if !pins.is_empty() || !adcs.is_empty() {
        let _ = writeln!(s, "#include <errno.h>");
    }
    let _ = writeln!(s, "#include <stddef.h>\n#include <stdint.h>\n");
    if !pins.is_empty() || !adcs.is_empty() {
        let _ = writeln!(s, "#include <zephyr/devicetree.h>");
    }
    if !adcs.is_empty() {
        let _ = writeln!(s, "#include <zephyr/drivers/adc.h>");
    }
    if !pins.is_empty() {
        let _ = writeln!(s, "#include <zephyr/drivers/gpio.h>");
    }
    if !pins.is_empty() || !adcs.is_empty() {
        let _ = writeln!(s);
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
        node(&mut s, &pin.channel, &pin.driver.address, &pin.label, "gpios");
        let _ = writeln!(
            s,
            "static const struct gpio_dt_spec {} = GPIO_DT_SPEC_GET(DT_NODELABEL({}), gpios);\n",
            pin.spec(x),
            pin.label
        );
    }
    for adc in adcs {
        node(&mut s, &adc.channel, &adc.driver.address, &adc.label, "io_channels");
        let _ = writeln!(
            s,
            "static const struct adc_dt_spec {} = ADC_DT_SPEC_GET(DT_NODELABEL({}));\n",
            adc.spec(x),
            adc.label
        );
    }
    for pin in pins {
        driver(&mut s, pin, x);
    }
    for adc in adcs {
        adc_driver(&mut s, adc, x);
    }
    for (d, checks) in alive {
        let _ = writeln!(s, "/* {}: Jeder seiner Kanaele ist bereit. */", d.doc);
        let _ = writeln!(s, "{} {{", c_signature(d, x));
        let _ = writeln!(s, "    (void)user;\n    (void)now;");
        let _ = writeln!(s, "    return ({}) ? 1U : 0U;\n}}\n", checks.join(" && "));
    }
    init(&mut s, pins, adcs, x);
    s
}

/// Die Zusicherung, dass der Knoten `label` die Eigenschaft `property` hat.
fn node(s: &mut String, channel: &str, address: &str, label: &str, property: &str) {
    let shown = property.replace('_', "-");
    let _ = writeln!(s, "/* Kanal `{channel}` an `{address}`: der Knoten `{label}`. */");
    let _ = writeln!(s, "#if !DT_NODE_HAS_PROP(DT_NODELABEL({label}), {property})");
    let _ = writeln!(
        s,
        "#error \"takt: Kanal `{channel}` an `{address}`: kein Knoten `{label}` mit `{shown}` im Devicetree\""
    );
    let _ = writeln!(s, "#endif");
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

/// Was der Wert eines analogen Eingangs vom Typ `c` aus `microvolts`
/// verlangt, und wie er entsteht: die Bedingung, unter der er in den Typ
/// passt (leer, wenn immer), und der Ausdruck.
fn microvolts_into(c: &str) -> (&'static str, String) {
    let fits = match c {
        "int32_t" => return ("", "microvolts".to_string()),
        "int64_t" => "",
        "int16_t" => "(microvolts >= INT16_MIN) && (microvolts <= INT16_MAX)",
        "int8_t" => "(microvolts >= INT8_MIN) && (microvolts <= INT8_MAX)",
        "uint64_t" | "uint32_t" => "microvolts >= 0",
        "uint16_t" => "(microvolts >= 0) && (microvolts <= (int32_t)UINT16_MAX)",
        _ => "(microvolts >= 0) && (microvolts <= (int32_t)UINT8_MAX)",
    };
    (fits, format!("({c})microvolts"))
}

/// Der Treiber eines analogen Eingangs: eine Wandlung im Tick, Mikrovolt aus
/// dem Devicetree, wahlweise die Kalibrierung der Konfiguration.
fn adc_driver(s: &mut String, adc: &Adc<'_>, x: &Prefix) {
    let (spec, upper) = (adc.spec(x), x.as_str().to_uppercase());
    let c = adc.driver.value.map_or("int32_t", |v| v.c);
    let _ = writeln!(s, "/* {}, hw(\"{}\"). */", adc.driver.doc, adc.driver.address);
    let _ = writeln!(s, "{} {{", c_signature(adc.driver, x));
    let scaled = match &adc.linear {
        Some((gain, offset)) => format!("\n     * und `linear({gain}, {offset})` der Konfiguration"),
        None => String::new(),
    };
    let _ = writeln!(
        s,
        "    /* Eine Wandlung an der Tickgrenze (12.6 Zeile 2), Mikrovolt aus\n     * Aufloesung, Verstaerkung und \
         Referenz des Devicetree{scaled}\n     * (8.10). Qualitaet, Grund, Alter und Zeitstempel bleiben, wie der \
         Rahmen\n     * sie vorgibt; scheitert ein Schritt oder passt der Wert nicht in den\n     * Typ des Kanals, \
         ist die Abtastung `BAD`. */"
    );
    let _ = writeln!(s, "    uint16_t narrow = 0U;\n    uint32_t wide = 0U;");
    let _ = writeln!(s, "    struct adc_sequence sequence = {{.buffer = &narrow, .buffer_size = sizeof narrow}};");
    let _ = writeln!(s, "    int32_t microvolts = 0;\n    uint8_t taken = 2U;");
    let _ = writeln!(s, "    (void)user;\n    (void)now;\n    (void)age;\n    (void)t;");
    let _ = writeln!(
        s,
        "    /* Bis 16 Bit legt der Treiber eine Wandlung als `uint16_t` an, darueber\n     * als `uint32_t`, je in \
         der Bytefolge der CPU; die Aufloesung steht im\n     * Devicetree. */"
    );
    let _ = writeln!(s, "    if ({spec}.resolution > 16U) {{");
    let _ = writeln!(s, "        sequence.buffer = &wide;\n        sequence.buffer_size = sizeof wide;\n    }}");
    let _ = writeln!(s, "    int rc = adc_sequence_init_dt(&{spec}, &sequence);");
    let _ = writeln!(s, "    if (rc == 0) {{\n        rc = adc_read_dt(&{spec}, &sequence);\n    }}");
    let _ = writeln!(s, "    if (rc == 0) {{");
    let _ = writeln!(s, "        if ({spec}.resolution > 16U) {{");
    let _ =
        writeln!(s, "            /* Ueber 16 Bit ist der Kanal nie differentiell (`{}`). */", x.name("zephyr_init"));
    let _ = writeln!(s, "            if (wide <= (uint32_t)INT32_MAX) {{");
    let _ = writeln!(s, "                microvolts = (int32_t)wide;\n            }} else {{");
    let _ = writeln!(s, "                rc = -ERANGE;\n            }}");
    let _ = writeln!(s, "        }} else if ({spec}.channel_cfg.differential != 0U) {{");
    let _ = writeln!(s, "            /* Ein differentieller Kanal liefert das Zweierkomplement. */");
    let _ = writeln!(s, "            const int32_t low = (int32_t)narrow;");
    let _ = writeln!(s, "            microvolts = (low >= 0x8000) ? (low - 0x10000) : low;");
    let _ = writeln!(s, "        }} else {{\n            microvolts = (int32_t)narrow;\n        }}\n    }}");
    let _ = writeln!(s, "    if (rc == 0) {{\n        rc = adc_raw_to_microvolts_dt(&{spec}, &microvolts);\n    }}");
    match &adc.linear {
        Some((gain, offset)) => {
            let _ = writeln!(s, "    if (rc == 0) {{");
            let _ = writeln!(
                s,
                "        const double scaled = ((double)({gain}) * (double)microvolts) + (double)({offset});"
            );
            let into = if c == "float" { "(float)scaled" } else { "scaled" };
            let _ = writeln!(s, "        *value = {into};\n        taken = 1U;\n    }}");
        }
        None => {
            let (fits, into) = microvolts_into(c);
            let condition = if fits.is_empty() { "rc == 0".to_string() } else { format!("(rc == 0) && {fits}") };
            let _ = writeln!(s, "    if ({condition}) {{");
            let _ = writeln!(s, "        *value = {into};\n        taken = 1U;\n    }}");
        }
    }
    let _ = writeln!(s, "    if (taken != 1U) {{");
    let _ = writeln!(s, "        *quality = {upper}_ZEPHYR_BAD;\n        *reason = {upper}_ZEPHYR_DRIVER;\n    }}");
    let _ = writeln!(s, "    return taken;\n}}\n");
}

/// `P_zephyr_init`: richtet jeden Pin ein, einen Eingang als Eingang, einen
/// Ausgang auf seinem `safe`-Wert, bis der erste Tick ihn setzt (12.4), dann
/// jeden analogen Eingang. Die Tabellen stehen in der Funktion, die sie
/// liest.
fn init(s: &mut String, pins: &[Pin<'_>], adcs: &[Adc<'_>], x: &Prefix) {
    let name = x.name("zephyr_init");
    if pins.is_empty() && adcs.is_empty() {
        let _ = writeln!(s, "int {name}(const char **channel) {{\n    *channel = NULL;\n    return 0;\n}}");
        return;
    }
    let (pin_tag, adc_tag) = (x.name("zephyr_pin"), x.name("zephyr_adc"));
    if !pins.is_empty() {
        let _ = writeln!(s, "/* Ein Pin, wie `{name}` ihn einrichtet. */");
        let _ = writeln!(s, "struct {pin_tag} {{\n    const struct gpio_dt_spec *spec;\n    gpio_flags_t flags;");
        let _ = writeln!(s, "    const char *channel;\n}};\n");
    }
    if !adcs.is_empty() {
        let _ = writeln!(s, "/* Ein analoger Eingang, wie `{name}` ihn einrichtet. */");
        let _ =
            writeln!(s, "struct {adc_tag} {{\n    const struct adc_dt_spec *spec;\n    const char *channel;\n}};\n");
    }
    let _ = writeln!(s, "int {name}(const char **channel) {{");
    if !pins.is_empty() {
        let _ = writeln!(s, "    static const struct {pin_tag} pins[] = {{");
        for pin in pins {
            let flags = match pin.safe {
                None => "GPIO_INPUT",
                Some(true) => "GPIO_OUTPUT_ACTIVE",
                Some(false) => "GPIO_OUTPUT_INACTIVE",
            };
            let _ = writeln!(s, "        {{&{}, {flags}, \"{}\"}},", pin.spec(x), pin.channel);
        }
        let _ = writeln!(s, "    }};");
    }
    if !adcs.is_empty() {
        let _ = writeln!(s, "    static const struct {adc_tag} adcs[] = {{");
        for adc in adcs {
            let _ = writeln!(s, "        {{&{}, \"{}\"}},", adc.spec(x), adc.channel);
        }
        let _ = writeln!(s, "    }};");
    }
    let _ = writeln!(s, "    int rc = 0;\n    size_t i = 0U;\n    *channel = NULL;");
    if !pins.is_empty() {
        let _ = writeln!(s, "    while ((rc == 0) && (i < (sizeof pins / sizeof pins[0]))) {{");
        let _ = writeln!(s, "        const struct {pin_tag} *pin = &pins[i];");
        let configure = "gpio_pin_configure_dt(pin->spec, pin->flags)";
        let _ = writeln!(s, "        rc = gpio_is_ready_dt(pin->spec) ? {configure} : -ENODEV;");
        let _ = writeln!(s, "        if (rc != 0) {{\n            *channel = pin->channel;\n        }}");
        let _ = writeln!(s, "        i++;\n    }}");
    }
    if !adcs.is_empty() {
        if !pins.is_empty() {
            let _ = writeln!(s, "    i = 0U;");
        }
        let _ = writeln!(s, "    while ((rc == 0) && (i < (sizeof adcs / sizeof adcs[0]))) {{");
        let _ = writeln!(s, "        const struct {adc_tag} *adc = &adcs[i];");
        let _ = writeln!(s, "        rc = adc_is_ready_dt(adc->spec) ? adc_channel_setup_dt(adc->spec) : -ENODEV;");
        let _ = writeln!(
            s,
            "        /* Ein differentieller Kanal ueber 16 Bit: Das Zweierkomplement in 32 Bit\n         * waere nicht eindeutig. */\n        if ((rc \
             == 0) && (adc->spec->channel_cfg.differential != 0U) && (adc->spec->resolution > 16U)) {{\n            \
             rc = -ENOTSUP;\n        }}"
        );
        let _ = writeln!(s, "        if (rc != 0) {{\n            *channel = adc->channel;\n        }}");
        let _ = writeln!(s, "        i++;\n    }}");
    }
    let _ = writeln!(s, "    return rc;\n}}");
}

/// `P_zephyr.h`.
fn header(x: &Prefix) -> String {
    let guard = format!("{}_ZEPHYR_H", x.as_str().to_uppercase());
    let name = x.name("zephyr_init");
    format!(
        "/* {x}_zephyr.h - die Treiber aus dem Devicetree fuer `{x}` (8.10, 12.11),\n * erzeugt von `takt build \
         --emit embed --zephyr`. */\n#ifndef {guard}\n#define {guard}\n\n/* Richtet die Pins und analogen \
         Eingaenge ein, bevor der Wirt\n * `takt_core_init` ruft: 0, oder -errno, und `*channel` nennt den Kanal, \
         den\n * es nicht einrichten konnte. */\nint {name}(const char **channel);\n\n#endif /* {guard} */\n"
    )
}
