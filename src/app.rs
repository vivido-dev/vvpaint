use std::{
    io::{self, Write},
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use crossterm::{
    cursor::{MoveTo, MoveToColumn},
    event::{
        Event as TerminalEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton,
        MouseEvent, MouseEventKind,
    },
    queue,
    style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal::{Clear, ClearType},
};
use image::Rgba;

use crate::{
    args::{Args, default_export_size, default_output_path, resolve_format, validate_output},
    canvas::{BaseSource, DrawingCanvas, Point, Style, Tool, WidthPreset},
    export::{self, ExportFormat, ExportSize},
    terminal::{Layout, MouseMapper, MouseTarget, TerminalSession},
    terminal_input::InputReader,
    theme,
    vivid::{self, VividHandle},
};

#[derive(Debug, Clone)]
struct State {
    tool: Tool,
    previous_tool: Tool,
    primary: Rgba<u8>,
    secondary: Rgba<u8>,
    widths: [WidthPreset; Tool::COUNT],
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
            tool: Tool::Pencil,
            previous_tool: Tool::Pencil,
            primary: canvas.default_primary(),
            secondary: canvas.default_secondary(),
            widths: [WidthPreset::Medium; Tool::COUNT],
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
            Tool::Eraser => self.secondary,
            _ if button == MouseButton::Right => self.secondary,
            _ => self.primary,
        };
        if self.tool == Tool::Highlighter {
            Style::highlighter(color, self.width())
        } else {
            Style::opaque(color, self.width())
        }
    }

    fn width(&self) -> WidthPreset {
        self.widths[self.tool.index()]
    }

    fn set_width(&mut self, width: WidthPreset) {
        if self.tool.supports_width() {
            self.widths[self.tool.index()] = width;
            self.message = format!("{} width: {}", tool_label(self.tool), width.label());
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
const PALETTE: [PaletteColor; 28] = [
    PaletteColor {
        name: "black",
        color: Rgba([0, 0, 0, 255]),
    },
    PaletteColor {
        name: "gray",
        color: Rgba([128, 128, 128, 255]),
    },
    PaletteColor {
        name: "dark red",
        color: Rgba([128, 0, 0, 255]),
    },
    PaletteColor {
        name: "olive",
        color: Rgba([128, 128, 0, 255]),
    },
    PaletteColor {
        name: "dark green",
        color: Rgba([0, 128, 0, 255]),
    },
    PaletteColor {
        name: "teal",
        color: Rgba([0, 128, 128, 255]),
    },
    PaletteColor {
        name: "navy",
        color: Rgba([0, 0, 128, 255]),
    },
    PaletteColor {
        name: "purple",
        color: Rgba([128, 0, 128, 255]),
    },
    PaletteColor {
        name: "khaki",
        color: Rgba([128, 128, 64, 255]),
    },
    PaletteColor {
        name: "dark teal",
        color: Rgba([0, 64, 64, 255]),
    },
    PaletteColor {
        name: "azure",
        color: Rgba([0, 128, 255, 255]),
    },
    PaletteColor {
        name: "dark azure",
        color: Rgba([0, 64, 128, 255]),
    },
    PaletteColor {
        name: "violet",
        color: Rgba([128, 0, 255, 255]),
    },
    PaletteColor {
        name: "brown",
        color: Rgba([128, 64, 0, 255]),
    },
    PaletteColor {
        name: "white",
        color: Rgba([255, 255, 255, 255]),
    },
    PaletteColor {
        name: "silver",
        color: Rgba([192, 192, 192, 255]),
    },
    PaletteColor {
        name: "red",
        color: Rgba([255, 0, 0, 255]),
    },
    PaletteColor {
        name: "yellow",
        color: Rgba([255, 255, 0, 255]),
    },
    PaletteColor {
        name: "green",
        color: Rgba([0, 255, 0, 255]),
    },
    PaletteColor {
        name: "cyan",
        color: Rgba([0, 255, 255, 255]),
    },
    PaletteColor {
        name: "blue",
        color: Rgba([0, 0, 255, 255]),
    },
    PaletteColor {
        name: "magenta",
        color: Rgba([255, 0, 255, 255]),
    },
    PaletteColor {
        name: "light yellow",
        color: Rgba([255, 255, 128, 255]),
    },
    PaletteColor {
        name: "spring green",
        color: Rgba([0, 255, 128, 255]),
    },
    PaletteColor {
        name: "light cyan",
        color: Rgba([128, 255, 255, 255]),
    },
    PaletteColor {
        name: "periwinkle",
        color: Rgba([128, 128, 255, 255]),
    },
    PaletteColor {
        name: "pink",
        color: Rgba([255, 0, 128, 255]),
    },
    PaletteColor {
        name: "orange",
        color: Rgba([255, 128, 64, 255]),
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
    let mut input = InputReader::new();
    render_ui(&mut output, state, canvas, *layout, false)?;
    loop {
        while let Some(event) = handle.try_event() {
            match event {
                vivid::Event::Resized(next) => {
                    *layout = next;
                    canvas.resize(next.backing_width, next.backing_height);
                    mapper = terminal.mouse_mapper();
                    handle.publish(canvas.render())?;
                    render_ui(&mut output, state, canvas, *layout, true)?;
                }
                vivid::Event::Error(error) => return Err(anyhow!(error)),
                vivid::Event::Closed => return Err(anyhow!("Vivid presenter connection closed")),
                vivid::Event::Ready(_) | vivid::Event::Presented => {}
            }
        }
        if !input.poll(Duration::from_millis(25))? {
            continue;
        }
        let mut redraw = false;
        loop {
            match input.read()? {
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
            if !input.poll(Duration::ZERO)? {
                break;
            }
        }
        if redraw {
            handle.publish(canvas.render())?;
            render_ui(&mut output, state, canvas, *layout, false)?;
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
                KeyCode::Char('e') => Some(Tool::Eraser),
                KeyCode::Char('f') => Some(Tool::Fill),
                KeyCode::Char('i') => Some(Tool::Picker),
                KeyCode::Char('p') => Some(Tool::Pencil),
                KeyCode::Char('b') => Some(Tool::Brush),
                KeyCode::Char('a') => Some(Tool::Airbrush),
                KeyCode::Char('t') => Some(Tool::Text),
                KeyCode::Char('l') => Some(Tool::Line),
                KeyCode::Char('r') => Some(Tool::Rectangle),
                KeyCode::Char('o') => Some(Tool::Ellipse),
                KeyCode::Char('u') => Some(Tool::RoundedRectangle),
                KeyCode::Char('h') => Some(Tool::Highlighter),
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
                    state.set_width(state.width().previous());
                }
                KeyCode::Char(']') => {
                    state.set_width(state.width().next());
                }
                KeyCode::Char('c') => {
                    state.input = InputMode::Color {
                        target: ColorTarget::Primary,
                        buffer: String::new(),
                    };
                    state.message = "Enter primary color".into();
                }
                KeyCode::Char('v') => {
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
                MouseTarget::Ui { row, column } => {
                    handle_toolbar_click(row, column, button, canvas, state, layout.columns)
                }
                MouseTarget::None => false,
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
    clear_screen: bool,
) -> Result<()> {
    if clear_screen {
        queue!(output, Clear(ClearType::All))?;
    }
    for index in 0..2 {
        if let Some(row) = layout.toolbar_row(index) {
            queue!(output, MoveTo(0, row), Clear(ClearType::CurrentLine))?;
            write_toolbar_row(output, state, layout.columns, index as u16)?;
        }
    }
    if let Some(row) = layout.message_row() {
        queue!(
            output,
            MoveTo(0, row),
            Clear(ClearType::CurrentLine),
            Print(truncate(&message_text(state, canvas), layout.columns))
        )?;
    }
    output.flush()?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolbarControl {
    Tool(Tool),
    Width(WidthPreset),
    PrimaryWell,
    SecondaryWell,
    Palette(usize),
}

#[derive(Debug, Clone, Copy)]
struct ToolbarMetrics {
    tool_tile_width: u16,
    tools_width: u16,
    option_start: u16,
    option_width: u16,
    option_tile_width: u16,
    colors_start: u16,
    well_width: u16,
    palette_start: u16,
    swatch_width: u16,
}

impl ToolbarMetrics {
    fn new(columns: u32) -> Self {
        let compact = columns < 59;
        let tool_tile_width = if compact { 2 } else { 4 };
        let tools_width = tool_tile_width * 6;
        let option_start = tools_width + 1;
        let option_tile_width = 2;
        let option_width = option_tile_width * 3;
        let well_width = if compact { 2 } else { 3 };
        let palette_start = if compact { 24 } else { 40 };
        let colors_start = palette_start - well_width;
        let swatch_width = if compact { 1 } else { 2 };
        Self {
            tool_tile_width,
            tools_width,
            option_start,
            option_width,
            option_tile_width,
            colors_start,
            well_width,
            palette_start,
            swatch_width,
        }
    }

    fn palette_slots(self, columns: u32) -> usize {
        let remaining = columns.saturating_sub(u32::from(self.palette_start));
        usize::try_from((remaining / u32::from(self.swatch_width)).min(14)).unwrap_or(14)
    }

    fn hit(self, row: u16, column: u16, columns: u32) -> Option<ToolbarControl> {
        if row >= 2 {
            return None;
        }
        if column < self.tools_width {
            let position = usize::from(column / self.tool_tile_width);
            return Tool::ALL
                .get(usize::from(row) * 6 + position)
                .copied()
                .map(ToolbarControl::Tool);
        }
        if row == 1 && column >= self.option_start && column < self.option_start + self.option_width
        {
            return WidthPreset::ALL
                .get(usize::from(
                    (column - self.option_start) / self.option_tile_width,
                ))
                .copied()
                .map(ToolbarControl::Width);
        }
        if column >= self.colors_start && column < self.colors_start + self.well_width {
            return Some(if row == 0 {
                ToolbarControl::PrimaryWell
            } else {
                ToolbarControl::SecondaryWell
            });
        }
        if column >= self.palette_start {
            let position = usize::from((column - self.palette_start) / self.swatch_width);
            if position < self.palette_slots(columns) {
                return Some(ToolbarControl::Palette(usize::from(row) * 14 + position));
            }
        }
        None
    }
}

fn handle_toolbar_click(
    row: u16,
    column: u16,
    button: MouseButton,
    canvas: &mut DrawingCanvas,
    state: &mut State,
    columns: u32,
) -> bool {
    let Some(control) = ToolbarMetrics::new(columns).hit(row, column, columns) else {
        return false;
    };
    match control {
        ToolbarControl::Tool(tool) => {
            canvas.cancel();
            state.set_tool(tool);
        }
        ToolbarControl::Width(width) => {
            if !state.tool.supports_width() {
                return false;
            }
            state.set_width(width);
        }
        ToolbarControl::PrimaryWell | ToolbarControl::SecondaryWell => {
            let target = if control == ToolbarControl::PrimaryWell {
                ColorTarget::Primary
            } else {
                ColorTarget::Secondary
            };
            state.input = InputMode::Color {
                target,
                buffer: String::new(),
            };
            state.message = format!("Enter {} color", color_target_label(target).to_lowercase());
        }
        ToolbarControl::Palette(index) => {
            let palette = PALETTE[index];
            let target = if button == MouseButton::Right {
                ColorTarget::Secondary
            } else {
                ColorTarget::Primary
            };
            state.set_color(target, palette.color, palette.name);
        }
    }
    true
}

fn write_toolbar_row<W: Write>(
    output: &mut W,
    state: &State,
    columns: u32,
    row: u16,
) -> Result<()> {
    let metrics = ToolbarMetrics::new(columns);
    for (position, tool) in Tool::ALL[usize::from(row) * 6..usize::from(row + 1) * 6]
        .iter()
        .enumerate()
    {
        queue!(
            output,
            MoveToColumn(u16::try_from(position).unwrap_or(u16::MAX) * metrics.tool_tile_width)
        )?;
        if *tool == state.tool {
            queue!(
                output,
                SetForegroundColor(Color::White),
                SetBackgroundColor(Color::DarkGrey)
            )?;
        } else {
            queue!(output, ResetColor)?;
        }
        queue!(output, Print(tool_tile(*tool, metrics.tool_tile_width)))?;
    }
    queue!(
        output,
        ResetColor,
        MoveToColumn(metrics.tools_width),
        Print("|"),
        MoveToColumn(metrics.option_start)
    )?;
    if row == 0 {
        queue!(output, Print("Width "))?;
    } else if state.tool.supports_width() {
        for (position, width) in WidthPreset::ALL.into_iter().enumerate() {
            queue!(
                output,
                MoveToColumn(
                    metrics.option_start
                        + u16::try_from(position).unwrap_or(u16::MAX) * metrics.option_tile_width
                )
            )?;
            if width == state.width() {
                queue!(
                    output,
                    SetForegroundColor(Color::White),
                    SetBackgroundColor(Color::DarkGrey)
                )?;
            } else {
                queue!(output, ResetColor)?;
            }
            queue!(output, Print(width_tile(width, metrics.option_tile_width)))?;
        }
    } else {
        queue!(output, Print(" ".repeat(usize::from(metrics.option_width))))?;
    }
    queue!(
        output,
        ResetColor,
        MoveToColumn(metrics.colors_start - 1),
        Print("|"),
        MoveToColumn(metrics.colors_start)
    )?;
    let (label, active_color) = if row == 0 {
        ("P", state.primary)
    } else {
        ("S", state.secondary)
    };
    let well = format!("{label}{}", " ".repeat(usize::from(metrics.well_width - 1)));
    queue!(
        output,
        SetForegroundColor(contrast_color(active_color)),
        SetBackgroundColor(terminal_color(active_color)),
        Print(well),
        ResetColor,
        MoveToColumn(metrics.palette_start)
    )?;
    let slots = metrics.palette_slots(columns);
    for palette in &PALETTE[usize::from(row) * 14..usize::from(row) * 14 + slots] {
        queue!(
            output,
            SetBackgroundColor(terminal_color(palette.color)),
            Print(" ".repeat(usize::from(metrics.swatch_width)))
        )?;
    }
    queue!(output, ResetColor)?;
    Ok(())
}

fn message_text(state: &State, canvas: &DrawingCanvas) -> String {
    match &state.input {
        InputMode::Color { target, buffer } => format!(
            "{} color> {buffer}  Enter apply, Esc cancel",
            color_target_label(*target)
        ),
        InputMode::Text { buffer, .. } => format!("Text> {buffer}  Enter apply, Esc cancel"),
        InputMode::None => {
            let (width, height) = canvas.dimensions();
            let tool_width = if state.tool.supports_width() {
                format!(" · {}", state.width().label())
            } else {
                String::new()
            };
            format!(
                "{}{} · P {} S {} · {}x{} · {} {} · {} · {}",
                tool_label(state.tool),
                tool_width,
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
                },
                state.message
            )
        }
    }
}

fn color_target_label(target: ColorTarget) -> &'static str {
    match target {
        ColorTarget::Primary => "Primary",
        ColorTarget::Secondary => "Secondary",
    }
}

fn truncate(value: &str, columns: u32) -> String {
    value.chars().take(columns as usize).collect()
}

fn contrast_color(value: Rgba<u8>) -> Color {
    if u16::from(value[0]) + u16::from(value[1]) + u16::from(value[2]) > 420 {
        Color::Black
    } else {
        Color::White
    }
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
        Tool::Eraser => "Eraser",
        Tool::Fill => "Fill",
        Tool::Picker => "Color Picker",
        Tool::Pencil => "Pencil",
        Tool::Brush => "Brush",
        Tool::Airbrush => "Airbrush",
        Tool::Text => "Text",
        Tool::Line => "Line",
        Tool::Rectangle => "Rectangle",
        Tool::Ellipse => "Ellipse",
        Tool::RoundedRectangle => "Rounded Rectangle",
        Tool::Highlighter => "Highlighter",
    }
}

fn tool_icon(tool: Tool) -> &'static str {
    match tool {
        Tool::Eraser => "\u{1f9fc}",           // soap
        Tool::Fill => "\u{1faa3}",             // bucket
        Tool::Picker => "\u{1f9ea}",           // test tube
        Tool::Pencil => "\u{270f}",            // pencil
        Tool::Brush => "\u{1f58c}",            // paintbrush
        Tool::Airbrush => "\u{1f4a8}",         // dashing away
        Tool::Text => "\u{1f524}",             // input Latin letters
        Tool::Line => "\u{1f4cf}",             // straight ruler
        Tool::Rectangle => "\u{f0e5f}",        // Material Design rectangle-outline
        Tool::Ellipse => "\u{f0ea1}",          // Material Design ellipse-outline
        Tool::RoundedRectangle => "\u{f14fc}", // Material Design square-rounded-outline
        Tool::Highlighter => "\u{1f58d}",      // crayon
    }
}

fn tool_icon_width(tool: Tool) -> u16 {
    match tool {
        Tool::Pencil
        | Tool::Brush
        | Tool::Rectangle
        | Tool::Ellipse
        | Tool::RoundedRectangle
        | Tool::Highlighter => 1,
        Tool::Eraser | Tool::Fill | Tool::Picker | Tool::Airbrush | Tool::Text | Tool::Line => 2,
    }
}

fn tool_tile(tool: Tool, width: u16) -> String {
    let padding = width.saturating_sub(tool_icon_width(tool));
    let leading = padding / 2;
    let trailing = padding - leading;
    let mut tile = " ".repeat(usize::from(leading));
    tile.push_str(tool_icon(tool));
    tile.push_str(&" ".repeat(usize::from(trailing)));
    tile
}

fn width_icon(width: WidthPreset) -> &'static str {
    match width {
        WidthPreset::Small => "S",
        WidthPreset::Medium => "M",
        WidthPreset::Large => "L",
    }
}

fn width_tile(width: WidthPreset, tile_width: u16) -> String {
    let mut tile = width_icon(width).to_owned();
    tile.push_str(&" ".repeat(usize::from(tile_width.saturating_sub(1))));
    tile
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use ab_glyph::{Font, FontRef};

    fn state() -> (DrawingCanvas, State) {
        let canvas = DrawingCanvas::blank(100, 50, Theme::Light);
        let state = State::new(&canvas, ExportFormat::Png, ExportSize::Canvas);
        (canvas, state)
    }

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
            backing_width: 800,
            backing_height: 420,
        }
    }

    #[test]
    fn resize_redraw_clears_stale_ui_rows() {
        let (canvas, state) = state();
        let mut output = Vec::new();
        render_ui(&mut output, &state, &canvas, layout(), true).unwrap();
        assert!(output.starts_with(b"\x1b[2J"));
    }

    #[test]
    fn tool_icons_use_supported_fonts_and_fit_tiles() {
        let font = FontRef::try_from_slice(include_bytes!("../assets/seguiemj-1.35-flat.ttf"))
            .expect("bundled Segoe UI Emoji font should be valid");

        for tool in Tool::ALL {
            let icon = tool_icon(tool);
            let base_characters = icon.chars().filter(|character| *character != '\u{fe0f}');
            assert_eq!(
                base_characters.count(),
                1,
                "{tool:?} must have one base glyph"
            );
            if !matches!(
                tool,
                Tool::Rectangle | Tool::Ellipse | Tool::RoundedRectangle
            ) {
                for character in icon.chars() {
                    assert_ne!(font.glyph_id(character).0, 0, "{tool:?} glyph is absent");
                }
            }
            for width in [2, 3] {
                let padding = width - tool_icon_width(tool);
                let leading = padding / 2;
                let mut expected = " ".repeat(usize::from(leading));
                expected.push_str(icon);
                expected.push_str(&" ".repeat(usize::from(padding - leading)));
                assert_eq!(tool_tile(tool, width), expected);
            }
        }

        assert_eq!(tool_icon(Tool::Line), "\u{1f4cf}");
        assert_eq!(tool_icon(Tool::Rectangle), "\u{f0e5f}");
        assert_eq!(tool_icon(Tool::Ellipse), "\u{f0ea1}");
        assert_eq!(tool_icon(Tool::RoundedRectangle), "\u{f14fc}");
        assert_eq!(tool_tile(Tool::Rectangle, 4), " \u{f0e5f}  ");
        assert_eq!(tool_tile(Tool::Ellipse, 4), " \u{f0ea1}  ");
        assert_eq!(tool_tile(Tool::RoundedRectangle, 4), " \u{f14fc}  ");

        assert_eq!(width_icon(WidthPreset::Small), "S");
        assert_eq!(width_icon(WidthPreset::Medium), "M");
        assert_eq!(width_icon(WidthPreset::Large), "L");
        assert_eq!(width_tile(WidthPreset::Small, 2), "S ");
        assert_eq!(width_tile(WidthPreset::Medium, 2), "M ");
        assert_eq!(width_tile(WidthPreset::Large, 2), "L ");
        for width in WidthPreset::ALL {
            for character in width_icon(width).chars() {
                assert_ne!(font.glyph_id(character).0, 0, "{width:?} glyph is absent");
            }
        }
    }

    #[test]
    fn custom_colors_and_shortcuts_work() {
        assert_eq!(parse_color("#0f0"), Some(Rgba([0, 255, 0, 255])));
        assert_eq!(parse_color("blue"), Some(Rgba([0, 0, 255, 255])));
        let (mut canvas, mut state) = state();
        handle_key(
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE),
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
        assert_eq!(state.tool, Tool::Pencil);
        assert!(!canvas.is_dirty());
    }

    #[test]
    fn toolbar_hitboxes_select_tools_widths_and_colors() {
        let (mut canvas, mut state) = state();
        let metrics = ToolbarMetrics::new(80);
        assert_eq!(metrics.tool_tile_width, 4);
        assert_eq!(metrics.palette_start, 40);
        assert_eq!(
            metrics.colors_start + metrics.well_width,
            metrics.palette_start,
            "color wells should touch the palette"
        );
        assert!(
            metrics.colors_start > metrics.option_start + metrics.option_width + 1,
            "color wells should be separated from width selection"
        );
        assert_eq!(
            metrics.hit(0, 0, 80),
            Some(ToolbarControl::Tool(Tool::Eraser))
        );
        assert_eq!(
            metrics.hit(1, metrics.tool_tile_width * 5, 80),
            Some(ToolbarControl::Tool(Tool::Highlighter))
        );

        handle_toolbar_click(
            0,
            metrics.tool_tile_width * 4,
            MouseButton::Left,
            &mut canvas,
            &mut state,
            80,
        );
        assert_eq!(state.tool, Tool::Brush);
        handle_toolbar_click(
            1,
            metrics.option_start + metrics.option_tile_width * 2,
            MouseButton::Left,
            &mut canvas,
            &mut state,
            80,
        );
        assert_eq!(state.width(), WidthPreset::Large);

        handle_toolbar_click(
            0,
            metrics.palette_start,
            MouseButton::Right,
            &mut canvas,
            &mut state,
            80,
        );
        assert_eq!(state.secondary, PALETTE[0].color);
        assert_eq!(
            metrics.hit(0, metrics.colors_start + metrics.well_width, 80),
            Some(ToolbarControl::Palette(0))
        );
    }

    #[test]
    fn widthless_tools_leave_contextual_options_inactive() {
        let (mut canvas, mut state) = state();
        let metrics = ToolbarMetrics::new(40);
        state.set_tool(Tool::Pencil);
        assert!(!handle_toolbar_click(
            1,
            metrics.option_start,
            MouseButton::Left,
            &mut canvas,
            &mut state,
            40,
        ));
        assert_eq!(state.width(), WidthPreset::Medium);
        assert_eq!(metrics.palette_start, 24);
        assert_eq!(metrics.palette_slots(40), 14);
    }
}
