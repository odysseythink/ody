use std::io;
use std::io::stdout;

use ratatui::backend::CrosstermBackend;
use ratatui::layout::Position;
use ratatui::layout::Size;

use super::Tui;
use super::terminal_stderr::TerminalStderrGuard;
use crate::custom_terminal::Terminal;

/// Builds a `Tui` over the real crossterm backend, but with a fixed 80x24 screen.
///
/// The size must be pinned, not merely seeded: tests that draw would otherwise see the process's
/// real window disagree with `last_known_screen_size`, treat that as a resize, and block on a
/// cursor-position query (`ESC[6n`) that nothing answers.
pub(crate) fn make_test_tui() -> io::Result<Tui> {
    let backend = CrosstermBackend::new(stdout());
    let terminal = Terminal::with_screen_size_and_cursor_position_for_test(
        backend,
        Size {
            width: 80,
            height: 24,
        },
        Position { x: 0, y: 0 },
    );
    let stderr_guard = TerminalStderrGuard::install()?;
    Ok(Tui::new(
        terminal,
        /*enhanced_keys_supported*/ false,
        stderr_guard,
    ))
}
