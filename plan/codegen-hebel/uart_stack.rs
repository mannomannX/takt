//! Derselbe UART-Stapel wie test_uart_c6_hw.takt in `no_std`-Rust, so
//! idiomatisch wie die C-Fassung: Indexpruefungen bleiben (Rust), keine
//! Bereichstypen, kein Fault-Pfad, keine Meldungen.
#![no_std]
#![allow(static_mut_refs)]

use core::ptr::{read_volatile, write_volatile};

const TICK_US: u32 = 500;
const fn ms(x: u32) -> u16 {
    (x * 1000 / TICK_US) as u16
}

const UART_FIFO: *mut u32 = 0x6000_0000 as *mut u32;
const UART_INT_RAW: *mut u32 = 0x6000_0004 as *mut u32;
const UART_INT_CLR: *mut u32 = 0x6000_0010 as *mut u32;
const UART_STATUS: *mut u32 = 0x6000_001C as *mut u32;

const ACK: u8 = 0x06;
const NAK: u8 = 0x15;
const DATA: u8 = 0x02;
const MAGIC: u8 = 0xA5;
const HDR: usize = 5;
const MAX_PAYLOAD: usize = 1024;
const MAX_ENCODED: usize = 1030;
const RETRIES: u8 = 3;

#[derive(Clone, Copy)]
struct Bytes<const N: usize> {
    len: u16,
    d: [u8; N],
}

impl<const N: usize> Bytes<N> {
    const fn new() -> Self {
        Bytes { len: 0, d: [0; N] }
    }
    fn as_slice(&self) -> &[u8] {
        &self.d[..self.len as usize]
    }
}

#[derive(Clone, Copy, Default)]
struct RxByte {
    b: u8,
    frame: bool,
    parity: bool,
    brk: bool,
}

/// Frames als u16-Laenge plus Bytes, umlaufend.
struct Fq<const CAP: usize> {
    pool: [u8; CAP],
    head: u16,
    used: u16,
}

impl<const CAP: usize> Fq<CAP> {
    const fn new() -> Self {
        Fq { pool: [0; CAP], head: 0, used: 0 }
    }
    fn at(&self, mut i: u32) -> usize {
        if i >= CAP as u32 {
            i -= CAP as u32;
        }
        i as usize
    }
    fn push(&mut self, d: &[u8]) -> bool {
        let n = d.len() as u32;
        if self.used as u32 + 2 + n > CAP as u32 {
            return false;
        }
        let base = self.head as u32 + self.used as u32;
        let at0 = self.at(base);
        self.pool[at0] = n as u8;
        let at1 = self.at(base + 1);
        self.pool[at1] = (n >> 8) as u8;
        for (i, b) in d.iter().enumerate() {
            let at = self.at(base + 2 + i as u32);
            self.pool[at] = *b;
        }
        self.used += 2 + n as u16;
        true
    }
    fn pop(&mut self, d: &mut [u8]) -> usize {
        if self.used == 0 {
            return 0;
        }
        let h = self.head as u32;
        let n = self.pool[self.at(h)] as usize | (self.pool[self.at(h + 1)] as usize) << 8;
        for i in 0..n {
            d[i] = self.pool[self.at(h + 2 + i as u32)];
        }
        self.head = self.at(h + 2 + n as u32) as u16;
        self.used -= 2 + n as u16;
        n
    }
}

struct Ring<T: Copy + Default, const N: usize> {
    e: [T; N],
    head: u16,
    len: u16,
}

impl<T: Copy + Default, const N: usize> Ring<T, N> {
    const fn new(zero: T) -> Self {
        Ring { e: [zero; N], head: 0, len: 0 }
    }
    fn push(&mut self, v: T) -> bool {
        if self.len as usize == N {
            return false;
        }
        self.e[(self.head as usize + self.len as usize) % N] = v;
        self.len += 1;
        true
    }
    fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        let v = self.e[self.head as usize % N];
        self.head = ((self.head as usize + 1) % N) as u16;
        self.len -= 1;
        Some(v)
    }
}

fn cobs_encode(src: &[u8], dst: &mut Bytes<MAX_ENCODED>) {
    let (mut code_at, mut n, mut code) = (0usize, 1usize, 1u8);
    dst.d[0] = 0;
    for &b in src {
        if b != 0 {
            dst.d[n] = b;
            n += 1;
            code += 1;
        }
        if b == 0 || code == 255 {
            dst.d[code_at] = code;
            code_at = n;
            dst.d[n] = 0;
            n += 1;
            code = 1;
        }
    }
    dst.d[code_at] = code;
    dst.d[n] = 0;
    dst.len = (n + 1) as u16;
}

fn cobs_decode(src: &[u8], dst: &mut Bytes<MAX_PAYLOAD>) {
    let (mut i, mut n) = (0usize, 0usize);
    while i < src.len() {
        let code = src[i];
        i += 1;
        if code == 0 {
            dst.len = 0;
            return;
        }
        let mut k = 1u8;
        while k < code && i < src.len() {
            if n >= MAX_PAYLOAD {
                dst.len = 0;
                return;
            }
            dst.d[n] = src[i];
            n += 1;
            i += 1;
            k += 1;
        }
        if code < 255 && i < src.len() {
            if n >= MAX_PAYLOAD {
                dst.len = 0;
                return;
            }
            dst.d[n] = 0;
            n += 1;
        }
    }
    dst.len = n as u16;
}

fn crc16_ccitt(b: &[u8], mut crc: u16) -> u16 {
    for &x in b {
        crc ^= (x as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

#[derive(Default)]
pub struct Outputs {
    pub rs485_de: bool,
    pub link_up: bool,
    pub sent_lines: u16,
    pub got_lines: u16,
    pub d_rx: u32,
    pub d_tx: u32,
    pub d_ovf: u32,
    pub d_ferr: u32,
    pub d_fin: u32,
    pub d_fout: u32,
    pub d_dec: u32,
    pub d_long: u32,
    pub d_crc: u32,
    pub d_retx: u32,
}

#[derive(Clone, Copy, PartialEq)]
enum PortState {
    Run,
    Fault,
}

struct Port {
    state: PortState,
    t: u16,
    tx_stalled_for: u16,
    de_hold: u8,
    overruns: u32,
    frame_errs: u32,
    parity_errs: u32,
    breaks: u32,
    rx_bytes: u32,
    tx_bytes: u32,
    sending: bool,
    cts_ok: bool,
    line_ok: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum FramerState {
    Assemble,
    Sync,
    Resync,
}

struct Framer {
    state: FramerState,
    t: u16,
    frames_in: u32,
    frames_out: u32,
    decode_errors: u32,
    too_long: u32,
    acc: Bytes<MAX_ENCODED>,
    enc: Bytes<MAX_ENCODED>,
    out: Bytes<MAX_PAYLOAD>,
}

#[derive(Clone, Copy, PartialEq)]
enum LinkState {
    Idle,
    Transmit,
    Await,
    Backoff,
    Down,
}

struct Link {
    state: LinkState,
    tx_seq: u8,
    rx_seq: u8,
    last_err: u8,
    attempts: u8,
    t: u16,
    retransmits: u32,
    crc_errors: u32,
    delivered: u32,
    up: bool,
    pending: Bytes<MAX_PAYLOAD>,
    scratch: Bytes<MAX_PAYLOAD>,
}

struct Stack {
    rx_frames: Fq<5120>,
    tx_frames: Fq<3072>,
    app_rx: Fq<1024>,
    app_tx: Fq<5120>,
    rx_raw: Ring<RxByte, 64>,
    tx_raw: Ring<u8, 256>,
    port: Port,
    fr: Framer,
    lk: Link,
    app_t: u16,
    app_n: u16,
    app_m: u16,
    out: Outputs,
}

static mut S: Stack = Stack {
    rx_frames: Fq::new(),
    tx_frames: Fq::new(),
    app_rx: Fq::new(),
    app_tx: Fq::new(),
    rx_raw: Ring::new(RxByte { b: 0, frame: false, parity: false, brk: false }),
    tx_raw: Ring::new(0),
    port: Port {
        state: PortState::Run,
        t: 0,
        tx_stalled_for: 0,
        de_hold: 0,
        overruns: 0,
        frame_errs: 0,
        parity_errs: 0,
        breaks: 0,
        rx_bytes: 0,
        tx_bytes: 0,
        sending: false,
        cts_ok: true,
        line_ok: false,
    },
    fr: Framer {
        state: FramerState::Assemble,
        t: 0,
        frames_in: 0,
        frames_out: 0,
        decode_errors: 0,
        too_long: 0,
        acc: Bytes::new(),
        enc: Bytes::new(),
        out: Bytes::new(),
    },
    lk: Link {
        state: LinkState::Idle,
        tx_seq: 0,
        rx_seq: 0,
        last_err: 0,
        attempts: 0,
        t: 0,
        retransmits: 0,
        crc_errors: 0,
        delivered: 0,
        up: false,
        pending: Bytes::new(),
        scratch: Bytes::new(),
    },
    app_t: 0,
    app_n: 0,
    app_m: 0,
    out: Outputs {
        rs485_de: false,
        link_up: false,
        sent_lines: 0,
        got_lines: 0,
        d_rx: 0,
        d_tx: 0,
        d_ovf: 0,
        d_ferr: 0,
        d_fin: 0,
        d_fout: 0,
        d_dec: 0,
        d_long: 0,
        d_crc: 0,
        d_retx: 0,
    },
};

impl Stack {
    fn port_fault(&mut self) {
        self.port.state = PortState::Fault;
        self.port.t = 0;
        self.out.rs485_de = false;
        self.port.line_ok = false;
    }

    fn port_step(&mut self) {
        if self.port.state == PortState::Fault {
            self.port.t += 1;
            if self.port.t >= ms(100) {
                self.port.state = PortState::Run;
            }
            return;
        }
        // SAFETY: Registerzugriffe des UART0 (12.10).
        let (st, ir) = unsafe { (read_volatile(UART_STATUS), read_volatile(UART_INT_RAW)) };
        self.port.line_ok = true;
        for (bit, counter) in [
            (4u32, &mut self.port.overruns),
            (3, &mut self.port.frame_errs),
            (2, &mut self.port.parity_errs),
            (7, &mut self.port.breaks),
        ] {
            if ir & (1 << bit) != 0 {
                *counter += 1;
                unsafe { write_volatile(UART_INT_CLR, 1 << bit) };
            }
        }
        if st & 0xFF != 0 {
            let b = unsafe { read_volatile(UART_FIFO) } as u8;
            let ev = RxByte { b, frame: ir & 8 != 0, parity: ir & 4 != 0, brk: ir & 0x80 != 0 };
            if !self.rx_raw.push(ev) {
                self.port_fault();
                return;
            }
            self.port.rx_bytes += 1;
        }
        let fifo = (st >> 16) & 0xFF;
        let room = 120 - fifo as i32;
        let mut sent = 0i32;
        while sent < room && self.port.cts_ok {
            let Some(b) = self.tx_raw.pop() else { break };
            unsafe { write_volatile(UART_FIFO, b as u32) };
            self.port.tx_bytes += 1;
            sent += 1;
        }
        if self.tx_raw.len > 0 && sent == 0 {
            self.port.tx_stalled_for += 1;
            if self.port.tx_stalled_for >= ms(50) {
                self.port_fault();
                return;
            }
        } else {
            self.port.tx_stalled_for = 0;
        }
        self.port.sending = sent > 0;
        let busy = self.tx_raw.len > 0 || sent > 0 || fifo > 0;
        self.out.rs485_de = busy || self.port.de_hold > 0;
        if busy {
            self.port.de_hold = ms(3) as u8;
        } else if self.port.de_hold > 0 {
            self.port.de_hold -= 1;
        }
    }

    fn framer_fault(&mut self) {
        self.fr.state = FramerState::Resync;
        self.fr.t = 0;
        self.fr.acc.len = 0;
    }

    fn framer_step(&mut self) {
        let n = self.tx_frames.pop(&mut self.fr.out.d);
        if n > 0 {
            self.fr.out.len = n as u16;
            let (out, enc) = (&self.fr.out, &mut self.fr.enc);
            cobs_encode(out.as_slice(), enc);
            for i in 0..self.fr.enc.len as usize {
                if !self.tx_raw.push(self.fr.enc.d[i]) {
                    self.framer_fault();
                    return;
                }
            }
            self.fr.frames_out += 1;
        }
        if self.fr.state == FramerState::Resync {
            self.fr.t += 1;
            if self.fr.t >= ms(1) {
                self.fr.state = FramerState::Sync;
            }
            return;
        }
        while let Some(ev) = self.rx_raw.pop() {
            if self.fr.state == FramerState::Sync {
                self.fr.acc.len = 0;
                if ev.b == 0 {
                    self.fr.state = FramerState::Assemble;
                }
                continue;
            }
            if ev.frame || ev.parity || ev.brk {
                self.fr.decode_errors += 1;
                self.framer_fault();
                return;
            }
            if ev.b == 0 {
                if self.fr.acc.len > 0 {
                    let (acc, out) = (&self.fr.acc, &mut self.fr.out);
                    cobs_decode(acc.as_slice(), out);
                    if self.fr.out.len > 0 {
                        if !self.rx_frames.push(self.fr.out.as_slice()) {
                            self.framer_fault();
                            return;
                        }
                        self.fr.frames_in += 1;
                    } else {
                        self.fr.decode_errors += 1;
                    }
                }
                self.fr.acc.len = 0;
            } else if (self.fr.acc.len as usize) < MAX_ENCODED {
                let l = self.fr.acc.len as usize;
                self.fr.acc.d[l] = ev.b;
                self.fr.acc.len += 1;
            } else {
                self.fr.too_long += 1;
                self.framer_fault();
                return;
            }
        }
    }

    fn link_down(&mut self) {
        self.lk.state = LinkState::Down;
        self.lk.up = false;
        self.lk.pending.len = 0;
    }

    fn send_frame(&mut self, kind: u8, seq: u8, payload: &[u8]) {
        let mut b = [0u8; HDR + MAX_PAYLOAD + 2];
        let len = payload.len();
        b[0] = MAGIC;
        b[1] = kind;
        b[2] = seq;
        b[3] = len as u8;
        b[4] = (len >> 8) as u8;
        b[HDR..HDR + len].copy_from_slice(payload);
        let c = crc16_ccitt(payload, 0xFFFF);
        b[HDR + len] = (c >> 8) as u8;
        b[HDR + len + 1] = c as u8;
        if !self.tx_frames.push(&b[..HDR + len + 2]) {
            self.link_down();
        }
    }

    fn link_rx(&mut self) {
        loop {
            let mut f = [0u8; MAX_PAYLOAD];
            let n = self.rx_frames.pop(&mut f);
            if n == 0 {
                return;
            }
            let len = f[3] as usize | (f[4] as usize) << 8;
            let valid = n >= HDR && f[0] == MAGIC && matches!(f[1], ACK | NAK | DATA) && len <= MAX_PAYLOAD;
            if !valid {
                self.lk.crc_errors += 1;
                self.lk.last_err = 1;
                continue;
            }
            if self.lk.state == LinkState::Await {
                if f[1] == ACK {
                    self.lk.up = true;
                    self.lk.tx_seq = self.lk.tx_seq.wrapping_add(1);
                    self.lk.state = LinkState::Idle;
                } else if f[1] == NAK {
                    self.lk.last_err = 4;
                    self.lk.retransmits += 1;
                    self.lk.state = LinkState::Backoff;
                    self.lk.t = 0;
                }
            }
            if n < HDR + len + 2 {
                self.lk.last_err = 2;
                continue;
            }
            let want = (f[HDR + len] as u16) << 8 | f[HDR + len + 1] as u16;
            if crc16_ccitt(&f[HDR..HDR + len], 0xFFFF) != want {
                self.lk.crc_errors += 1;
                self.lk.last_err = 1;
                self.send_frame(NAK, f[2], &[]);
                continue;
            }
            if f[1] == DATA {
                if f[2] == self.lk.rx_seq {
                    if !self.app_rx.push(&f[HDR..HDR + len]) {
                        self.link_down();
                        return;
                    }
                    self.lk.delivered += 1;
                    self.lk.rx_seq = self.lk.rx_seq.wrapping_add(1);
                }
                self.send_frame(ACK, f[2], &[]);
            }
        }
    }

    fn link_step(&mut self) {
        if self.lk.crc_errors >= 1_000_000 {
            self.link_down();
        }
        if self.lk.state != LinkState::Down {
            self.link_rx();
        }
        match self.lk.state {
            LinkState::Idle => {
                let n = self.app_tx.pop(&mut self.lk.pending.d);
                if n > 0 {
                    self.lk.pending.len = n as u16;
                    self.lk.attempts = 0;
                    self.transmit();
                }
            }
            LinkState::Transmit => self.transmit(),
            LinkState::Await => {
                self.lk.t += 1;
                if self.lk.t >= ms(25) {
                    self.lk.last_err = 0;
                    self.lk.retransmits += 1;
                    self.lk.state = LinkState::Backoff;
                    self.lk.t = 0;
                }
            }
            LinkState::Backoff => {
                if self.lk.attempts > RETRIES {
                    self.link_down();
                } else {
                    self.lk.t += 1;
                    if self.lk.t >= ms(5) {
                        self.lk.state = LinkState::Transmit;
                    }
                }
            }
            LinkState::Down => {
                if self.port.line_ok && self.port.cts_ok {
                    self.lk.state = LinkState::Idle;
                }
            }
        }
    }

    fn transmit(&mut self) {
        let pending = self.lk.pending;
        self.send_frame(DATA, self.lk.tx_seq, pending.as_slice());
        self.lk.attempts += 1;
        self.lk.state = LinkState::Await;
        self.lk.t = 0;
    }

    fn source_step(&mut self) {
        self.app_t += 1;
        if self.app_t < ms(1000) {
            return;
        }
        self.app_t = 0;
        self.app_tx.push(b"PING\n");
        self.app_n = (self.app_n + 1) % 1000;
        self.out.sent_lines = self.app_n;
        self.out.link_up = self.lk.up;
        self.out.d_rx = self.port.rx_bytes;
        self.out.d_tx = self.port.tx_bytes;
        self.out.d_ovf = self.port.overruns;
        self.out.d_ferr = self.port.frame_errs;
        self.out.d_fin = self.fr.frames_in;
        self.out.d_fout = self.fr.frames_out;
        self.out.d_dec = self.fr.decode_errors;
        self.out.d_long = self.fr.too_long;
        self.out.d_crc = self.lk.crc_errors;
        self.out.d_retx = self.lk.retransmits;
    }

    fn sink_step(&mut self) {
        while self.app_rx.pop(&mut self.lk.scratch.d) > 0 {
            self.app_m = (self.app_m + 1) % 1000;
            self.out.got_lines = self.app_m;
        }
    }
}

/// Ein Tick des Stapels.
#[no_mangle]
pub extern "C" fn stack_tick() {
    // SAFETY: eine Schleife, ein Zustand.
    let s = unsafe { &mut S };
    s.port_step();
    s.framer_step();
    s.link_step();
    s.source_step();
    s.sink_step();
}

/// Die Ausgaenge, fuer den Rahmen.
#[no_mangle]
pub extern "C" fn stack_outputs() -> *const Outputs {
    unsafe { &S.out }
}
