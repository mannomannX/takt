//! Das NVM der Linux-Box: zwei Slots in einer Datei (5.9, 12.2).
//!
//! 12.2 verlangt fuer den Tick-Thread „keine Syscalls und keine
//! Allokation". Die Datei-I/O laeuft darum in einem eigenen Thread; der
//! Tick reicht Auftraege ueber einen Zustand durch und fragt sie ab —
//! dieselbe Form wie ein Flash-Controller, der im Hintergrund loescht.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use takt_rt_core::loopcore::{Nvm, NvmState};

const IDLE: u8 = 0;
const REQUESTED: u8 = 1;
const DONE: u8 = 2;
const FAILED: u8 = 3;

/// Was der Hintergrund-Thread als Naechstes tut.
enum Job {
    None,
    Erase(u8),
    Write { slot: u8, offset: u32, bytes: Vec<u8> },
}

/// Zwei Slots in einer Datei; Schreiben im Hintergrund.
pub struct FileNvm {
    file: File,
    slot_size: u32,
    state: Arc<AtomicU8>,
    job: Arc<Mutex<Job>>,
}

impl FileNvm {
    /// Oeffnet oder legt die Datei an; ein neuer Speicher ist geloescht
    /// (`0xFF`), wie ein frischer Flash.
    pub fn open(path: &Path, slot_size: u32) -> std::io::Result<FileNvm> {
        let mut file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
        let want = u64::from(slot_size) * 2;
        if file.metadata()?.len() < want {
            file.set_len(0)?;
            file.seek(SeekFrom::Start(0))?;
            let blank = vec![0xFFu8; slot_size as usize];
            file.write_all(&blank)?;
            file.write_all(&blank)?;
            file.sync_all()?;
        }
        let state = Arc::new(AtomicU8::new(IDLE));
        let job = Arc::new(Mutex::new(Job::None));
        let worker_file = file.try_clone()?;
        spawn_worker(worker_file, slot_size, Arc::clone(&state), Arc::clone(&job));
        Ok(FileNvm { file, slot_size, state, job })
    }

    fn submit(&mut self, job: Job) -> bool {
        if self.state.load(Ordering::Acquire) == REQUESTED {
            return false;
        }
        if let Ok(mut slot) = self.job.lock() {
            *slot = job;
        } else {
            return false;
        }
        self.state.store(REQUESTED, Ordering::Release);
        true
    }
}

/// Der Hintergrund-Thread: wartet auf einen Auftrag, fuehrt ihn aus,
/// meldet das Ergebnis. Er endet, wenn der Speicher fallen gelassen wird.
fn spawn_worker(mut file: File, slot_size: u32, state: Arc<AtomicU8>, job: Arc<Mutex<Job>>) {
    thread::spawn(move || {
        loop {
            if Arc::strong_count(&state) == 1 {
                return;
            }
            if state.load(Ordering::Acquire) != REQUESTED {
                thread::sleep(Duration::from_millis(1));
                continue;
            }
            let taken = match job.lock() {
                Ok(mut j) => std::mem::replace(&mut *j, Job::None),
                Err(_) => Job::None,
            };
            let ok = match taken {
                Job::None => true,
                Job::Erase(slot) => {
                    let blank = vec![0xFFu8; slot_size as usize];
                    write_at(&mut file, u64::from(slot) * u64::from(slot_size), &blank)
                }
                Job::Write { slot, offset, bytes } => {
                    write_at(&mut file, u64::from(slot) * u64::from(slot_size) + u64::from(offset), &bytes)
                }
            };
            state.store(if ok { DONE } else { FAILED }, Ordering::Release);
        }
    });
}

fn write_at(file: &mut File, at: u64, bytes: &[u8]) -> bool {
    file.seek(SeekFrom::Start(at)).is_ok() && file.write_all(bytes).is_ok() && file.sync_data().is_ok()
}

impl Nvm for FileNvm {
    fn slot_size(&self) -> u32 {
        self.slot_size
    }

    fn begin_erase(&mut self, slot: u8) -> bool {
        slot < 2 && self.submit(Job::Erase(slot))
    }

    fn begin_write(&mut self, slot: u8, offset: u32, bytes: &[u8]) -> bool {
        if slot > 1 || offset.saturating_add(bytes.len() as u32) > self.slot_size {
            return false;
        }
        self.submit(Job::Write { slot, offset, bytes: bytes.to_vec() })
    }

    fn poll(&mut self) -> NvmState {
        match self.state.load(Ordering::Acquire) {
            REQUESTED => NvmState::Busy,
            DONE => {
                self.state.store(IDLE, Ordering::Release);
                NvmState::Done
            }
            FAILED => {
                self.state.store(IDLE, Ordering::Release);
                NvmState::Failed
            }
            _ => NvmState::Idle,
        }
    }

    fn read(&mut self, slot: u8, offset: u32, into: &mut [u8]) -> bool {
        if slot > 1 || offset.saturating_add(into.len() as u32) > self.slot_size {
            return false;
        }
        let at = u64::from(slot) * u64::from(self.slot_size) + u64::from(offset);
        self.file.seek(SeekFrom::Start(at)).is_ok() && self.file.read_exact(into).is_ok()
    }
}
