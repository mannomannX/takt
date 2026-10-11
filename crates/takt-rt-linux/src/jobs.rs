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
        let mut out = Vec::with_capacity(usize::try_from(slots).unwrap_or(0));
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

    fn slot(&self, slot: u32) -> Option<&Slot> {
        self.slots.get(usize::try_from(slot).ok()?)
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
            if state.compare_exchange(REQUESTED, next, Ordering::SeqCst, Ordering::SeqCst).is_err() {
                state.store(IDLE, Ordering::SeqCst);
            }
        }
    })?;
    Ok(())
}

/// Zerlegt die Argumente: je Block `u32` Laenge, dann die Bytes (5.9).
fn blocks(args: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut rest = args;
    while let Some((len, tail)) = rest.split_first_chunk::<4>() {
        let Some((block, after)) =
            usize::try_from(u32::from_le_bytes(*len)).ok().and_then(|n| tail.split_at_checked(n))
        else {
            break;
        };
        out.push(block);
        rest = after;
    }
    out
}

/// Ein `bytes<N>`, auch ein Digest oder einer fester Laenge, steht im
/// Block als Laenge und Daten; ein Record oder ein Feld als seine
/// kanonische Form.
fn payload(kind: Kind, block: &[u8]) -> &[u8] {
    match kind {
        Kind::Bytes | Kind::Digest | Kind::Fixed(_) => {
            let Some((len, tail)) = block.split_first_chunk::<4>() else { return &[] };
            usize::try_from(u32::from_le_bytes(*len)).ok().and_then(|n| tail.get(..n)).unwrap_or(&[])
        }
        _ => block,
    }
}

/// Fuehrt die Native ueber den Bloecken aus; das Ergebnis in kanonischer
/// Form, `None` bei einer Signatur, die nicht passt.
fn run(native: Native, args: &[u8]) -> Option<Vec<u8>> {
    use takt_native::sha256::{CTX_MAX_BYTES, Ctx};
    let signature = native.signature();
    let raw = blocks(args);
    if raw.len() != signature.params.len() {
        return None;
    }
    let inputs: Vec<&[u8]> = raw.iter().zip(signature.params).map(|(b, k)| payload(*k, b)).collect();
    let ctx_bytes = |ctx: &Ctx| {
        let mut buf = [0u8; CTX_MAX_BYTES];
        let n = ctx.to_bytes(&mut buf).ok()?;
        buf.get(..n).map(<[u8]>::to_vec)
    };
    match native {
        Native::Sha256Init => ctx_bytes(&Ctx::new()),
        Native::Sha256Update => {
            let mut ctx = Ctx::from_bytes(inputs.first()?)?;
            ctx.update(inputs.get(1)?);
            ctx_bytes(&ctx)
        }
        Native::Sha256Final => Some(digest(&Ctx::from_bytes(inputs.first()?)?.finish())),
        // Ein Argument in falscher Laenge ist keine gueltige Signatur (4.5, FB-487).
        Native::EcdsaP256Verify => {
            let [key, digest, sig] = inputs.as_slice() else { return None };
            let ok = match (<[u8; 64]>::try_from(*key), <[u8; 32]>::try_from(*digest), <[u8; 64]>::try_from(*sig)) {
                (Ok(key), Ok(digest), Ok(sig)) => takt_crypto::ecdsa_p256_verify(&key, &digest, &sig).ok()?,
                _ => false,
            };
            Some(vec![u8::from(ok)])
        }
        Native::Rsa3072Verify => {
            let [key, digest, sig] = inputs.as_slice() else { return None };
            takt_crypto::rsa3072_verify(key, digest, sig).ok().map(|b| vec![u8::from(b)])
        }
        // Ein Tag, der nicht passt, endet als `Err(FAILED)`: `None`.
        Native::AesGcmDecrypt => {
            let [key, nonce, aad, data, tag] = inputs.as_slice() else { return None };
            let mut out = vec![0u8; data.len()];
            let len = takt_crypto::aes_gcm_decrypt(key, nonce, aad, data, tag, &mut out).ok()??;
            Some(digest(out.get(..len)?))
        }
        _ => match takt_native::call(native, &inputs)? {
            Output::Digest(d) => Some(digest(&d)),
            Output::Floats { bytes, len } => bytes.get(..len).map(<[u8]>::to_vec),
            // Die Breite der Rueckgabe: die unteren Bytes des Werts.
            Output::Scalar(v) => {
                let width = match signature.ret {
                    Kind::U8 => 1,
                    Kind::U16 => 2,
                    _ => 4,
                };
                v.to_le_bytes().get(..width).map(<[u8]>::to_vec)
            }
        },
    }
}

/// `bytes<N>` in kanonischer Form: Laenge, dann die Bytes.
fn digest(d: &[u8]) -> Vec<u8> {
    let mut out = u32::try_from(d.len()).unwrap_or(u32::MAX).to_le_bytes().to_vec();
    out.extend_from_slice(d);
    out
}

impl Jobs for ThreadJobs {
    fn slots(&self) -> u32 {
        u32::try_from(self.slots.len()).unwrap_or(u32::MAX)
    }

    fn begin(&mut self, slot: u32, native: &str, args: &[u8]) -> bool {
        let Some(s) = self.slot(slot) else { return false };
        let Some(f) = Native::by_name(native) else { return false };
        if s.state.compare_exchange(IDLE, REQUESTED, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            return false;
        }
        let (lock, ready) = &*s.request;
        let Ok(mut guard) = lock.lock() else { return false };
        *guard = Some((f, args.to_vec()));
        ready.notify_one();
        true
    }

    fn poll(&mut self, slot: u32) -> JobState {
        match self.slot(slot).map(|s| s.state.load(Ordering::SeqCst)) {
            Some(REQUESTED) | Some(CANCELLED) => JobState::Running,
            Some(DONE) => JobState::Done,
            Some(FAILED) => JobState::Failed,
            _ => JobState::Idle,
        }
    }

    fn take(&mut self, slot: u32, into: &mut [u8]) -> usize {
        let Some(s) = self.slot(slot) else { return 0 };
        let state = s.state.load(Ordering::SeqCst);
        if state != DONE && state != FAILED {
            return 0;
        }
        // Ein Ergebnis, das nicht passt, kommt nicht gekuerzt an; seine
        // Laenge sagt es dem Aufrufer (`Jobs::take`).
        let n = match s.result.lock() {
            Ok(mut r) if state == DONE => {
                if let Some(to) = into.get_mut(..r.len()) {
                    to.copy_from_slice(&r);
                }
                let n = r.len();
                r.clear();
                n
            }
            _ => 0,
        };
        s.state.store(IDLE, Ordering::SeqCst);
        n
    }

    fn cancel(&mut self, slot: u32) {
        let Some(s) = self.slot(slot) else { return };
        // Laeuft er noch, raeumt der Arbeiter; liegt ein Ergebnis bereit,
        // verfaellt es hier.
        if s.state.compare_exchange(REQUESTED, CANCELLED, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            let state = s.state.load(Ordering::SeqCst);
            if state == DONE || state == FAILED {
                s.state.store(IDLE, Ordering::SeqCst);
            }
        }
    }
}
