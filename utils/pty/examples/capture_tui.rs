//! Capture the exact byte stream a TUI app emits inside a ConPTY, answering
//! common terminal queries (CPR, OSC 10/11, DA, kitty keyboard) the way a real
//! terminal emulator would.
//!
//! Usage: capture_tui <exe> <out.bin> [cols] [rows] [prompt] [settle_secs] [run_secs]

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Scan newly received bytes for terminal queries and queue answers.
struct QueryAnswerer {
    carry: Vec<u8>,
    answers: Vec<Vec<u8>>,
    log: Vec<String>,
}

impl QueryAnswerer {
    fn new() -> Self {
        Self {
            carry: Vec::new(),
            answers: Vec::new(),
            log: Vec::new(),
        }
    }

    fn feed(&mut self, bytes: &[u8], rows: u16) {
        self.carry.extend_from_slice(bytes);
        loop {
            // CPR request
            if let Some(pos) = find_sub(&self.carry, b"\x1b[6n") {
                self.answers
                    .push(format!("\x1b[{rows};1R").into_bytes());
                self.log.push("CPR ? -> answered".into());
                self.carry.drain(..pos + 3);
                continue;
            }
            // OSC 10/11 color queries
            if let Some(pos) = find_sub(&self.carry, b"\x1b]10;?\x1b\\") {
                self.answers
                    .push(b"\x1b]10;rgb:bebe/bebe/bebe\x1b\\".to_vec());
                self.log.push("OSC10 ? -> answered".into());
                self.carry.drain(..pos + 7);
                continue;
            }
            if let Some(pos) = find_sub(&self.carry, b"\x1b]11;?\x1b\\") {
                self.answers
                    .push(b"\x1b]11;rgb:0c0c/0c0c/0c0c\x1b\\".to_vec());
                self.log.push("OSC11 ? -> answered".into());
                self.carry.drain(..pos + 7);
                continue;
            }
            // DA1
            if let Some(pos) = find_sub(&self.carry, b"\x1b[c") {
                self.answers.push(b"\x1b[?1;2c".to_vec());
                self.log.push("DA ? -> answered".into());
                self.carry.drain(..pos + 3);
                continue;
            }
            // kitty keyboard flags query
            if let Some(pos) = find_sub(&self.carry, b"\x1b[?u") {
                self.answers.push(b"\x1b[?0u".to_vec());
                self.log.push("kitty kbd ? -> answered".into());
                self.carry.drain(..pos + 4);
                continue;
            }
            // DECRQM: CSI ? Ps $ p  ->  CSI ? Ps ; 2 $ y
            if let Some((pos, mode)) = find_decrqm(&self.carry) {
                self.answers
                    .push(format!("\x1b[?{mode};2$y").into_bytes());
                self.log.push(format!("DECRQM {mode} ? -> answered"));
                self.carry.drain(..pos);
                continue;
            }
            break;
        }
        // keep the carry bounded but preserve a tail that could be a partial query
        if self.carry.len() > 64 {
            let drop_to = self.carry.len() - 16;
            self.carry.drain(..drop_to);
        }
    }
}

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Returns (end_pos, mode) for a full `ESC [ ? <mode> $ p` sequence.
fn find_decrqm(hay: &[u8]) -> Option<(usize, u16)> {
    let mut i = 0;
    while i + 5 < hay.len() {
        if hay[i] == 0x1b && hay[i + 1] == b'[' && hay[i + 2] == b'?' {
            let mut j = i + 3;
            let start = j;
            while j < hay.len() && hay[j].is_ascii_digit() {
                j += 1;
            }
            if j > start && j + 1 < hay.len() && hay[j] == b'$' && hay[j + 1] == b'p' {
                let mode: u16 = hay[start..j]
                    .iter()
                    .fold(0u16, |acc, d| acc * 10 + (d - b'0') as u16);
                return Some((j + 2, mode));
            }
        }
        i += 1;
    }
    None
}

fn main() {
    let mut args = std::env::args().skip(1);
    let exe = args.next().expect("exe path");
    let out_path = args.next().expect("output path");
    let cols: u16 = args.next().unwrap_or_else(|| "300".into()).parse().unwrap();
    let rows: u16 = args.next().unwrap_or_else(|| "70".into()).parse().unwrap();
    let prompt = args
        .next()
        .unwrap_or_else(|| "reply with the single word: ok".into());
    let settle_secs: u64 = args.next().unwrap_or_else(|| "8".into()).parse().unwrap();
    let run_secs: u64 = args.next().unwrap_or_else(|| "15".into()).parse().unwrap();

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");

    let mut cmd = CommandBuilder::new(&exe);
    for (k, v) in std::env::vars() {
        cmd.env(k, v);
    }
    cmd.cwd(std::env::current_dir().unwrap());

    let mut child = pair.slave.spawn_command(cmd).expect("spawn child");
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().expect("clone reader");
    let writer = pair.master.take_writer().expect("take writer");

    let out = Arc::new(Mutex::new(
        std::fs::File::create(&out_path).expect("create output file"),
    ));
    let times = Arc::new(Mutex::new(
        std::fs::File::create(format!("{out_path}.times")).expect("create times file"),
    ));
    let t0 = std::time::Instant::now();
    let stop = Arc::new(AtomicBool::new(false));
    let answerer = Arc::new(Mutex::new(QueryAnswerer::new()));
    let writer_shared = Arc::new(Mutex::new(writer));

    let out2 = Arc::clone(&out);
    let times2 = Arc::clone(&times);
    let stop2 = Arc::clone(&stop);
    let answerer2 = Arc::clone(&answerer);
    let writer2 = Arc::clone(&writer_shared);
    let reader_thread = std::thread::spawn(move || {
        let mut buf = [0u8; 65536];
        let mut total: u64 = 0;
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    out2.lock().unwrap().write_all(&buf[..n]).unwrap();
                    let ts = t0.elapsed().as_millis();
                    times2
                        .lock()
                        .unwrap()
                        .write_all(format!("{total} {n} {ts}\n").as_bytes())
                        .unwrap();
                    total += n as u64;
                    let answers = {
                        let mut qa = answerer2.lock().unwrap();
                        qa.feed(&buf[..n], rows);
                        std::mem::take(&mut qa.answers)
                    };
                    if !answers.is_empty() {
                        let mut w = writer2.lock().unwrap();
                        for a in answers {
                            let _ = w.write_all(&a);
                        }
                        let _ = w.flush();
                    }
                }
                Err(_) => break,
            }
            if stop2.load(Ordering::Relaxed) {
                break;
            }
        }
    });

    std::thread::sleep(Duration::from_secs(settle_secs));
    eprintln!("[capture] sending prompt (slow char-by-char to avoid paste-burst)");
    {
        let mut w = writer_shared.lock().unwrap();
        for &b in prompt.as_bytes() {
            w.write_all(&[b]).unwrap();
            w.flush().unwrap();
            std::thread::sleep(Duration::from_millis(25));
        }
        std::thread::sleep(Duration::from_millis(400));
        w.write_all(b"\r").unwrap();
        w.flush().unwrap();
    }

    std::thread::sleep(Duration::from_secs(run_secs));

    eprintln!("[capture] interrupting (esc, then ctrl+c x2)");
    {
        let mut w = writer_shared.lock().unwrap();
        let _ = w.write_all(b"\x1b");
        std::thread::sleep(Duration::from_millis(800));
        let _ = w.write_all(b"\x03");
        std::thread::sleep(Duration::from_millis(800));
        let _ = w.write_all(b"\x03");
    }
    std::thread::sleep(Duration::from_secs(2));

    stop.store(true, Ordering::Relaxed);
    let _ = child.kill();
    eprintln!("[capture] query log: {:?}", answerer.lock().unwrap().log);
    eprintln!("[capture] done -> {out_path}");
    std::process::exit(0);
}
