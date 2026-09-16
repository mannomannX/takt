//! Jobs auf Arbeiter-Threads (4.5, 12.2): je Slot ein Thread mit dem
//! deklarierten `stack`. Der Tick-Thread uebergibt den Auftrag und holt das
//! Ergebnis ueber ein `AtomicU8` je Slot, wie `FileNvm` — kein Syscall und
//! kein Warten im Tick.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use takt_native::{Kind, Native, Output};
use takt_rt_core::loopcore::{JobState, Jobs};

const IDLE: u8 = 0;
const REQUESTED: u8 = 1;
const DONE: u8 = 2;
const FAILED: u8 = 3;
/// Verworfen, waehrend der Arbeiter noch rechnet; er raeumt auf `IDLE`.
const CANCELLED: u8 = 4;

/// Der Auftrag eines Slots: die Native und ihre Argumente, hinter einer
/// Condvar, auf der der Arbeiter wartet.
type Request = Arc<(Mutex<Option<(Native, Vec<u8>)>>, Condvar)>;

struct Slot {
    state: Arc<AtomicU8>,
    request: Request,
    result: Arc<Mutex<Vec<u8>>>,
}

/// Die Jobs eines Programms auf Linux.
pub struct ThreadJobs {
    slots: Vec<Slot>,
}

impl ThreadJobs {
    /// `slots` Arbeiter, jeder mit `stack` Byte Stack (4.5: `stack` der Native).
    pub fn new(slots: u32, stack: usize) -> std::io::Result<ThreadJobs> {
        let mut out = Vec::with_capacity(slots as usize);
        for _ in 0..slots {
            let slot = Slot {
                state: Arc::new(AtomicU8::new(IDLE)),
                request: Arc::new((Mutex::new(None), Condvar::new())),
                result: Arc::new(Mutex::new(Vec::new())),
            };
            spawn_worker(stack, Arc::clone(&slot.state), Arc::clone(&slot.request), Arc::clone(&slot.result))?;
            out.push(slot);
        }
        Ok(ThreadJobs { slots: out })
    }
}

fn spawn_worker(
    stack: usize,
    state: Arc<AtomicU8>,
    request: Request,
    result: Arc<Mutex<Vec<u8>>>,
) -> std::io::Result<()> {
    thread::Builder::new().name("takt-job".into()).stack_size(stack.max(16 * 1024)).spawn(move || {
        loop {
            let job = {
                let (lock, ready) = &*request;
                let Ok(mut guard) = lock.lock() else { return };
                while guard.is_none() {
                    guard = match ready.wait(guard) {
                        Ok(g) => g,
                        Err(_) => return,
                    };
                }
                guard.take()
            };
            let Some((native, args)) = job else { continue };
            let outcome = run(native, &args);
            if let Ok(mut r) = result.lock() {
                *r = outcome.clone().unwrap_or_default();
            }
            let next = if outcome.is_some() { DONE } else { FAILED };
            // Ein verworfener Lauf endet still: `cancel` hat den Slot
            // markiert, das Ergebnis erreicht das Programm nicht (5.3).
            if state.compare_exchange(REQUESTED, next, Ordering::AcqRel, Ordering::Acquire).is_err() {
                state.store(IDLE, Ordering::Release);
            }
        }
    })?;
    Ok(())
}

/// Zerlegt die Argumente: je Block `u32` Laenge, dann die Bytes (5.9).
fn blocks(args: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 4 <= args.len() {
        let n = u32::from_le_bytes([args[at], args[at + 1], args[at + 2], args[at + 3]]) as usize;
        let Some(block) = args.get(at + 4..at + 4 + n) else { break };
        out.push(block);
        at += 4 + n;
    }
    out
}

/// Ein `bytes<N>` steht im Block als Laenge und Daten; ein Record als
/// seine kanonische Form.
fn payload(kind: Kind, block: &[u8]) -> &[u8] {
    match kind {
        Kind::Bytes => {
            let n = block.get(..4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize).unwrap_or(0);
            block.get(4..4 + n).unwrap_or(&[])
        }
        _ => block,
    }
}

/// Fuehrt die Native ueber den Bloecken aus; das Ergebnis in kanonischer
/// Form, `None` bei einer Signatur, die nicht passt.
fn run(native: Native, args: &[u8]) -> Option<Vec<u8>> {
    use takt_native::sha256::{CTX_MAX_BYTES, Ctx};
    let sig = native.signature();
    let raw = blocks(args);
    if raw.len() != sig.params.len() {
        return None;
    }
    let inputs: Vec<&[u8]> = raw.iter().zip(sig.params).map(|(b, k)| payload(*k, b)).collect();
    let ctx_bytes = |ctx: &Ctx| {
        let mut buf = [0u8; CTX_MAX_BYTES];
        let n = ctx.to_bytes(&mut buf).ok()?;
        Some(buf[..n].to_vec())
    };
    match native {
        Native::Sha256Init => ctx_bytes(&Ctx::new()),
        Native::Sha256Update => {
            let mut ctx = Ctx::from_bytes(inputs.first()?)?;
            ctx.update(inputs.get(1)?);
            ctx_bytes(&ctx)
        }
        Native::Sha256Final => Some(digest(&Ctx::from_bytes(inputs.first()?)?.finish())),
        Native::EcdsaP256Verify => {
            let key = <[u8; 64]>::try_from(*inputs.first()?).ok()?;
            let digest = <[u8; 32]>::try_from(*inputs.get(1)?).ok()?;
            let sig = <[u8; 64]>::try_from(*inputs.get(2)?).ok()?;
            takt_crypto::ecdsa_p256_verify(&key, &digest, &sig).ok().map(|b| vec![u8::from(b)])
        }
        _ => match takt_native::call(native, &inputs)? {
            Output::Digest(d) => Some(digest(&d)),
            Output::Scalar(v) => Some(match sig.ret {
                Kind::U8 => vec![v as u8],
                Kind::U16 => (v as u16).to_le_bytes().to_vec(),
                _ => (v as u32).to_le_bytes().to_vec(),
            }),
        },
    }
}

/// `bytes<32>` in kanonischer Form: Laenge, dann die Bytes.
fn digest(d: &[u8; 32]) -> Vec<u8> {
    let mut out = 32u32.to_le_bytes().to_vec();
    out.extend_from_slice(d);
    out
}

impl Jobs for ThreadJobs {
    fn slots(&self) -> u32 {
        self.slots.len() as u32
    }

    fn begin(&mut self, slot: u32, native: &str, args: &[u8]) -> bool {
        let Some(s) = self.slots.get(slot as usize) else { return false };
        let Some(f) = Native::by_name(native) else { return false };
        if s.state.compare_exchange(IDLE, REQUESTED, Ordering::AcqRel, Ordering::Acquire).is_err() {
            return false;
        }
        let (lock, ready) = &*s.request;
        let Ok(mut guard) = lock.lock() else { return false };
        *guard = Some((f, args.to_vec()));
        ready.notify_one();
        true
    }

    fn poll(&mut self, slot: u32) -> JobState {
        match self.slots.get(slot as usize).map(|s| s.state.load(Ordering::Acquire)) {
            Some(REQUESTED) | Some(CANCELLED) => JobState::Running,
            Some(DONE) => JobState::Done,
            Some(FAILED) => JobState::Failed,
            _ => JobState::Idle,
        }
    }

    fn take(&mut self, slot: u32, into: &mut [u8]) -> usize {
        let Some(s) = self.slots.get(slot as usize) else { return 0 };
        let state = s.state.load(Ordering::Acquire);
        if state != DONE && state != FAILED {
            return 0;
        }
        let n = match s.result.lock() {
            Ok(mut r) if state == DONE => {
                let n = r.len().min(into.len());
                into[..n].copy_from_slice(&r[..n]);
                r.clear();
                n
            }
            _ => 0,
        };
        s.state.store(IDLE, Ordering::Release);
        n
    }

    fn cancel(&mut self, slot: u32) {
        let Some(s) = self.slots.get(slot as usize) else { return };
        // Laeuft er noch, raeumt der Arbeiter; liegt ein Ergebnis bereit,
        // verfaellt es hier.
        if s.state.compare_exchange(REQUESTED, CANCELLED, Ordering::AcqRel, Ordering::Acquire).is_err() {
            let state = s.state.load(Ordering::Acquire);
            if state == DONE || state == FAILED {
                s.state.store(IDLE, Ordering::Release);
            }
        }
    }
}
