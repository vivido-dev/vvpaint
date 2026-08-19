use std::io::{self, Write};
#[cfg(unix)]
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{DisableMouseCapture, EnableMouseCapture, MouseEvent},
    execute,
    style::Print,
    terminal::{
        Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
        enable_raw_mode,
    },
};

use crate::canvas::Point;

pub const RESERVED_UI_ROWS: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub columns: u32,
    pub rows: u32,
    pub canvas_rows: u32,
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub grid_origin_x: u32,
    pub grid_origin_y: u32,
    pub cell_width: u32,
    pub cell_height: u32,
    pub backing_width: u32,
    pub backing_height: u32,
}

impl Layout {
    pub fn toolbar_row(self, index: u32) -> Option<u16> {
        (index < 2 && self.rows > self.canvas_rows + index)
            .then(|| u16::try_from(self.canvas_rows + index).unwrap_or(u16::MAX))
    }

    pub fn message_row(self) -> Option<u16> {
        (self.rows > self.canvas_rows + 2)
            .then(|| u16::try_from(self.canvas_rows + 2).unwrap_or(u16::MAX))
    }

    pub fn canvas_display_width(self) -> u32 {
        self.columns
            .saturating_mul(self.cell_width)
            .min(self.viewport_width)
    }

    pub fn canvas_display_height(self) -> u32 {
        self.canvas_rows
            .saturating_mul(self.cell_height)
            .min(self.viewport_height)
    }
}

pub struct TerminalSession {
    mouse_mode: CoordinateMode,
}

impl TerminalSession {
    pub fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let mut output = io::stdout();
        if let Err(error) = execute!(
            output,
            EnterAlternateScreen,
            EnableMouseCapture,
            Print("\x1b[?1016h"),
            Hide,
            Clear(ClearType::All),
            MoveTo(0, 0)
        ) {
            let _ = disable_raw_mode();
            return Err(error.into());
        }
        output.flush()?;
        let mouse_mode = detect_mouse_mode(&mut output);
        Ok(Self { mouse_mode })
    }

    /// Mapper for the coordinate space this terminal actually reports.
    pub fn mouse_mapper(&self) -> MouseMapper {
        MouseMapper::new(self.mouse_mode)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let mut output = io::stdout();
        let _ = execute!(
            output,
            Show,
            Print("\x1b[?1016l"),
            DisableMouseCapture,
            LeaveAlternateScreen,
            MoveTo(0, 0)
        );
        let _ = output.flush();
        let _ = disable_raw_mode();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoordinateMode {
    Pixel,
    Cell,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MouseTarget {
    Canvas(Point),
    Ui { row: u16, column: u16 },
    None,
}

#[derive(Debug, Clone, Copy)]
pub struct MouseMapper {
    mode: CoordinateMode,
}

impl MouseMapper {
    fn new(mode: CoordinateMode) -> Self {
        Self { mode }
    }

    pub fn target(&mut self, event: MouseEvent, layout: Layout, captured: bool) -> MouseTarget {
        if u32::from(event.column) >= layout.columns || u32::from(event.row) >= layout.rows {
            self.mode = CoordinateMode::Pixel;
        }
        match self.mode {
            CoordinateMode::Cell => self.cell_target(event, layout, captured),
            CoordinateMode::Pixel => self.pixel_target(event, layout, captured),
        }
    }

    fn cell_target(self, event: MouseEvent, layout: Layout, captured: bool) -> MouseTarget {
        let column = u32::from(event.column);
        let row = u32::from(event.row);
        if row < layout.canvas_rows || captured {
            let x = if captured {
                column.min(layout.columns.saturating_sub(1))
            } else {
                column
            };
            let y = row.min(layout.canvas_rows.saturating_sub(1));
            return MouseTarget::Canvas(Point::new(
                (x as f32 + 0.5) / layout.columns.max(1) as f32,
                (y as f32 + 0.5) / layout.canvas_rows.max(1) as f32,
            ));
        }
        if row < layout.rows {
            MouseTarget::Ui {
                row: u16::try_from(row.saturating_sub(layout.canvas_rows)).unwrap_or(u16::MAX),
                column: event.column,
            }
        } else {
            MouseTarget::None
        }
    }

    fn pixel_target(self, event: MouseEvent, layout: Layout, captured: bool) -> MouseTarget {
        let x = u32::from(event.column);
        let y = u32::from(event.row);
        let canvas_top = layout.grid_origin_y;
        let canvas_bottom = canvas_top.saturating_add(layout.canvas_display_height());
        if (y >= canvas_top && y < canvas_bottom) || captured {
            let relative_x = x
                .saturating_sub(layout.grid_origin_x)
                .min(layout.canvas_display_width().saturating_sub(1));
            let relative_y = y
                .saturating_sub(canvas_top)
                .min(layout.canvas_display_height().saturating_sub(1));
            return MouseTarget::Canvas(Point::new(
                relative_x as f32 / layout.canvas_display_width().max(1) as f32,
                relative_y as f32 / layout.canvas_display_height().max(1) as f32,
            ));
        }
        let row = y
            .saturating_sub(layout.grid_origin_y)
            .checked_div(layout.cell_height.max(1))
            .unwrap_or(0);
        let column = x
            .saturating_sub(layout.grid_origin_x)
            .checked_div(layout.cell_width.max(1))
            .unwrap_or(0);
        if row >= layout.canvas_rows && row < layout.rows {
            MouseTarget::Ui {
                row: u16::try_from(row - layout.canvas_rows).unwrap_or(u16::MAX),
                column: u16::try_from(column).unwrap_or(u16::MAX),
            }
        } else {
            MouseTarget::None
        }
    }
}

/// Ask the terminal whether SGR pixel mode 1016 took effect, and read its answer.
///
/// The coordinate space belongs to the terminal that owns this PTY, so the environment cannot
/// answer for it: `vvssh` forwards a Vivid session into a remote shell but not `VIVIDO_WINDOW_ID`,
/// which names a window on the machine running Vivido. DECRQM asks the terminal itself and is
/// answered identically over a forwarded session.
///
/// A primary device attributes request follows the mode request. Every VT-compatible terminal
/// answers DA1, so a terminal that ignores the mode request still ends this read, and it answers
/// in order, so the mode reply cannot arrive after its attributes.
///
/// Nothing is asked unless the answer can be read: an unread reply would reach the event loop as
/// input the user never typed.
#[cfg(unix)]
fn detect_mouse_mode(output: &mut impl Write) -> CoordinateMode {
    // SAFETY: `isatty` only inspects the descriptor.
    if unsafe { libc::isatty(libc::STDIN_FILENO) } != 1 {
        return CoordinateMode::Cell;
    }
    if write!(output, "\x1b[?1016$p\x1b[c").is_err() || output.flush().is_err() {
        return CoordinateMode::Cell;
    }
    read_mouse_mode_reply()
}

/// Windows console input is not a pollable byte stream here, and a read that outlived the probe
/// would consume terminal input the event loop still needs, so the terminal is not asked at all.
/// Vivido sets `VIVIDO_WINDOW_ID` for the shells it starts, which covers a vvpaint running on the
/// same machine as its Vivido window.
#[cfg(not(unix))]
fn detect_mouse_mode(_output: &mut impl Write) -> CoordinateMode {
    if std::env::var_os("VIVIDO_WINDOW_ID").is_some() {
        CoordinateMode::Pixel
    } else {
        CoordinateMode::Cell
    }
}

/// Parameter and intermediate bytes of the longest reply worth keeping.
const PROBE_SEQUENCE_LIMIT: usize = 32;

/// Terminal answers travel the same path as the drawing itself, so this budget covers a forwarded
/// round trip. Expiring costs at most one mis-mapped report: `MouseMapper` corrects itself as soon
/// as a report lands outside the grid.
#[cfg(unix)]
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

#[cfg(unix)]
fn read_mouse_mode_reply() -> CoordinateMode {
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let mut scanner = ProbeScanner::default();
    let mut buffer = [0u8; 64];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let milliseconds = match i32::try_from(remaining.as_millis()) {
            Ok(0) | Err(_) => return CoordinateMode::Cell,
            Ok(milliseconds) => milliseconds,
        };
        let mut descriptor = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `descriptor` is one initialized `pollfd` and the count matches that single entry.
        if unsafe { libc::poll(&mut descriptor, 1, milliseconds) } <= 0 {
            return CoordinateMode::Cell;
        }
        // SAFETY: the terminal reported readable input, and the read is bounded by `buffer`.
        let count =
            unsafe { libc::read(libc::STDIN_FILENO, buffer.as_mut_ptr().cast(), buffer.len()) };
        let Ok(count) = usize::try_from(count) else {
            return CoordinateMode::Cell;
        };
        if count == 0 {
            return CoordinateMode::Cell;
        }
        for &byte in &buffer[..count] {
            if let Some(mode) = scanner.advance(byte) {
                return mode;
            }
        }
    }
}

/// Incremental scanner for the escape sequences that end the mouse-mode probe.
#[derive(Debug, Default)]
struct ProbeScanner {
    escape: bool,
    sequence: Option<Vec<u8>>,
}

impl ProbeScanner {
    fn advance(&mut self, byte: u8) -> Option<CoordinateMode> {
        match (&mut self.sequence, byte) {
            (Some(sequence), 0x40..=0x7e) => {
                let sequence = std::mem::take(sequence);
                self.sequence = None;
                classify_probe_reply(&sequence, byte)
            }
            (Some(sequence), _) => {
                if sequence.len() < PROBE_SEQUENCE_LIMIT {
                    sequence.push(byte);
                }
                None
            }
            (None, b'\x1b') => {
                self.escape = true;
                None
            }
            (None, b'[') if self.escape => {
                self.escape = false;
                self.sequence = Some(Vec::new());
                None
            }
            (None, _) => {
                self.escape = false;
                None
            }
        }
    }
}

/// Decide the coordinate space from one complete control sequence.
fn classify_probe_reply(sequence: &[u8], final_byte: u8) -> Option<CoordinateMode> {
    match final_byte {
        // DECRPM: CSI ? 1016 ; Ps $ y. Ps is 1 for set and 3 for permanently set; 0 reports a mode
        // the terminal does not recognize, and 2 or 4 report one it declined to set.
        b'y' => {
            let body = sequence.strip_prefix(b"?")?.strip_suffix(b"$")?;
            let (mode, state) =
                body.split_at_checked(body.iter().position(|byte| *byte == b';')?)?;
            (mode == b"1016").then_some(match state {
                b";1" | b";3" => CoordinateMode::Pixel,
                _ => CoordinateMode::Cell,
            })
        }
        // DA1 answers last, so the terminal never reported mode 1016.
        b'c' => Some(CoordinateMode::Cell),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyModifiers, MouseEventKind};

    fn layout() -> Layout {
        Layout {
            columns: 80,
            rows: 24,
            canvas_rows: 21,
            viewport_width: 800,
            viewport_height: 480,
            grid_origin_x: 0,
            grid_origin_y: 0,
            cell_width: 10,
            cell_height: 20,
            backing_width: 400,
            backing_height: 220,
        }
    }

    #[test]
    fn maps_cells_and_captured_outside_drags() {
        let mut mapper = MouseMapper {
            mode: CoordinateMode::Cell,
        };
        let event = MouseEvent {
            kind: MouseEventKind::Drag(crossterm::event::MouseButton::Left),
            column: 79,
            row: 23,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            mapper.target(event, layout(), true),
            MouseTarget::Canvas(Point::new(0.99375, 0.97619045))
        );
    }

    fn scan(bytes: &[u8]) -> Option<CoordinateMode> {
        let mut scanner = ProbeScanner::default();
        bytes.iter().find_map(|byte| scanner.advance(*byte))
    }

    #[test]
    fn a_reported_pixel_mode_selects_pixel_coordinates() {
        assert_eq!(scan(b"\x1b[?1016;1$y\x1b[?6c"), Some(CoordinateMode::Pixel));
    }

    /// A terminal that answers only its device attributes never enabled mode 1016.
    #[test]
    fn attributes_without_a_mode_report_select_cell_coordinates() {
        assert_eq!(scan(b"\x1b[?6c"), Some(CoordinateMode::Cell));
        assert_eq!(scan(b"\x1b[?1016;0$y"), Some(CoordinateMode::Cell));
        assert_eq!(scan(b"\x1b[?1016;2$y"), Some(CoordinateMode::Cell));
    }

    /// Mouse reports and keys can precede the answer; only the answer ends the probe.
    #[test]
    fn unrelated_input_does_not_end_the_probe() {
        assert_eq!(scan(b"\x1b[<35;10;20Max\x1b[A"), None);
        assert_eq!(
            scan(b"\x1b[<35;10;20M\x1b[?1016;1$y"),
            Some(CoordinateMode::Pixel)
        );
    }

    /// Another mode's report must not answer for 1016.
    #[test]
    fn a_report_for_a_different_mode_is_ignored() {
        assert_eq!(scan(b"\x1b[?1006;1$y"), None);
    }

    #[test]
    fn a_reply_split_across_reads_is_reassembled() {
        let mut scanner = ProbeScanner::default();
        assert!(
            b"\x1b[?1016;"
                .iter()
                .all(|byte| scanner.advance(*byte).is_none())
        );
        assert_eq!(scanner.advance(b'1'), None);
        assert_eq!(scanner.advance(b'$'), None);
        assert_eq!(scanner.advance(b'y'), Some(CoordinateMode::Pixel));
    }

    #[test]
    fn excludes_status_from_canvas() {
        let mut mapper = MouseMapper {
            mode: CoordinateMode::Cell,
        };
        let event = MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: 3,
            row: 21,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            mapper.target(event, layout(), false),
            MouseTarget::Ui { row: 0, column: 3 }
        );
    }
}
