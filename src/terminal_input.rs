#[cfg(unix)]
use std::{collections::VecDeque, ffi::OsStr};
use std::{io, time::Duration};

use crossterm::event::{self, Event};
#[cfg(any(unix, test))]
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

#[cfg(unix)]
pub fn is_remote() -> bool {
    std::env::var_os("VIVID_REMOTE").is_some_and(|value| value != OsStr::new("0"))
}

pub struct InputReader {
    #[cfg(unix)]
    remote: Option<RemoteInput>,
}

impl InputReader {
    pub fn new() -> Self {
        Self {
            #[cfg(unix)]
            remote: is_remote().then(RemoteInput::default),
        }
    }

    pub fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
        #[cfg(unix)]
        if let Some(remote) = &mut self.remote {
            return remote.poll(timeout);
        }
        event::poll(timeout)
    }

    pub fn read(&mut self) -> io::Result<Event> {
        #[cfg(unix)]
        if let Some(remote) = &mut self.remote {
            return remote.read();
        }
        event::read()
    }
}

#[cfg(unix)]
#[derive(Default)]
struct RemoteInput {
    decoder: Decoder,
    events: VecDeque<Event>,
}

#[cfg(unix)]
impl RemoteInput {
    fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
        if self.refill_events() {
            return Ok(true);
        }

        let deadline = std::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if !poll_stdin(remaining)? {
                return Ok(false);
            }
            self.read_stdin()?;
            if self.refill_events() {
                return Ok(true);
            }
            if std::time::Instant::now() >= deadline {
                return Ok(false);
            }
        }
    }

    fn read(&mut self) -> io::Result<Event> {
        loop {
            if self.refill_events() {
                return self.events.pop_front().ok_or_else(|| {
                    io::Error::other("remote terminal input queue became unexpectedly empty")
                });
            }
            if !poll_stdin(Duration::MAX)? {
                continue;
            }
            self.read_stdin()?;
        }
    }

    fn refill_events(&mut self) -> bool {
        self.events.extend(self.decoder.drain());
        !self.events.is_empty()
    }

    fn read_stdin(&mut self) -> io::Result<()> {
        let mut bytes = [0_u8; 4096];
        // SAFETY: `bytes` is writable for exactly its declared length and stdin remains owned by
        // the process. The terminal is in raw mode, so no text conversion occurs here.
        let count =
            unsafe { libc::read(libc::STDIN_FILENO, bytes.as_mut_ptr().cast(), bytes.len()) };
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "remote terminal input closed",
            ));
        }
        let count = usize::try_from(count).map_err(|_| io::Error::last_os_error())?;
        self.decoder.push(&bytes[..count]);
        Ok(())
    }
}

#[cfg(unix)]
fn poll_stdin(timeout: Duration) -> io::Result<bool> {
    let milliseconds = if timeout == Duration::MAX {
        -1
    } else {
        i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX)
    };
    let mut descriptor = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: `descriptor` is one initialized `pollfd` and the count describes that one value.
    let result = unsafe { libc::poll(&mut descriptor, 1, milliseconds) };
    if result < 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            return Ok(false);
        }
        return Err(error);
    }
    Ok(result > 0)
}

#[cfg(any(unix, test))]
#[derive(Debug, Default)]
struct Decoder {
    bytes: Vec<u8>,
}

#[cfg(any(unix, test))]
impl Decoder {
    fn push(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    fn drain(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        loop {
            match decode_one(&self.bytes) {
                Decoded::Incomplete => break,
                Decoded::Consumed { count, event } => {
                    self.bytes.drain(..count);
                    if let Some(event) = event {
                        events.push(event);
                    }
                }
            }
        }
        events
    }
}

#[cfg(any(unix, test))]
enum Decoded {
    Incomplete,
    Consumed { count: usize, event: Option<Event> },
}

#[cfg(any(unix, test))]
fn decode_one(bytes: &[u8]) -> Decoded {
    let Some(&first) = bytes.first() else {
        return Decoded::Incomplete;
    };
    if first == b'\x1b' {
        if bytes.len() < 2 {
            return Decoded::Incomplete;
        }
        if bytes[1] != b'[' {
            return Decoded::Consumed {
                count: 1,
                event: None,
            };
        }
        let Some(final_index) = bytes[2..]
            .iter()
            .position(|byte| (0x40..=0x7e).contains(byte))
            .map(|index| index + 2)
        else {
            if bytes.len() > 128 {
                return Decoded::Consumed {
                    count: 1,
                    event: None,
                };
            }
            return Decoded::Incomplete;
        };
        let sequence = &bytes[..=final_index];
        let event = if sequence.starts_with(b"\x1b[<") {
            decode_sgr_mouse(sequence).map(Event::Mouse)
        } else if sequence.ends_with(b"u") {
            decode_csi_key(sequence).map(Event::Key)
        } else {
            None
        };
        return Decoded::Consumed {
            count: sequence.len(),
            event,
        };
    }

    let (count, event) = match first {
        b'\r' => (1, Some(Event::Key(KeyCode::Enter.into()))),
        b'\t' => (1, Some(Event::Key(KeyCode::Tab.into()))),
        b'\x7f' => (1, Some(Event::Key(KeyCode::Backspace.into()))),
        byte @ b'\x01'..=b'\x1a' => (
            1,
            Some(Event::Key(KeyEvent::new(
                KeyCode::Char(char::from(byte - 1 + b'a')),
                KeyModifiers::CONTROL,
            ))),
        ),
        _ => match decode_utf8(bytes) {
            Some((count, character)) => {
                let modifiers = if character.is_uppercase() {
                    KeyModifiers::SHIFT
                } else {
                    KeyModifiers::NONE
                };
                (
                    count,
                    Some(Event::Key(KeyEvent::new(
                        KeyCode::Char(character),
                        modifiers,
                    ))),
                )
            }
            None => return Decoded::Incomplete,
        },
    };
    Decoded::Consumed { count, event }
}

#[cfg(any(unix, test))]
fn decode_utf8(bytes: &[u8]) -> Option<(usize, char)> {
    let width = match *bytes.first()? {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return Some((1, '\u{fffd}')),
    };
    if bytes.len() < width {
        return None;
    }
    let character = std::str::from_utf8(&bytes[..width])
        .ok()
        .and_then(|text| text.chars().next())
        .unwrap_or('\u{fffd}');
    Some((width, character))
}

#[cfg(any(unix, test))]
fn decode_sgr_mouse(sequence: &[u8]) -> Option<MouseEvent> {
    let final_byte = *sequence.last()?;
    if !matches!(final_byte, b'M' | b'm') {
        return None;
    }
    let body = std::str::from_utf8(&sequence[3..sequence.len() - 1]).ok()?;
    let mut fields = body.split(';');
    let code = fields.next()?.parse::<u8>().ok()?;
    let column = fields.next()?.parse::<u16>().ok()?.checked_sub(1)?;
    let row = fields.next()?.parse::<u16>().ok()?.checked_sub(1)?;
    if fields.next().is_some() {
        return None;
    }

    let button_number = (code & 0b0000_0011) | ((code & 0b1100_0000) >> 4);
    let dragging = code & 0b0010_0000 != 0;
    let mut kind = match (button_number, dragging) {
        (0, false) => MouseEventKind::Down(MouseButton::Left),
        (1, false) => MouseEventKind::Down(MouseButton::Middle),
        (2, false) => MouseEventKind::Down(MouseButton::Right),
        (0, true) => MouseEventKind::Drag(MouseButton::Left),
        (1, true) => MouseEventKind::Drag(MouseButton::Middle),
        (2, true) => MouseEventKind::Drag(MouseButton::Right),
        (3, false) => MouseEventKind::Up(MouseButton::Left),
        (3..=5, true) => MouseEventKind::Moved,
        (4, false) => MouseEventKind::ScrollUp,
        (5, false) => MouseEventKind::ScrollDown,
        (6, false) => MouseEventKind::ScrollLeft,
        (7, false) => MouseEventKind::ScrollRight,
        _ => return None,
    };
    if final_byte == b'm'
        && let MouseEventKind::Down(button) = kind
    {
        kind = MouseEventKind::Up(button);
    }

    let mut modifiers = KeyModifiers::NONE;
    modifiers.set(KeyModifiers::SHIFT, code & 4 != 0);
    modifiers.set(KeyModifiers::ALT, code & 8 != 0);
    modifiers.set(KeyModifiers::CONTROL, code & 16 != 0);
    Some(MouseEvent {
        kind,
        column,
        row,
        modifiers,
    })
}

#[cfg(any(unix, test))]
fn decode_csi_key(sequence: &[u8]) -> Option<KeyEvent> {
    let body = std::str::from_utf8(&sequence[2..sequence.len() - 1]).ok()?;
    let mut fields = body.split(';');
    let codepoint = fields.next()?.split(':').next()?.parse::<u32>().ok()?;
    let modifier_field = fields.next().unwrap_or("1");
    let modifier_number = modifier_field.split(':').next()?.parse::<u8>().ok()?;
    let modifier_mask = modifier_number.checked_sub(1)?;
    let mut modifiers = KeyModifiers::NONE;
    modifiers.set(KeyModifiers::SHIFT, modifier_mask & 1 != 0);
    modifiers.set(KeyModifiers::ALT, modifier_mask & 2 != 0);
    modifiers.set(KeyModifiers::CONTROL, modifier_mask & 4 != 0);
    modifiers.set(KeyModifiers::SUPER, modifier_mask & 8 != 0);
    let code = match char::from_u32(codepoint)? {
        '\u{1b}' => KeyCode::Esc,
        '\r' => KeyCode::Enter,
        '\t' => KeyCode::Tab,
        '\u{7f}' => KeyCode::Backspace,
        character => KeyCode::Char(character),
    };
    Some(KeyEvent::new(code, modifiers))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_mouse_reports_are_retained_until_complete() {
        let input = b"\x1b[<0;11;21M\x1b[<32;12;22M\x1b[<0;13;23m";
        for split in 1..input.len() {
            let mut decoder = Decoder::default();
            decoder.push(&input[..split]);
            let mut events = decoder.drain();
            decoder.push(&input[split..]);
            events.extend(decoder.drain());

            assert_eq!(events.len(), 3, "split at byte {split}");
            assert!(matches!(
                events[0],
                Event::Mouse(MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    ..
                })
            ));
            assert!(matches!(
                events[1],
                Event::Mouse(MouseEvent {
                    kind: MouseEventKind::Drag(MouseButton::Left),
                    ..
                })
            ));
            assert!(matches!(
                events[2],
                Event::Mouse(MouseEvent {
                    kind: MouseEventKind::Up(MouseButton::Left),
                    ..
                })
            ));
        }
    }

    #[test]
    fn disambiguated_escape_and_modified_keys_decode() {
        let mut decoder = Decoder::default();
        decoder.push(b"\x1b[27u\x1b[99;5u");
        let events = decoder.drain();
        assert!(matches!(
            events[0],
            Event::Key(KeyEvent {
                code: KeyCode::Esc,
                ..
            })
        ));
        assert!(matches!(
            events[1],
            Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, .. })
                if modifiers == KeyModifiers::CONTROL
        ));
    }
}
