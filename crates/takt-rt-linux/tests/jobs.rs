//! `ThreadJobs` (4.5): Ein Job liefert das Ergebnis seiner Native in
//! kanonischer Form, ein verworfener Job liefert keines.

use std::thread;
use std::time::Duration;

use takt_rt_core::loopcore::{JobState, Jobs};
use takt_rt_linux::ThreadJobs;

/// `bytes<N>` als Job-Argument: Blocklaenge, dann Laenge und Daten (5.9).
fn bytes_arg(data: &[u8]) -> Vec<u8> {
    let mut out = ((data.len() + 4) as u32).to_le_bytes().to_vec();
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    out
}

fn wait_done(jobs: &mut ThreadJobs, slot: u32) -> JobState {
    for _ in 0..2000 {
        let s = jobs.poll(slot);
        if s != JobState::Running {
            return s;
        }
        thread::sleep(Duration::from_millis(1));
    }
    jobs.poll(slot)
}

#[test]
fn a_job_delivers_the_digest_of_its_argument() {
    let mut jobs = ThreadJobs::new(2, 64 * 1024).expect("Arbeiter");
    assert_eq!(jobs.slots(), 2);
    assert!(jobs.begin(1, "sha256", &bytes_arg(b"abc")));
    assert_eq!(wait_done(&mut jobs, 1), JobState::Done);
    let mut out = [0u8; 64];
    let n = jobs.take(1, &mut out);
    assert_eq!(n, 36);
    assert_eq!(&out[..4], &32u32.to_le_bytes());
    assert_eq!(out[4..36].to_vec(), takt_native::sha256::sha256(b"abc").to_vec());
    assert_eq!(jobs.poll(1), JobState::Idle);
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("Hex")).collect()
}

/// Der Digest ist ein `bytes<32>` und kommt wie jedes `bytes<N>` als Laenge
/// und Daten (RFC 6979 A.2.5, `grammar/takt-native.md`).
#[test]
fn an_ecdsa_job_verifies_its_signature() {
    let key = hex("60fed4ba255a9d31c961eb74c6356d68c049b8923b61fa6ce669622e60f29fb6\
         7903fe1008b8bc99a41ae9e95628bc64f2f1b20c2d7e9f5177a3c294d4462299");
    let digest = hex("af2bdbe1aa9b6ec1e2ade1d694f41fc71a831d0268e9891562113d8a62add1bf");
    let sig = hex("efd48b2aacb6a8fd1140dd9cd45e81d69d2c877b56aaf991c34d0ea84eaf3716\
         f7cb1c942d657c41d436c7a1b6e29f65f3e900dbb9aff4064dc4ab2f843acda8");
    let args = [bytes_arg(&key), bytes_arg(&digest), bytes_arg(&sig)].concat();
    let mut jobs = ThreadJobs::new(1, 256 * 1024).expect("Arbeiter");
    assert!(jobs.begin(0, "ecdsa_p256_verify", &args));
    assert_eq!(wait_done(&mut jobs, 0), JobState::Done);
    let mut out = [0u8; 8];
    assert_eq!(jobs.take(0, &mut out), 1);
    assert_eq!(out[0], 1, "die Signatur aus RFC 6979 prueft");
}

#[test]
fn an_unknown_native_is_refused() {
    let mut jobs = ThreadJobs::new(1, 64 * 1024).expect("Arbeiter");
    assert!(!jobs.begin(0, "md5", &bytes_arg(b"abc")));
    assert!(!jobs.begin(3, "sha256", &bytes_arg(b"abc")), "kein Slot 3");
    assert_eq!(jobs.poll(0), JobState::Idle);
}

#[test]
fn a_cancelled_job_leaves_no_result() {
    let mut jobs = ThreadJobs::new(1, 64 * 1024).expect("Arbeiter");
    assert!(jobs.begin(0, "sha256", &bytes_arg(b"abc")));
    jobs.cancel(0);
    for _ in 0..2000 {
        if jobs.poll(0) == JobState::Idle {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(jobs.poll(0), JobState::Idle);
    let mut out = [0u8; 64];
    assert_eq!(jobs.take(0, &mut out), 0);
    // Der Slot ist danach frei.
    assert!(jobs.begin(0, "sum8", &bytes_arg(&[1, 2, 3])));
    assert_eq!(wait_done(&mut jobs, 0), JobState::Done);
    assert_eq!(jobs.take(0, &mut out), 1);
    assert_eq!(out[0], 6);
}
