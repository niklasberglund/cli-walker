//! Runs the real binary in a pseudo-terminal, the way a user would, and checks what ends up on
//! the screen. This catches what in-memory snapshot tests cannot: raw-mode keyboard input,
//! the inline viewport's terminal queries, and leaving the terminal usable on exit.
//!
//! Unix only for now: ConPTY on Windows answers terminal queries itself and needs separate
//! handling.
#![cfg(unix)]

use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

const ROWS: u16 = 24;
const COLS: u16 = 80;
const TIMEOUT: Duration = Duration::from_secs(10);

/// Device Status Report: the inline viewport asks where the cursor is on startup.
const CURSOR_POSITION_REQUEST: &[u8] = b"\x1b[6n";
const SHOW_CURSOR: &[u8] = b"\x1b[?25h";

/// The binary running in a pseudo-terminal, with a virtual screen fed by its output.
struct Session {
    _master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    output: Receiver<Vec<u8>>,
    screen: vt100::Parser,
    raw: Vec<u8>,
}

impl Session {
    fn spawn() -> Self {
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: ROWS,
                cols: COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open pty");
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_cli-walker"));
        cmd.env("TERM", "xterm-256color");
        let child = pty.slave.spawn_command(cmd).expect("spawn cli-walker");
        drop(pty.slave);

        let mut reader = pty.master.try_clone_reader().expect("pty reader");
        let (tx, output) = mpsc::channel();
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n @ 1..) = reader.read(&mut buf) {
                if tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });

        let writer = pty.master.take_writer().expect("pty writer");
        Session {
            _master: pty.master,
            child,
            writer,
            output,
            screen: vt100::Parser::new(ROWS.try_into().unwrap(), COLS.try_into().unwrap(), 0),
            raw: Vec::new(),
        }
    }

    /// Feeds pending output to the virtual screen, answering cursor position requests the way
    /// a real terminal would.
    fn pump(&mut self, wait: Duration) -> Pump {
        match self.output.recv_timeout(wait) {
            Ok(chunk) => {
                let before = self.raw.len();
                self.raw.extend_from_slice(&chunk);
                self.screen.process(&chunk);
                // Search from a little before the new chunk in case a request was split.
                let from = before.saturating_sub(CURSOR_POSITION_REQUEST.len());
                let requests = count(&self.raw[from..], CURSOR_POSITION_REQUEST);
                for _ in 0..requests {
                    let (row, col) = self.screen.screen().cursor_position();
                    write!(self.writer, "\x1b[{};{}R", row + 1, col + 1).unwrap();
                    self.writer.flush().unwrap();
                }
                Pump::Data
            }
            Err(mpsc::RecvTimeoutError::Timeout) => Pump::Idle,
            Err(mpsc::RecvTimeoutError::Disconnected) => Pump::Closed,
        }
    }

    fn contents(&self) -> String {
        self.screen.screen().contents()
    }

    fn wait_for_text(&mut self, text: &str) {
        let deadline = Instant::now() + TIMEOUT;
        while !self.contents().contains(text) {
            assert!(
                Instant::now() < deadline && self.pump(Duration::from_millis(50)) != Pump::Closed,
                "timed out waiting for {text:?}; screen:\n{}",
                self.contents()
            );
        }
    }

    fn press(&mut self, keys: &[u8]) {
        self.writer.write_all(keys).unwrap();
        self.writer.flush().unwrap();
    }

    /// Waits for the program to exit and returns whether it succeeded.
    fn wait_for_exit(&mut self) -> bool {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                // Drain whatever the program wrote on its way out.
                while self.pump(Duration::from_millis(200)) == Pump::Data {}
                return status.success();
            }
            assert!(
                Instant::now() < deadline,
                "program did not exit; screen:\n{}",
                self.contents()
            );
            self.pump(Duration::from_millis(50));
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[derive(Debug, PartialEq)]
enum Pump {
    Data,
    Idle,
    Closed,
}

fn count(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|w| *w == needle)
        .count()
}

#[test]
fn shows_the_screen_and_quits_on_q() {
    let mut session = Session::spawn();
    session.wait_for_text("CLI-Walker");
    session.wait_for_text("Press q to quit.");

    // Keys that do nothing must not end the program.
    session.press(b"x");
    session.pump(Duration::from_millis(200));
    assert!(
        session.child.try_wait().unwrap().is_none(),
        "exited on an unrelated key"
    );

    session.press(b"q");
    assert!(session.wait_for_exit(), "non-zero exit status");

    // The inline viewport stays on screen after exit, and the cursor is visible again.
    assert!(
        session
            .contents()
            .contains("The Rust port is not functional yet.")
    );
    assert!(session.raw.ends_with(b"\n") || session.raw.ends_with(b"\r\n"));
    assert!(!session.screen.screen().hide_cursor(), "cursor left hidden");
    assert!(count(&session.raw, SHOW_CURSOR) > 0);
}

#[test]
fn quits_on_ctrl_c() {
    let mut session = Session::spawn();
    session.wait_for_text("Press q to quit.");
    // In raw mode Ctrl+C arrives as a key press, not as SIGINT.
    session.press(b"\x03");
    assert!(session.wait_for_exit(), "non-zero exit status");
}
