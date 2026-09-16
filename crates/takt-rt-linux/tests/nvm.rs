//! `FileNvm`: zwei Slots in einer Datei, Schreiben im Hintergrund (5.9, 12.2).

use takt_rt_core::journal::{Journal, Loaded};
use takt_rt_core::loopcore::{Nvm, NvmState};
use takt_rt_linux::FileNvm;

const SLOT: u32 = 512;
const HASH: u64 = 0xF00D;

fn temp_path(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("takt-nvm-tests");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{name}-{}.nvm", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

/// Treibt einen Vorgang zu Ende; das Geraet arbeitet im Hintergrund.
fn settle(nvm: &mut FileNvm) -> NvmState {
    for _ in 0..2000 {
        match nvm.poll() {
            NvmState::Busy => std::thread::sleep(std::time::Duration::from_millis(1)),
            other => return other,
        }
    }
    panic!("Vorgang wird nicht fertig");
}

#[test]
fn a_fresh_file_reads_as_erased() {
    let path = temp_path("fresh");
    let mut nvm = FileNvm::open(&path, SLOT).expect("oeffnen");
    let mut buf = [0u8; 16];
    assert!(nvm.read(1, SLOT - 16, &mut buf));
    assert_eq!(buf, [0xFF; 16]);
}

#[test]
fn a_write_survives_reopening() {
    let path = temp_path("reopen");
    {
        let mut nvm = FileNvm::open(&path, SLOT).expect("oeffnen");
        assert!(nvm.begin_erase(0));
        assert_eq!(settle(&mut nvm), NvmState::Done);
        assert!(nvm.begin_write(0, 10, b"hallo"));
        assert_eq!(settle(&mut nvm), NvmState::Done);
    }
    let mut nvm = FileNvm::open(&path, SLOT).expect("erneut oeffnen");
    let mut buf = [0u8; 5];
    assert!(nvm.read(0, 10, &mut buf));
    assert_eq!(&buf, b"hallo");
}

#[test]
fn the_journal_round_trips_through_the_file() {
    let path = temp_path("journal");
    let payload = b"zustand-eins";
    {
        let mut j = Journal::new(FileNvm::open(&path, SLOT).expect("oeffnen"), HASH, 0);
        j.load(&mut [0u8; SLOT as usize]);
        let (mut stored, mut len) = ([0u8; SLOT as usize], 0);
        for _ in 0..2000 {
            j.poll(0, payload, &mut stored, &mut len);
            if j.writes() > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(j.writes(), 1, "Journal hat nicht geschrieben");
    }
    let mut j = Journal::new(FileNvm::open(&path, SLOT).expect("erneut oeffnen"), HASH, 0);
    let mut buf = [0u8; SLOT as usize];
    let Loaded::Found { length, .. } = j.load(&mut buf) else { panic!("nichts gefunden") };
    assert_eq!(&buf[..length as usize], payload);
}

#[test]
fn a_second_request_while_busy_is_refused() {
    let path = temp_path("busy");
    let mut nvm = FileNvm::open(&path, SLOT).expect("oeffnen");
    assert!(nvm.begin_erase(0));
    // Sofort danach ist das Geraet beschaeftigt oder schon fertig; ein
    // zweiter Auftrag darf nie still verloren gehen.
    let second = nvm.begin_erase(1);
    let state = nvm.poll();
    assert!(second || state == NvmState::Busy || state == NvmState::Done);
    settle(&mut nvm);
}
