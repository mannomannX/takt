//! Was die Abnahme neben dem Testwirt braucht: die Bibliotheken der Wirte,
//! den Parametervektor eines Profils und die Byteform eines Tunables.
//!
//! **Einen Rahmen erzeugt dieses Modul nicht mehr** (M11 Schritt 9). Der
//! erzeugte Code laeuft auf dem Wirt im Produktrahmen (`takt_frame::mcu`)
//! mit dem Kern des SDK, gefahren vom Testwirt ([`crate::testhost`]) — demselben
//! Rahmen, den die Boards und jeder Wirt binden. Ein zweiter Rahmen fuer
//! den Vergleich wich vom Produkt ab, ohne dass es auffiel.

use takt_mir::program::Program;

/// Das Programm mit den Parameterwerten des Profils `name` (8.4): Defaults,
/// dann das Profil, wie `eval_params` im Interpreter. Der Rahmen setzt den
/// Parametervektor aus den Defaults; ohne diesen Schritt liefe ein Szenario
/// mit Profil nativ mit anderen Parametern als im Interpreter. `None`, wenn
/// es das Profil nicht gibt.
pub fn with_profile(p: &Program, name: &str) -> Option<Program> {
    let profile = p.profiles.iter().find(|x| x.name == name)?;
    let mut out = p.clone();
    for (id, value) in &profile.assignments {
        out.params[id.index()].default = value.clone();
    }
    Some(out)
}

/// Die statische Bibliothek mit den C-Einstiegen der Natives fuer den Wirt
/// (FB-293), einmal je Prozess gebaut: `takt-native-abi` mit Panic-Handler
/// und den Jobs aus `takt-crypto`, im eigenen Zielverzeichnis, damit der
/// Bau nicht auf die Sperre eines laufenden `cargo` wartet.
pub fn native_library() -> Result<std::path::PathBuf, String> {
    static LIB: std::sync::OnceLock<Result<std::path::PathBuf, String>> = std::sync::OnceLock::new();
    LIB.get_or_init(|| native_library_for(None)).clone()
}

/// Dieselbe Bibliothek fuer ein Ziel (`Some(triple)`, etwa die Boards) oder
/// den Wirt (`None`), ohne Zwischenspeicher im Prozess.
pub fn native_library_for(triple: Option<&str>) -> Result<std::path::PathBuf, String> {
    static_library("takt-native-abi", &["host", "ecdsa", "rsa", "aes-gcm"], triple)
}

/// Das SDK-Archiv des C-Wirts (`crates/takt-embed-c`, 12.11, M11 Schritt
/// 18a): der Kern ohne Warten mit Natives und Mathematik, fuer ein Ziel oder
/// den Wirt. Es traegt die Natives selbst; neben [`native_library`] gelinkt,
/// staenden sie zweimal im Abbild.
pub fn sdk_library_for(triple: Option<&str>) -> Result<std::path::PathBuf, String> {
    sdk_library_with(&[], triple)
}

/// Das SDK-Archiv mit den Features `features` (die Jobs aus `takt-crypto`:
/// `ecdsa`, `rsa`, `aes-gcm`), fuer ein Ziel oder den Wirt.
pub fn sdk_library_with(features: &[&str], triple: Option<&str>) -> Result<std::path::PathBuf, String> {
    static_library("takt-embed-c", features, triple)
}

/// Baut das Crate `krate` als statische Bibliothek mit `features`, im
/// eigenen Zielverzeichnis je Satz von Features: Der Bau wartet nicht auf die
/// Sperre eines laufenden `cargo`, und zwei Tests mit verschiedenen Saetzen
/// ueberschreiben einander nicht das Archiv, waehrend der andere es bindet.
fn static_library(krate: &str, features: &[&str], triple: Option<&str>) -> Result<std::path::PathBuf, String> {
    let root = crate::repo_root();
    let mut cargo = std::process::Command::new("cargo");
    cargo.args(["rustc", "--release", "--lib", "--crate-type", "staticlib"]);
    if !features.is_empty() {
        cargo.args(["--features", &features.join(",")]);
    }
    if let Some(triple) = triple {
        cargo.args(["--target", triple]);
    }
    let out =
        cargo
            .arg("--message-format=json-render-diagnostics")
            .arg("--manifest-path")
            .arg(root.join("crates").join(krate).join("Cargo.toml"))
            .arg("--target-dir")
            .arg(crate::target_dir().join(
                features.iter().fold(krate.trim_start_matches("takt-").to_string(), |dir, f| format!("{dir}+{f}")),
            ))
            .output()
            .map_err(|e| format!("cargo: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.contains("\"reason\":\"compiler-artifact\""))
        .filter_map(|l| {
            let rest = &l[l.find("\"filenames\":[\"")? + "\"filenames\":[\"".len()..];
            Some(rest[..rest.find('"')?].replace("\\\\", "\\"))
        })
        .find(|f| f.ends_with(".lib") || f.ends_with(".a"))
        .map(std::path::PathBuf::from)
        .ok_or_else(|| "cargo meldete keine statische Bibliothek".to_string())
}

/// Der Wert einer `tune`-Zeile in kanonischer Byteform (5.9), wie ihn
/// `takt_tune_value` nimmt; `None`, wenn der Text kein Wert des Parametertyps ist.
/// Die Range prueft `takt_tune_value` selbst, wie im Produktrahmen.
pub fn tune_bytes(p: &Program, index: usize, text: &str) -> Option<Vec<u8>> {
    let ty = p.params.get(index)?.ty;
    let v = takt_interp::trace::parse_value(text, ty, p).ok()?;
    takt_interp::bytes::encode(p, &v, ty).ok()
}
