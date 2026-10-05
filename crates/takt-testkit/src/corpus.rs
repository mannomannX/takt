//! Die Programme unter `corpus-try` (plan.md 3), ein Leser fuer alle
//! Korpustests: Liegen die Programme einmal anders oder filtert ein Praefix
//! mehr als gedacht, scheitert der Test, statt ohne eine Datei zu bestehen.

use std::path::{Path, PathBuf};

/// Mindestzahl der positiven Programme in `corpus-try` (Stand: 107). Ein
/// Programm verschwindet nur, wenn diese Zahl mitsinkt.
pub const MIN_PROGRAMS: usize = 107;

/// Das Verzeichnis `corpus-try` im Workspace.
pub fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try"))
}

/// Die `.takt`-Dateien eines Verzeichnisses (ohne Unterverzeichnisse), sortiert.
///
/// # Panics
///
/// Wenn das Verzeichnis nicht lesbar ist.
pub fn takt_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "takt"))
        .collect();
    files.sort();
    files
}

/// Die positiven Programme `corpus-try/*.takt`, ohne die Negativdateien `n0…`.
///
/// # Panics
///
/// Wenn es weniger als [`MIN_PROGRAMS`] sind.
pub fn programs() -> Vec<PathBuf> {
    let files: Vec<PathBuf> = takt_files(&root())
        .into_iter()
        .filter(|p| !p.file_name().and_then(|n| n.to_str()).unwrap_or_default().starts_with("n0"))
        .collect();
    assert!(files.len() >= MIN_PROGRAMS, "nur {} Programme in corpus-try, erwartet {MIN_PROGRAMS}", files.len());
    files
}
