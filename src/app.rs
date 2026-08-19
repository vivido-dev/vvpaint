use std::{
    io::{self, Write},
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use crossterm::{
    cursor::MoveTo,
    event::{
        self, Event as TerminalEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton,
        MouseEvent, MouseEventKind,
    },
    queue,
    style::{Color, Print, ResetColor, SetBackgroundColor},
    terminal::{Clear, ClearType},
};
use image::Rgba;

use crate::{
    args::{Args, default_export_size, default_output_path, resolve_format, validate_output},
    canvas::{BaseSource, DrawingCanvas, Point, Style, Tool, WidthPreset},
    export::{self, ExportFormat, ExportSize},
    terminal::{Layout, MouseMapper, MouseTarget, TerminalSession},
    theme,
    vivid::{self, VividHandle},
};

#[derive(Debug, Clone)]
struct State {
    tool: Tool,
    previous_tool: Tool,
    primary: Rgba<u8>,
    secondary: Rgba<u8>,
    width: WidthPreset,
    input: InputMode,
    message: String,
    format: ExportFormat,
    export_size: ExportSize,
}

#[derive(Debug, Clone)]
enum InputMode {
    None,
    Color {
        target: ColorTarget,
        buffer: String,
    },
    Text {
        position: Point,
        style: Style,
        buffer: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColorTarget {
    Primary,
    Secondary,
}

impl State {
    fn new(canvas: &DrawingCanvas, format: ExportFormat, export_size: ExportSize) -> Self {
        Self {
            tool: Tool::Freehand,
            previous_tool: Tool::Freehand,
            primary: canvas.default_primary(),
            secondary: canvas.default_secondary(),
            width: WidthPreset::Medium,
            input: InputMode::None,
            message: "Ready".into(),
            format,
            export_size,
        }
    }

    fn set_tool(&mut self, tool: Tool) {
        if tool != Tool::Picker {
            self.previous_tool = tool;
        } else if self.tool != Tool::Picker {
            self.previous_tool = self.tool;
        }
        self.tool = tool;
        self.message = format!("Tool: {}", tool_label(tool));
    }

    fn style(&self, button: MouseButton) -> Style {
        let color = match self.tool {
            Tool::Redaction => Rgba([0, 0, 0, 255]),
            Tool::Eraser => self.secondary,
            _ if button == MouseButton::Right => self.secondary,
            _ => self.primary,
        };
        if self.tool == Tool::Highlighter {
            Style::highlighter(color, self.width)
        } else {
            Style::opaque(color, self.width)
        }
    }

    fn set_color(&mut self, target: ColorTarget, color: Rgba<u8>, label: &str) {
        match target {
            ColorTarget::Primary => self.primary = color,
            ColorTarget::Secondary => self.secondary = color,
        }
        self.input = InputMode::None;
        self.message = format!(
            "{} color: {label}",
            if target == ColorTarget::Primary {
                "Primary"
            } else {
                "Secondary"
            }
        );
    }
}

#[derive(Debug, Clone, Copy)]
struct PaletteColor {
    name: &'static str,
    color: Rgba<u8>,
}
const PALETTE: [PaletteColor; 9] = [
    PaletteColor {
        name: "black",
        color: Rgba([0, 0, 0, 255]),
    },
    PaletteColor {
        name: "white",
        color: Rgba([255, 255, 255, 255]),
    },
    PaletteColor {
        name: "red",
        color: Rgba([255, 0, 0, 255]),
    },
    PaletteColor {
        name: "orange",
        color: Rgba([255, 128, 0, 255]),
    },
    PaletteColor {
        name: "yellow",
        color: Rgba([255, 221, 0, 255]),
    },
    PaletteColor {
        name: "green",
        color: Rgba([0, 180, 80, 255]),
    },
    PaletteColor {
        name: "cyan",
        color: Rgba([0, 190, 220, 255]),
    },
    PaletteColor {
        name: "blue",
        color: Rgba([30, 100, 255, 255]),
    },
    PaletteColor {
        name: "purple",
        color: Rgba([160, 80, 220, 255]),
    },
];

pub fn run(args: Args) -> Result<()> {
    let format = resolve_format(args.format, args.output.as_deref())?;
    let output = args
        .output
        .clone()
        .unwrap_or_else(|| default_output_path(args.input_image.as_deref(), format));
    validate_output(&output)?;
    let export_size = args
        .export_size
        .unwrap_or_else(|| default_export_size(args.input_image.as_deref()));
    let source = load_source(args.input_image.as_ref())?;
    let theme = theme::resolve(args.theme);
    let handle = VividHandle::spawn(vivid::producer_config(), args.resolution_scale)?;
    let mut layout = handle.wait_ready()?;
    let mut canvas = DrawingCanvas::new(layout.backing_width, layout.backing_height, source, theme);
    let mut state = State::new(&canvas, format, export_size);
    let terminal = TerminalSession::enter()?;
    handle.publish(canvas.render())?;
    handle.wait_presented()?;
    let result = event_loop(&handle, &terminal, &mut canvas, &mut state, &mut layout);
    canvas.cancel();
    handle.shutdown();
    drop(terminal);
    result?;
    export::save(&output, format, export_size, &canvas)?;
    println!("Saved {}", output.display());
    Ok(())
}

fn load_source(input: Option<&PathBuf>) -> Result<BaseSource> {
    match input {
        Some(path) => {
            if !path.is_file() {
                return Err(anyhow!("input image not found: {}", path.display()));
            }
            Ok(BaseSource::Image(image::open(path).with_context(|| {
                format!("failed to load image {}", path.display())
            })?))
        }
        None => Ok(BaseSource::Blank),
    }
}

fn event_loop(
    handle: &VividHandle,
    terminal: &TerminalSession,
    canvas: &mut DrawingCanvas,
    state: &mut State,
    layout: &mut Layout,
) -> Result<()> {
    let mut output = io::stdout().lock();
    let mut mapper = terminal.mouse_mapper();
    let mut captured: Option<MouseButton> = None;
    render_ui(&mut output, state, canvas, *layout)?;
    loop {
        while let Some(event) = handle.try_event() {
            match event {
                vivid::Event::Resized(next) => {
                    *layout = next;
                    canvas.resize(next.backing_width, next.backing_height);
                    mapper = terminal.mouse_mapper();
                    handle.publish(canvas.render())?;
                    render_ui(&mut output, state, canvas, *layout)?;
                }
                vivid::Event::Error(error) => return Err(anyhow!(error)),
                vivid::Event::Closed => return Err(anyhow!("Vivid presenter connection closed")),
                vivid::Event::Ready(_) | vivid::Event::Presented => {}
            }
        }
        if !event::poll(Duration::from_millis(25))? {
            continue;
        }
        let mut redraw = false;
        loop {
            match event::read()? {
                TerminalEvent::Key(key)
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
                    let action = handle_key(key, canvas, state);
                    if action.quit {
                        return Ok(());
                    }
                    redraw |= action.redraw;
                }
                TerminalEvent::Mouse(mouse) => {
                    redraw |=
                        handle_mouse(mouse, canvas, state, *layout, &mut mapper, &mut captured)
                }
                TerminalEvent::Resize(_, _) => {}
                _ => {}
            }
            if !event::poll(Duration::ZERO)? {
                break;
            }
        }
        if redraw {
            handle.publish(canvas.render())?;
            render_ui(&mut output, state, canvas, *layout)?;
        }
    }
}

#[derive(Default)]
struct KeyAction {
    quit: bool,
    redraw: bool,
}

fn handle_key(key: KeyEvent, canvas: &mut DrawingCanvas, state: &mut State) -> KeyAction {
    if matches!(key.code, KeyCode::Char('c')) && key.modifiers.contains(KeyModifiers::CONTROL) {
        return KeyAction {
            quit: true,
            redraw: false,
        };
    }
    match &mut state.input {
        InputMode::Color { target, buffer } => match key.code {
            KeyCode::Esc => {
                state.input = InputMode::None;
                state.message = "Color unchanged".into();
            }
            KeyCode::Enter => {
                let target = *target;
                let value = buffer.clone();
                if let Some(color) = parse_color(&value) {
                    state.set_color(target, color, &color_hex(color));
                } else {
                    state.message = format!("Unknown color: {}", value.trim());
                }
            }
            KeyCode::Backspace => {
                buffer.pop();
            }
            KeyCode::Char(character)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                buffer.push(character)
            }
            _ => {}
        },
        InputMode::Text {
            position,
            style,
            buffer,
        } => match key.code {
            KeyCode::Esc => {
                state.input = InputMode::None;
                state.message = "Text canceled".into();
            }
            KeyCode::Enter => {
                let position = *position;
                let style = *style;
                let text = buffer.clone();
                state.input = InputMode::None;
                if canvas.add_text(position, text, style) {
                    state.message = "Text added".into();
                    return KeyAction {
                        quit: false,
                        redraw: true,
                    };
                }
                state.message = "Text skipped".into();
            }
            KeyCode::Backspace => {
                buffer.pop();
            }
            KeyCode::Char(character)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                buffer.push(character)
            }
            _ => {}
        },
        InputMode::None => {
            let tool = match key.code {
                KeyCode::Char('f') => Some(Tool::Freehand),
                KeyCode::Char('l') => Some(Tool::Line),
                KeyCode::Char('r') => Some(Tool::Rectangle),
                KeyCode::Char('e') => Some(Tool::Ellipse),
                KeyCode::Char('a') => Some(Tool::Arrow),
                KeyCode::Char('t') => Some(Tool::Text),
                KeyCode::Char('h') => Some(Tool::Highlighter),
                KeyCode::Char('x') => Some(Tool::Redaction),
                KeyCode::Char('d') => Some(Tool::Eraser),
                KeyCode::Char('g') => Some(Tool::Fill),
                KeyCode::Char('p') => Some(Tool::Picker),
                _ => None,
            };
            if let Some(tool) = tool {
                canvas.cancel();
                state.set_tool(tool);
                return KeyAction {
                    quit: false,
                    redraw: true,
                };
            }
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    return KeyAction {
                        quit: true,
                        redraw: false,
                    };
                }
                KeyCode::Char('z') => {
                    canvas.undo();
                    return KeyAction {
                        quit: false,
                        redraw: true,
                    };
                }
                KeyCode::Char('y') => {
                    canvas.redo();
                    return KeyAction {
                        quit: false,
                        redraw: true,
                    };
                }
                KeyCode::Char('C') => {
                    if canvas.clear() {
                        state.message = "Drawing layer cleared".into();
                    }
                    return KeyAction {
                        quit: false,
                        redraw: true,
                    };
                }
                KeyCode::Char('[') => {
                    state.width = state.width.previous();
                    state.message = format!("Size: {}", state.width.label());
                }
                KeyCode::Char(']') => {
                    state.width = state.width.next();
                    state.message = format!("Size: {}", state.width.label());
                }
                KeyCode::Char('c') => {
                    state.input = InputMode::Color {
                        target: ColorTarget::Primary,
                        buffer: String::new(),
                    };
                    state.message = "Enter primary color".into();
                }
                KeyCode::Char('b') => {
                    state.input = InputMode::Color {
                        target: ColorTarget::Secondary,
                        buffer: String::new(),
                    };
                    state.message = "Enter secondary color".into();
                }
                _ => {}
            }
        }
    }
    KeyAction {
        quit: false,
        redraw: true,
    }
}

fn handle_mouse(
    mouse: MouseEvent,
    canvas: &mut DrawingCanvas,
    state: &mut State,
    layout: Layout,
    mapper: &mut MouseMapper,
    captured: &mut Option<MouseButton>,
) -> bool {
    if !matches!(state.input, InputMode::None) {
        return false;
    }
    match mouse.kind {
        MouseEventKind::Down(button @ (MouseButton::Left | MouseButton::Right)) => {
            match mapper.target(mouse, layout, false) {
                MouseTarget::Canvas(point) => {
                    *captured = Some(button);
                    match state.tool {
                        Tool::Text => {
                            state.input = InputMode::Text {
                                position: point,
                                style: state.style(button),
                                buffer: String::new(),
                            };
                            state.message = "Enter text".into();
                        }
                        Tool::Fill => {
                            canvas.fill(point, state.style(button).color);
                            *captured = None;
                        }
                        Tool::Picker => {
                            let color = canvas.color_at(point);
                            let target = if button == MouseButton::Right {
                                ColorTarget::Secondary
                            } else {
                                ColorTarget::Primary
                            };
                            state.set_color(target, color, &color_hex(color));
                            state.tool = state.previous_tool;
                            *captured = None;
                        }
                        _ => {
                            canvas.begin(state.tool, point, state.style(button));
                        }
                    }
                    true
                }
                MouseTarget::Status(column) => {
                    if let Some(palette) = palette_at(column, state, canvas, layout.columns) {
                        let target = if button == MouseButton::Right {
                            ColorTarget::Secondary
                        } else {
                            ColorTarget::Primary
                        };
                        state.set_color(target, palette.color, palette.name);
                        true
                    } else {
                        false
                    }
                }
                MouseTarget::Input | MouseTarget::None => false,
            }
        }
        MouseEventKind::Drag(button) if *captured == Some(button) => {
            if let MouseTarget::Canvas(point) = mapper.target(mouse, layout, true) {
                canvas.extend(point)
            } else {
                false
            }
        }
        MouseEventKind::Up(button) if *captured == Some(button) => {
            if let MouseTarget::Canvas(point) = mapper.target(mouse, layout, true) {
                canvas.extend(point);
            }
            *captured = None;
            canvas.finish()
        }
        _ => false,
    }
}

fn render_ui<W: Write>(
    output: &mut W,
    state: &State,
    canvas: &DrawingCanvas,
    layout: Layout,
) -> Result<()> {
    if let Some(row) = layout.status_row() {
        queue!(output, MoveTo(0, row), Clear(ClearType::CurrentLine))?;
        write_status(output, state, canvas, layout.columns)?;
    }
    if let Some(row) = layout.input_row() {
        queue!(
            output,
            MoveTo(0, row),
            Clear(ClearType::CurrentLine),
            Print(truncate(&input_text(state), layout.columns))
        )?;
    }
    output.flush()?;
    Ok(())
}

fn write_status<W: Write>(
    output: &mut W,
    state: &State,
    canvas: &DrawingCanvas,
    columns: u32,
) -> Result<()> {
    let prefix = status_prefix(state, canvas);
    let mut used = prefix.chars().count() as u32;
    queue!(output, Print(truncate(&prefix, columns)))?;
    for palette in PALETTE {
        let width = palette_width(palette);
        if used + width > columns {
            break;
        }
        queue!(
            output,
            Print(" "),
            SetBackgroundColor(terminal_color(palette.color)),
            Print("  "),
            ResetColor,
            Print(format!(" {}", palette.name))
        )?;
        used += width;
    }
    queue!(output, ResetColor)?;
    Ok(())
}

fn status_prefix(state: &State, canvas: &DrawingCanvas) -> String {
    let (width, height) = canvas.dimensions();
    format!(
        "{}:{} | Size {} | P {} S {} | {}x{} | {} {} | {} | Palette",
        tool_shortcut(state.tool),
        tool_label(state.tool),
        state.width.label(),
        color_hex(state.primary),
        color_hex(state.secondary),
        width,
        height,
        state.format,
        state.export_size,
        if canvas.is_dirty() {
            "modified"
        } else {
            "clean"
        }
    )
}

fn input_text(state: &State) -> String {
    match &state.input {
        InputMode::Color { target, buffer } => format!(
            "{} color> {buffer}  Enter apply, Esc cancel",
            if *target == ColorTarget::Primary {
                "Primary"
            } else {
                "Secondary"
            }
        ),
        InputMode::Text { buffer, .. } => format!("Text> {buffer}  Enter apply, Esc cancel"),
        InputMode::None => format!(
            "{} | f free l line r rect e ellipse a arrow t text h highlight x redact d erase g fill p pick [ ] size c/b colors C clear z/y undo/redo q save",
            state.message
        ),
    }
}

fn palette_at(
    column: u16,
    state: &State,
    canvas: &DrawingCanvas,
    columns: u32,
) -> Option<PaletteColor> {
    let mut start = status_prefix(state, canvas).chars().count() as u16;
    for palette in PALETTE {
        let width = palette_width(palette) as u16;
        if u32::from(start) + u32::from(width) > columns {
            return None;
        }
        if column >= start && column < start + width {
            return Some(palette);
        }
        start += width;
    }
    None
}
fn palette_width(value: PaletteColor) -> u32 {
    value.name.len() as u32 + 4
}
fn truncate(value: &str, columns: u32) -> String {
    value.chars().take(columns as usize).collect()
}
fn terminal_color(value: Rgba<u8>) -> Color {
    Color::Rgb {
        r: value[0],
        g: value[1],
        b: value[2],
    }
}
fn color_hex(value: Rgba<u8>) -> String {
    format!("#{:02x}{:02x}{:02x}", value[0], value[1], value[2])
}

fn parse_color(value: &str) -> Option<Rgba<u8>> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        return match hex.len() {
            3 => {
                let mut chars = hex.chars();
                let r = chars.next()?.to_digit(16)? as u8;
                let g = chars.next()?.to_digit(16)? as u8;
                let b = chars.next()?.to_digit(16)? as u8;
                Some(Rgba([r * 17, g * 17, b * 17, 255]))
            }
            6 => Some(Rgba([
                u8::from_str_radix(&hex[0..2], 16).ok()?,
                u8::from_str_radix(&hex[2..4], 16).ok()?,
                u8::from_str_radix(&hex[4..6], 16).ok()?,
                255,
            ])),
            _ => None,
        };
    }
    PALETTE
        .iter()
        .find(|color| color.name == value.to_ascii_lowercase())
        .map(|color| color.color)
        .or_else(|| match value.to_ascii_lowercase().as_str() {
            "pink" => Some(Rgba([255, 96, 170, 255])),
            "magenta" => Some(Rgba([220, 0, 220, 255])),
            "gray" | "grey" => Some(Rgba([128, 128, 128, 255])),
            _ => None,
        })
}

fn tool_label(tool: Tool) -> &'static str {
    match tool {
        Tool::Freehand => "freehand",
        Tool::Line => "line",
        Tool::Rectangle => "rectangle",
        Tool::Ellipse => "ellipse",
        Tool::Arrow => "arrow",
        Tool::Text => "text",
        Tool::Highlighter => "highlight",
        Tool::Redaction => "redact",
        Tool::Eraser => "eraser",
        Tool::Fill => "fill",
        Tool::Picker => "picker",
    }
}
fn tool_shortcut(tool: Tool) -> char {
    match tool {
        Tool::Freehand => 'f',
        Tool::Line => 'l',
        Tool::Rectangle => 'r',
        Tool::Ellipse => 'e',
        Tool::Arrow => 'a',
        Tool::Text => 't',
        Tool::Highlighter => 'h',
        Tool::Redaction => 'x',
        Tool::Eraser => 'd',
        Tool::Fill => 'g',
        Tool::Picker => 'p',
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    fn state() -> (DrawingCanvas, State) {
        let canvas = DrawingCanvas::blank(100, 50, Theme::Light);
        let state = State::new(&canvas, ExportFormat::Png, ExportSize::Canvas);
        (canvas, state)
    }

    #[test]
    fn custom_colors_and_shortcuts_work() {
        assert_eq!(parse_color("#0f0"), Some(Rgba([0, 255, 0, 255])));
        assert_eq!(parse_color("blue"), Some(Rgba([30, 100, 255, 255])));
        let (mut canvas, mut state) = state();
        handle_key(
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
            &mut canvas,
            &mut state,
        );
        assert_eq!(state.tool, Tool::Fill);
        handle_key(
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
            &mut canvas,
            &mut state,
        );
    }

    #[test]
    fn picker_changes_color_without_history() {
        let (canvas, mut state) = state();
        state.set_tool(Tool::Picker);
        let color = canvas.color_at(Point::new(0.5, 0.5));
        state.set_color(ColorTarget::Primary, color, "picked");
        state.tool = state.previous_tool;
        assert_eq!(state.tool, Tool::Freehand);
        assert!(!canvas.is_dirty());
    }
}
