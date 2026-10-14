//! Real-PTY verification for the owned transcript (P6).
//!
//! The roadmap is explicit that a `VT100Backend` unit test cannot decide whether the fullscreen
//! mode writes history rows or purges scrollback: those are questions about the *bytes* a real
//! terminal receives. This module therefore drives the owned draw path with stdout attached to an
//! actual pseudo-terminal, records every byte the session emits, and slices that record by phase:
//!
//! | phase | what it stands for |
//! | --- | --- |
//! | `resume` | the first synchronized frame, which also takes the alternate screen |
//! | `redraw` | an ordinary repaint (a streaming tick, a keypress) |
//! | `shorter-window` | a redraw after the terminal got shorter |
//! | `older-page` | a repaint after older history was prepended under the reader |
//! | `handoff` | an external program borrowing the terminal |
//! | `handoff-return` | coming back and repainting |
//!
//! Every phase is delimited by a sentinel written to the same descriptor and read back, so a phase
//! boundary is exact rather than a race against the reader thread.
//!
//! The test mutates the process's stdout, so it is `#[ignore]`d and meant to be run on its own:
//!
//! ```text
//! cargo test -p ody-tui --lib owned_pty -- --ignored --nocapture
//! ```
//!
//! It still cannot cover the multi-emulator matrix (Terminal.app / iTerm2 / Ghostty / VS Code /
//! Windows Terminal / tmux / Zellij); that remains a manual checklist in the P6 report. What it
//! does cover is the escape-sequence contract on a genuine terminal device, which is the part a
//! pipe-backed or in-memory backend cannot observe.

#![cfg(unix)]

use std::io::Read;
use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use super::owned_transcript::history_cell;
use crate::app::App;
use crate::app::test_support::make_test_app;
use crate::tui::test_support::make_test_tui;
use crossterm::event::KeyCode;

/// Purges the terminal's scrollback; an owned session must never emit it.
const SCROLLBACK_PURGE: &[u8] = b"\x1b[3J";
/// Enters the alternate screen buffer.
const ENTER_ALT_SCREEN: &[u8] = b"\x1b[?1049h";
/// Leaves the alternate screen buffer.
const LEAVE_ALT_SCREEN: &[u8] = b"\x1b[?1049l";
/// Phase barrier. An OSC 9 notification is ignored by terminals, and nothing else writes it.
const PHASE_MARK: &[u8] = b"\x1b]9;ody-phase\x07";

/// A pseudo-terminal the test can point stdout at and read back.
struct RealPty {
    master: libc::c_int,
    slave: libc::c_int,
    saved_stdout: libc::c_int,
    attached: bool,
}

impl RealPty {
    fn open(columns: u16, rows: u16) -> std::io::Result<Self> {
        // SAFETY: `posix_openpt` returns a fresh descriptor or -1.
        let master = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY) };
        if master == -1 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `master` is a valid descriptor returned by `posix_openpt`.
        if unsafe { libc::grantpt(master) } == -1 || unsafe { libc::unlockpt(master) } == -1 {
            let err = std::io::Error::last_os_error();
            unsafe { libc::close(master) };
            return Err(err);
        }
        // `ptsname` (not the `_r` variant) is the spelling both Darwin and glibc export. It returns a
        // pointer into a static buffer, which is fine here: the test is single-threaded at this point
        // and the name is consumed immediately by `open`.
        // SAFETY: `master` is a valid descriptor returned by `posix_openpt`.
        let name = unsafe { libc::ptsname(master) };
        if name.is_null() {
            let err = std::io::Error::last_os_error();
            unsafe { libc::close(master) };
            return Err(err);
        }
        // SAFETY: `ptsname` returned a NUL-terminated C string naming this pty's slave device.
        let slave = unsafe { libc::open(name, libc::O_RDWR | libc::O_NOCTTY) };
        if slave == -1 {
            let err = std::io::Error::last_os_error();
            unsafe { libc::close(master) };
            return Err(err);
        }

        // Report a realistic window size, which is what `Tui::draw` sizes the frame from.
        Self::set_window_size(master, columns, rows);

        // Disable output post-processing so the captured stream is byte-for-byte what the session
        // wrote; a real terminal applies `ONLCR` on the way out, which would only blur the counts
        // this test compares.
        // SAFETY: `slave` is a valid descriptor and `termios` is a correctly typed struct.
        unsafe {
            let mut settings: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(slave, &mut settings) == 0 {
                settings.c_oflag &= !libc::OPOST;
                settings.c_lflag &= !libc::ECHO;
                libc::tcsetattr(slave, libc::TCSANOW, &settings);
            }
        }

        // SAFETY: `dup` returns a fresh descriptor or -1.
        let saved_stdout = unsafe { libc::dup(libc::STDOUT_FILENO) };
        if saved_stdout == -1 {
            let err = std::io::Error::last_os_error();
            unsafe {
                libc::close(master);
                libc::close(slave);
            }
            return Err(err);
        }
        Ok(Self {
            master,
            slave,
            saved_stdout,
            attached: false,
        })
    }

    fn set_window_size(fd: libc::c_int, columns: u16, rows: u16) {
        // SAFETY: `fd` is a valid descriptor and `size` is a correctly typed `winsize`.
        unsafe {
            let mut size: libc::winsize = std::mem::zeroed();
            size.ws_row = rows;
            size.ws_col = columns;
            libc::ioctl(fd, libc::TIOCSWINSZ, &size);
        }
    }

    /// Take ownership of the master descriptor, transferring it to the reader thread.
    fn take_master(&mut self) -> libc::c_int {
        let master = self.master;
        self.master = -1;
        master
    }

    fn attach(&mut self) -> std::io::Result<()> {
        // SAFETY: `slave` and fd 1 are both valid for the duration of the call.
        if unsafe { libc::dup2(self.slave, libc::STDOUT_FILENO) } == -1 {
            return Err(std::io::Error::last_os_error());
        }
        self.attached = true;
        Ok(())
    }

    fn restore(&mut self) -> std::io::Result<()> {
        if !self.attached {
            return Ok(());
        }
        // SAFETY: `saved_stdout` is a valid descriptor returned by `dup`.
        if unsafe { libc::dup2(self.saved_stdout, libc::STDOUT_FILENO) } == -1 {
            return Err(std::io::Error::last_os_error());
        }
        self.attached = false;
        Ok(())
    }
}

impl Drop for RealPty {
    fn drop(&mut self) {
        // Restore stdout before closing the slave so the shell never inherits the pty.
        let _ = self.restore();
        for fd in [self.master, self.slave, self.saved_stdout] {
            if fd >= 0 {
                // SAFETY: each descriptor is owned by this struct and closed exactly once.
                unsafe { libc::close(fd) };
            }
        }
    }
}

/// A pty attached to stdout plus a per-phase byte record.
struct Capture {
    pty: Option<RealPty>,
    sink: Arc<Mutex<Vec<u8>>>,
    reader: Option<std::thread::JoinHandle<()>>,
    cursor: usize,
}

impl Capture {
    fn start(columns: u16, rows: u16) -> Self {
        let mut pty = RealPty::open(columns, rows).expect("open pty");
        let master = pty.take_master();
        let sink = Arc::new(Mutex::new(Vec::new()));
        let writer = Arc::clone(&sink);
        let reader = std::thread::spawn(move || {
            // SAFETY: the master descriptor was handed over by `take_master`.
            let mut master =
                unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(master) };
            let mut buf = [0u8; 8192];
            while let Ok(read) = master.read(&mut buf) {
                if read == 0 {
                    break;
                }
                writer
                    .lock()
                    .expect("capture lock")
                    .extend_from_slice(&buf[..read]);
            }
        });
        Self {
            pty: Some(pty),
            sink,
            reader: Some(reader),
            cursor: 0,
        }
    }

    fn attach(&mut self) {
        self.pty
            .as_mut()
            .expect("pty is open")
            .attach()
            .expect("attach stdout");
    }

    /// Resize the pty, which is what a real terminal sends on a window change.
    fn resize(&mut self, columns: u16, rows: u16) {
        let fd = self.pty.as_ref().expect("pty is open").slave;
        RealPty::set_window_size(fd, columns, rows);
    }

    /// Every byte written since the previous call, delimited by a sentinel round-trip.
    fn phase(&mut self, name: &str) -> Vec<u8> {
        let mut stdout = std::io::stdout();
        stdout.write_all(PHASE_MARK).expect("write phase marker");
        stdout.flush().expect("flush pty");

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            {
                let all = self.sink.lock().expect("capture lock");
                if let Some(offset) = find(&all[self.cursor..], PHASE_MARK) {
                    let bytes = all[self.cursor..self.cursor + offset].to_vec();
                    self.cursor += offset + PHASE_MARK.len();
                    return bytes;
                }
            }
            assert!(
                Instant::now() < deadline,
                "the pty never returned the `{name}` phase marker"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Release the pty and return everything captured, markers included.
    fn finish(mut self) -> Vec<u8> {
        std::io::stdout().flush().expect("flush pty");
        if let Some(mut pty) = self.pty.take() {
            pty.restore().expect("restore stdout");
        }
        // The `RealPty` above is dropped here, closing the slave; the reader then sees EOF.
        if let Some(reader) = self.reader.take() {
            reader.join().expect("pty reader thread");
        }
        let all = self.sink.lock().expect("capture lock").clone();
        all
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    find(haystack, needle).is_some()
}

/// Drop the phase markers so a recorded stream can be replayed into a terminal model.
fn strip_markers(stream: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(stream.len());
    let mut rest = stream;
    while let Some(offset) = find(rest, PHASE_MARK) {
        out.extend_from_slice(&rest[..offset]);
        rest = &rest[offset + PHASE_MARK.len()..];
    }
    out.extend_from_slice(rest);
    out
}

/// One recorded phase.
struct Session {
    phases: Vec<(&'static str, Vec<u8>)>,
    total: Vec<u8>,
}

impl Session {
    fn phase(&self, name: &str) -> &[u8] {
        &self
            .phases
            .iter()
            .find(|(phase, _)| *phase == name)
            .unwrap_or_else(|| panic!("no `{name}` phase was recorded"))
            .1
    }
}

/// Drive a full owned session on a real pty and record each phase.
///
/// `turns` controls how much history is loaded. The visible window is the same either way, so the
/// byte counts from two calls are directly comparable.
async fn record_session(turns: usize) -> Session {
    let mut app = make_test_app().await;
    for index in 0..turns {
        app.transcript_cells
            .push(history_cell(&format!("turn {index} body")));
    }

    // Build the session before attaching: constructing a `Tui` probes the terminal palette, and the
    // probe must not be mistaken for owned-mode output.
    let mut tui = make_test_tui().expect("test tui");
    let height = tui.terminal.size().expect("terminal size").height;
    let mut capture = Capture::start(/*columns*/ 80, /*rows*/ height);

    // Everything from here on is recorded, starting with the alternate-screen entry.
    capture.attach();
    tui.prepare_owned_screen(/*owned*/ true)
        .expect("prepare owned screen");
    let draw = |app: &mut App, tui: &mut crate::tui::Tui, height: u16| {
        tui.draw(height, |frame| {
            let area = frame.area();
            app.render_owned_transcript_frame(area, frame);
        })
        .expect("draw owned frame");
    };

    let mut phases = Vec::new();

    draw(&mut app, &mut tui, height);
    phases.push(("resume", capture.phase("resume")));

    // A second frame stands in for an ordinary repaint (a streaming tick, a keypress).
    draw(&mut app, &mut tui, height);
    phases.push(("redraw", capture.phase("redraw")));

    // The terminal got shorter: the frame must shrink with the viewport, not re-emit history.
    let shorter = height.saturating_sub(8).max(6);
    capture.resize(/*columns*/ 80, shorter);
    draw(&mut app, &mut tui, shorter);
    phases.push(("shorter-window", capture.phase("shorter-window")));
    capture.resize(/*columns*/ 80, height);
    draw(&mut app, &mut tui, height);
    phases.push(("restored-window", capture.phase("restored-window")));

    // Older history is prepended under a reader who has scrolled away from the tail.
    for index in 0..turns {
        app.transcript_cells
            .insert(0, history_cell(&format!("older turn {index}")));
    }
    assert!(app.route_owned_transcript_key(&mut tui, key(KeyCode::PageUp)));
    draw(&mut app, &mut tui, height);
    phases.push(("older-page", capture.phase("older-page")));

    // An external program borrows the terminal, then hands it back.
    tui.leave_alt_screen_for_handoff().expect("handoff");
    phases.push(("handoff", capture.phase("handoff")));

    tui.enter_alt_screen().expect("re-enter alt screen");
    draw(&mut app, &mut tui, height);
    phases.push(("handoff-return", capture.phase("handoff-return")));

    tui.set_owned_screen(/*owned*/ false).expect("release");
    phases.push(("release", capture.phase("release")));

    let total = capture.finish();
    Session { phases, total }
}

fn key(code: KeyCode) -> crossterm::event::KeyEvent {
    crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
}

/// The recorded bytes, replayed into a terminal model, reconstruct the owned window.
///
/// The pty records what the session *sent*; replaying that record shows the screen it *produces*.
/// That catches the failure a backend-buffer assertion cannot: an escape sequence that is right in
/// the ratatui buffer but wrong on the wire (a missing move, a stale row, a size the terminal
/// disagrees with).
#[tokio::test]
#[ignore = "real-PTY smoke test; mutates process stdout"]
async fn recorded_bytes_replay_into_the_owned_window() {
    let session = record_session(/*turns*/ 40).await;

    // Replay up to the ordinary repaint: past that point the session resizes, which a fixed-size
    // model cannot follow.
    let mut stream = Vec::new();
    for (name, bytes) in &session.phases {
        stream.extend_from_slice(bytes);
        if *name == "redraw" {
            break;
        }
    }

    let mut parser = vt100::Parser::new(/*rows*/ 24, /*cols*/ 80, /*scrollback_len*/ 0);
    parser.process(&strip_markers(&stream));
    let screen = parser.screen().contents();

    eprintln!("PTY_SCREEN_BEGIN\n{screen}\nPTY_SCREEN_END");
    assert!(
        screen.contains("turn 39 body"),
        "the owned window follows the newest turn: {screen}"
    );
    assert!(
        !screen.contains("turn 0 body"),
        "the owned window must not replay the whole loaded history: {screen}"
    );
}

/// With alternate-screen rendering disabled the session stays on the main screen and takes the
/// native-scrollback path, which is what `--no-alt-screen` and `alternate_screen = "never"` mean.
#[tokio::test]
#[ignore = "real-PTY smoke test; mutates process stdout"]
async fn terminal_fallback_on_a_real_pty_keeps_the_native_scrollback_path() {
    let mut app = make_test_app().await;
    app.transcript_cells.push(history_cell("committed turn"));

    let mut tui = make_test_tui().expect("test tui");
    tui.set_alt_screen_enabled(/*enabled*/ false);
    let height = tui.terminal.size().expect("terminal size").height;
    let mut capture = Capture::start(/*columns*/ 80, /*rows*/ height);

    capture.attach();
    assert_eq!(
        tui.transcript_mode(),
        crate::transcript_mode::TranscriptMode::Terminal,
        "a session without the alternate screen cannot own it"
    );
    tui.set_owned_screen(/*owned*/ true)
        .expect("an owned request is refused rather than honoured");
    assert!(!tui.is_owned_screen());

    // The native path still queues rows above the inline viewport.
    tui.insert_history_lines(vec![ratatui::text::Line::from("committed turn")]);
    assert_eq!(
        tui.pending_history_line_count(),
        1,
        "the fallback keeps the terminal transcript working"
    );

    tui.draw(height, |_frame| {}).expect("draw inline frame");
    let bytes = capture.phase("terminal-frame");

    eprintln!("PTY_FALLBACK bytes={} first_bytes={:?}", bytes.len(), &bytes[..bytes.len().min(32)]);
    assert!(
        !contains(&bytes, ENTER_ALT_SCREEN),
        "the fallback must never take the alternate screen"
    );

    tui.set_owned_screen(/*owned*/ false).expect("release");
    let _ = capture.finish();
}

/// On a real terminal device the owned session takes the alternate screen exactly once, never
/// purges scrollback, and returns the screen on the way out.
#[tokio::test]
#[ignore = "real-PTY smoke test; mutates process stdout"]
async fn owned_session_on_a_real_pty_holds_the_screen_contract() {
    let session = record_session(/*turns*/ 40).await;

    let entries: usize = session
        .phases
        .iter()
        .map(|(_, bytes)| {
            bytes
                .windows(ENTER_ALT_SCREEN.len())
                .filter(|window| *window == ENTER_ALT_SCREEN)
                .count()
        })
        .sum();
    assert!(
        contains(session.phase("resume"), ENTER_ALT_SCREEN),
        "the first owned frame takes the alternate screen"
    );
    assert_eq!(
        entries, 2,
        "exactly one entry at startup and one after the handoff; a repeat would flash the screen"
    );
    assert!(
        contains(session.phase("handoff"), LEAVE_ALT_SCREEN),
        "an external program gets the main screen back"
    );
    assert!(
        contains(session.phase("handoff-return"), ENTER_ALT_SCREEN),
        "handing the terminal back re-enters the owned screen"
    );

    for (name, bytes) in &session.phases {
        assert!(
            !contains(bytes, SCROLLBACK_PURGE),
            "the `{name}` phase must never emit CSI 3J"
        );
    }
    assert!(
        !contains(&session.total, SCROLLBACK_PURGE),
        "the session must never purge the shell's scrollback"
    );

    // The frame cost is the viewport, not the thread. `redraw` is an ordinary repaint with no
    // history change, so what it writes is exactly the window.
    let redraw = session.phase("redraw").len();
    let shorter = session.phase("shorter-window").len();
    eprintln!(
        "PTY_PHASES resume={} redraw={redraw} shorter_window={shorter} older_page={} \
         handoff={} handoff_return={} release={} total={}",
        session.phase("resume").len(),
        session.phase("older-page").len(),
        session.phase("handoff").len(),
        session.phase("handoff-return").len(),
        session.phase("release").len(),
        session.total.len(),
    );
    assert!(
        redraw > 0,
        "an owned repaint has to write something, otherwise the screen is stale"
    );
    assert!(
        shorter <= redraw.saturating_add(redraw / 2),
        "a shorter window must not cost more than the taller repaint: {shorter} vs {redraw}"
    );
}

/// A frame's cost is the viewport, not the thread: 1000 loaded turns cost about the same as 20.
#[tokio::test]
#[ignore = "real-PTY smoke test; mutates process stdout"]
async fn owned_frame_bytes_do_not_scale_with_loaded_history() {
    let small = record_session(/*turns*/ 20).await;
    let large = record_session(/*turns*/ 1000).await;

    // Compare the frames that do no history work of their own: startup and an ordinary repaint.
    // `older-page` is excluded on purpose — a 1000-turn session prepends 1000 more turns there, so
    // the reading position and the number of visible older rows genuinely differ.
    for phase in ["resume", "redraw", "shorter-window", "handoff-return"] {
        let small = small.phase(phase).len();
        let large = large.phase(phase).len();
        eprintln!("PTY_SCALE {phase}: turns=20 -> {small} bytes, turns=1000 -> {large} bytes");
        assert!(
            large <= small.saturating_mul(2),
            "the `{phase}` frame over 1000 loaded turns ({large} bytes) must stay within 2x of \
             the same frame over 20 ({small} bytes): the cost is the viewport, not the history"
        );
    }
}
