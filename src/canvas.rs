use std::{collections::VecDeque, sync::OnceLock};

use ab_glyph::{Font, FontArc, PxScale, ScaleFont, point};
use image::{DynamicImage, Rgba, RgbaImage, imageops::FilterType};

use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub fn new(x: f32, y: f32) -> Self {
        Self {
            x: x.clamp(0.0, 1.0),
            y: y.clamp(0.0, 1.0),
        }
    }

    fn transformed(self, fit: FitRect) -> Self {
        Self {
            x: (self.x - fit.x) / fit.width.max(f32::EPSILON),
            y: (self.y - fit.y) / fit.height.max(f32::EPSILON),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Eraser,
    Fill,
    Picker,
    Pencil,
    Brush,
    Airbrush,
    Text,
    Line,
    Rectangle,
    Ellipse,
    RoundedRectangle,
    Highlighter,
}

impl Tool {
    pub const ALL: [Self; 12] = [
        Self::Eraser,
        Self::Fill,
        Self::Picker,
        Self::Pencil,
        Self::Brush,
        Self::Airbrush,
        Self::Text,
        Self::Line,
        Self::Rectangle,
        Self::Ellipse,
        Self::RoundedRectangle,
        Self::Highlighter,
    ];
    pub const COUNT: usize = Self::ALL.len();

    pub const fn index(self) -> usize {
        match self {
            Self::Eraser => 0,
            Self::Fill => 1,
            Self::Picker => 2,
            Self::Pencil => 3,
            Self::Brush => 4,
            Self::Airbrush => 5,
            Self::Text => 6,
            Self::Line => 7,
            Self::Rectangle => 8,
            Self::Ellipse => 9,
            Self::RoundedRectangle => 10,
            Self::Highlighter => 11,
        }
    }

    pub const fn supports_width(self) -> bool {
        matches!(
            self,
            Self::Eraser
                | Self::Brush
                | Self::Airbrush
                | Self::Line
                | Self::Rectangle
                | Self::Ellipse
                | Self::RoundedRectangle
                | Self::Highlighter
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidthPreset {
    Small,
    Medium,
    Large,
    ExtraLarge,
}

impl WidthPreset {
    pub const ALL: [Self; 4] = [Self::Small, Self::Medium, Self::Large, Self::ExtraLarge];

    pub fn previous(self) -> Self {
        match self {
            Self::Small => Self::ExtraLarge,
            Self::Medium => Self::Small,
            Self::Large => Self::Medium,
            Self::ExtraLarge => Self::Large,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Small => Self::Medium,
            Self::Medium => Self::Large,
            Self::Large => Self::ExtraLarge,
            Self::ExtraLarge => Self::Small,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
            Self::ExtraLarge => "extra large",
        }
    }

    fn stroke_scale(self) -> f32 {
        match self {
            Self::Small => 0.65,
            Self::Medium => 1.0,
            Self::Large => 1.7,
            Self::ExtraLarge => 2.5,
        }
    }

    fn text_scale(self) -> f32 {
        match self {
            Self::Small => 0.85,
            Self::Medium => 1.1,
            Self::Large => 1.55,
            Self::ExtraLarge => 2.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub color: Rgba<u8>,
    pub width: WidthPreset,
    pub opacity: f32,
}

impl Style {
    pub fn opaque(color: Rgba<u8>, width: WidthPreset) -> Self {
        Self {
            color,
            width,
            opacity: 1.0,
        }
    }

    pub fn highlighter(color: Rgba<u8>, width: WidthPreset) -> Self {
        Self {
            color,
            width,
            opacity: 0.38,
        }
    }
}

#[derive(Debug, Clone)]
pub enum BaseSource {
    Blank,
    Image(DynamicImage),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Element {
    Pencil {
        points: Vec<Point>,
        color: Rgba<u8>,
    },
    Stroke {
        points: Vec<Point>,
        style: Style,
    },
    Airbrush {
        points: Vec<Point>,
        style: Style,
    },
    Highlighter {
        points: Vec<Point>,
        style: Style,
    },
    Line {
        start: Point,
        end: Point,
        style: Style,
    },
    Rectangle {
        start: Point,
        end: Point,
        style: Style,
    },
    Ellipse {
        start: Point,
        end: Point,
        style: Style,
    },
    RoundedRectangle {
        start: Point,
        end: Point,
        style: Style,
    },
    Text {
        position: Point,
        text: String,
        style: Style,
    },
    FloodFill {
        point: Point,
        color: Rgba<u8>,
    },
}

#[derive(Debug, Clone, PartialEq)]
enum Operation {
    Element(Element),
    ClearAnnotations,
}

#[derive(Debug, Clone, Copy)]
struct FitRect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl FitRect {
    fn full() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RenderSizing {
    pub pencil_width: f32,
    pub stroke_radius: f32,
    pub text_size: f32,
}

impl RenderSizing {
    pub fn scaled(self, scale: f32) -> Self {
        Self {
            pencil_width: self.pencil_width * scale,
            stroke_radius: self.stroke_radius * scale,
            text_size: self.text_size * scale,
        }
    }

    pub fn radius(self, style: Style) -> f32 {
        (self.stroke_radius * style.width.stroke_scale()).max(0.5)
    }

    pub fn text_size(self, style: Style) -> f32 {
        (self.text_size * style.width.text_scale()).max(4.0)
    }
}

#[derive(Debug, Clone)]
pub struct DrawingCanvas {
    width: u32,
    height: u32,
    source: BaseSource,
    base: RgbaImage,
    fit: FitRect,
    committed: RgbaImage,
    history: Vec<Operation>,
    cursor: usize,
    current: Option<Element>,
    theme: Theme,
}

impl DrawingCanvas {
    pub fn new(width: u32, height: u32, source: BaseSource, theme: Theme) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let (base, fit) = render_base(&source, width, height, theme.background());
        Self {
            width,
            height,
            source,
            committed: base.clone(),
            base,
            fit,
            history: Vec::new(),
            cursor: 0,
            current: None,
            theme,
        }
    }

    #[cfg(test)]
    pub fn blank(width: u32, height: u32, theme: Theme) -> Self {
        Self::new(width, height, BaseSource::Blank, theme)
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn default_primary(&self) -> Rgba<u8> {
        self.theme.foreground()
    }

    pub fn default_secondary(&self) -> Rgba<u8> {
        self.theme.background()
    }

    pub fn sizing(&self) -> RenderSizing {
        let unit = (self.width as f32 / 80.0).min(self.height as f32 / 22.0);
        RenderSizing {
            pencil_width: 1.0,
            stroke_radius: (unit * 0.175).max(0.75),
            text_size: (self.height as f32 / 22.0 * 1.1).max(6.0),
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width.max(1);
        self.height = height.max(1);
        (self.base, self.fit) = render_base(
            &self.source,
            self.width,
            self.height,
            self.theme.background(),
        );
        self.rebuild();
    }

    pub fn begin(&mut self, tool: Tool, point: Point, style: Style) -> bool {
        self.current = match tool {
            Tool::Pencil => Some(Element::Pencil {
                points: vec![point],
                color: style.color,
            }),
            Tool::Brush | Tool::Eraser => Some(Element::Stroke {
                points: vec![point],
                style,
            }),
            Tool::Airbrush => Some(Element::Airbrush {
                points: vec![point],
                style,
            }),
            Tool::Highlighter => Some(Element::Highlighter {
                points: vec![point],
                style,
            }),
            Tool::Line => Some(Element::Line {
                start: point,
                end: point,
                style,
            }),
            Tool::Rectangle => Some(Element::Rectangle {
                start: point,
                end: point,
                style,
            }),
            Tool::Ellipse => Some(Element::Ellipse {
                start: point,
                end: point,
                style,
            }),
            Tool::RoundedRectangle => Some(Element::RoundedRectangle {
                start: point,
                end: point,
                style,
            }),
            Tool::Text | Tool::Fill | Tool::Picker => None,
        };
        self.current.is_some()
    }

    pub fn extend(&mut self, point: Point) -> bool {
        match self.current.as_mut() {
            Some(
                Element::Pencil { points, .. }
                | Element::Stroke { points, .. }
                | Element::Airbrush { points, .. }
                | Element::Highlighter { points, .. },
            ) => {
                if points.last().copied() != Some(point) {
                    points.push(point);
                }
            }
            Some(
                Element::Line { end, .. }
                | Element::Rectangle { end, .. }
                | Element::Ellipse { end, .. }
                | Element::RoundedRectangle { end, .. },
            ) => *end = point,
            Some(Element::Text { .. } | Element::FloodFill { .. }) | None => return false,
        }
        true
    }

    pub fn finish(&mut self) -> bool {
        let Some(element) = self.current.take() else {
            return false;
        };
        self.commit(Operation::Element(element))
    }

    pub fn cancel(&mut self) -> bool {
        self.current.take().is_some()
    }

    pub fn add_text(&mut self, position: Point, text: String, style: Style) -> bool {
        if text.trim().is_empty() {
            return false;
        }
        self.commit(Operation::Element(Element::Text {
            position,
            text,
            style,
        }))
    }

    pub fn fill(&mut self, point: Point, color: Rgba<u8>) -> bool {
        if self.color_at(point) == color {
            return false;
        }
        self.commit(Operation::Element(Element::FloodFill { point, color }))
    }

    pub fn clear(&mut self) -> bool {
        self.current = None;
        if self.active_elements().is_empty() {
            return false;
        }
        self.commit(Operation::ClearAnnotations)
    }

    pub fn undo(&mut self) -> bool {
        self.current = None;
        if self.cursor == 0 {
            return false;
        }
        self.cursor -= 1;
        self.rebuild();
        true
    }

    pub fn redo(&mut self) -> bool {
        self.current = None;
        if self.cursor == self.history.len() {
            return false;
        }
        self.cursor += 1;
        self.rebuild();
        true
    }

    pub fn is_dirty(&self) -> bool {
        self.cursor != 0
    }

    pub fn color_at(&self, point: Point) -> Rgba<u8> {
        let (x, y) = pixel(point, self.width, self.height);
        *self.committed.get_pixel(x, y)
    }

    pub fn render(&self) -> RgbaImage {
        let mut image = self.committed.clone();
        if let Some(element) = &self.current {
            draw_element(&mut image, element, self.sizing());
        }
        image
    }

    pub fn render_canvas_export(&self) -> RgbaImage {
        self.committed.clone()
    }

    pub fn render_original_export(&self) -> RgbaImage {
        let BaseSource::Image(image) = &self.source else {
            return self.render_canvas_export();
        };
        let mut output = image.to_rgba8();
        let scale = (output.width() as f32 / (self.fit.width * self.width as f32).max(1.0))
            .min(output.height() as f32 / (self.fit.height * self.height as f32).max(1.0));
        let sizing = self.sizing().scaled(scale);
        let base = output.clone();
        for operation in &self.history[..self.cursor] {
            match operation {
                Operation::ClearAnnotations => output = base.clone(),
                Operation::Element(element) => {
                    let element = transform_element(element, self.fit);
                    draw_element(&mut output, &element, sizing);
                }
            }
        }
        output
    }

    pub fn source(&self) -> &BaseSource {
        &self.source
    }

    pub fn canvas_base(&self) -> &RgbaImage {
        &self.base
    }

    pub fn original_base(&self) -> Option<RgbaImage> {
        match &self.source {
            BaseSource::Image(image) => Some(image.to_rgba8()),
            BaseSource::Blank => None,
        }
    }

    pub fn original_scale(&self) -> Option<f32> {
        let BaseSource::Image(image) = &self.source else {
            return None;
        };
        Some(
            (image.width() as f32 / (self.fit.width * self.width as f32).max(1.0))
                .min(image.height() as f32 / (self.fit.height * self.height as f32).max(1.0)),
        )
    }

    pub fn active_elements(&self) -> Vec<Element> {
        let mut elements = Vec::new();
        for operation in &self.history[..self.cursor] {
            match operation {
                Operation::ClearAnnotations => elements.clear(),
                Operation::Element(element) => elements.push(element.clone()),
            }
        }
        elements
    }

    pub fn transformed_active_elements(&self) -> Vec<Element> {
        self.active_elements()
            .iter()
            .map(|element| transform_element(element, self.fit))
            .collect()
    }

    pub fn requires_raster_svg(&self) -> bool {
        self.active_elements()
            .iter()
            .any(|element| matches!(element, Element::FloodFill { .. }))
    }

    fn commit(&mut self, operation: Operation) -> bool {
        self.history.truncate(self.cursor);
        let sizing = self.sizing();
        apply_operation(&mut self.committed, &self.base, &operation, sizing);
        self.history.push(operation);
        self.cursor += 1;
        true
    }

    fn rebuild(&mut self) {
        self.committed = self.base.clone();
        let sizing = self.sizing();
        for operation in &self.history[..self.cursor] {
            apply_operation(&mut self.committed, &self.base, operation, sizing);
        }
    }
}

fn apply_operation(
    image: &mut RgbaImage,
    base: &RgbaImage,
    operation: &Operation,
    sizing: RenderSizing,
) {
    match operation {
        Operation::ClearAnnotations => image.clone_from(base),
        Operation::Element(element) => draw_element(image, element, sizing),
    }
}

fn render_base(
    source: &BaseSource,
    width: u32,
    height: u32,
    background: Rgba<u8>,
) -> (RgbaImage, FitRect) {
    let mut base = RgbaImage::from_pixel(width, height, background);
    let BaseSource::Image(image) = source else {
        return (base, FitRect::full());
    };
    let scale = (width as f64 / image.width().max(1) as f64)
        .min(height as f64 / image.height().max(1) as f64);
    let target_width = (image.width() as f64 * scale)
        .round()
        .clamp(1.0, width as f64) as u32;
    let target_height = (image.height() as f64 * scale)
        .round()
        .clamp(1.0, height as f64) as u32;
    let resized = image
        .resize_exact(target_width, target_height, FilterType::Lanczos3)
        .to_rgba8();
    let x = (width - target_width) / 2;
    let y = (height - target_height) / 2;
    image::imageops::overlay(&mut base, &resized, i64::from(x), i64::from(y));
    (
        base,
        FitRect {
            x: x as f32 / width as f32,
            y: y as f32 / height as f32,
            width: target_width as f32 / width as f32,
            height: target_height as f32 / height as f32,
        },
    )
}

fn transform_element(element: &Element, fit: FitRect) -> Element {
    match element {
        Element::Pencil { points, color } => Element::Pencil {
            points: points.iter().map(|point| point.transformed(fit)).collect(),
            color: *color,
        },
        Element::Stroke { points, style } => Element::Stroke {
            points: points.iter().map(|point| point.transformed(fit)).collect(),
            style: *style,
        },
        Element::Airbrush { points, style } => Element::Airbrush {
            points: points.iter().map(|point| point.transformed(fit)).collect(),
            style: *style,
        },
        Element::Highlighter { points, style } => Element::Highlighter {
            points: points.iter().map(|point| point.transformed(fit)).collect(),
            style: *style,
        },
        Element::Line { start, end, style } => Element::Line {
            start: start.transformed(fit),
            end: end.transformed(fit),
            style: *style,
        },
        Element::Rectangle { start, end, style } => Element::Rectangle {
            start: start.transformed(fit),
            end: end.transformed(fit),
            style: *style,
        },
        Element::Ellipse { start, end, style } => Element::Ellipse {
            start: start.transformed(fit),
            end: end.transformed(fit),
            style: *style,
        },
        Element::RoundedRectangle { start, end, style } => Element::RoundedRectangle {
            start: start.transformed(fit),
            end: end.transformed(fit),
            style: *style,
        },
        Element::Text {
            position,
            text,
            style,
        } => Element::Text {
            position: position.transformed(fit),
            text: text.clone(),
            style: *style,
        },
        Element::FloodFill { point, color } => Element::FloodFill {
            point: point.transformed(fit),
            color: *color,
        },
    }
}

fn draw_element(image: &mut RgbaImage, element: &Element, sizing: RenderSizing) {
    match element {
        Element::Pencil { points, color } => {
            draw_pencil_path(image, points, *color, sizing.pencil_width)
        }
        Element::Stroke { points, style } => {
            draw_path(image, points, *style, sizing.radius(*style))
        }
        Element::Airbrush { points, style } => draw_airbrush(image, points, *style, sizing),
        Element::Highlighter { points, style } => {
            draw_path(image, points, *style, sizing.radius(*style) * 3.2)
        }
        Element::Line { start, end, style } => {
            draw_segment(image, *start, *end, *style, sizing.radius(*style))
        }
        Element::Rectangle { start, end, style } => {
            draw_rectangle(image, *start, *end, *style, sizing.radius(*style))
        }
        Element::Ellipse { start, end, style } => {
            draw_ellipse(image, *start, *end, *style, sizing.radius(*style))
        }
        Element::RoundedRectangle { start, end, style } => {
            draw_rounded_rectangle(image, *start, *end, *style, sizing.radius(*style))
        }
        Element::Text {
            position,
            text,
            style,
        } => draw_text(image, *position, text, *style, sizing.text_size(*style)),
        Element::FloodFill { point, color } => flood_fill(image, *point, *color),
    }
}

fn draw_pencil_path(image: &mut RgbaImage, points: &[Point], color: Rgba<u8>, width: f32) {
    if width > 1.0 {
        draw_path(
            image,
            points,
            Style::opaque(color, WidthPreset::Medium),
            width * 0.5,
        );
        return;
    }
    let Some(first) = points.first().copied() else {
        return;
    };
    let (mut previous_x, mut previous_y) = pixel(first, image.width(), image.height());
    *image.get_pixel_mut(previous_x, previous_y) = color;
    for point in &points[1..] {
        let (next_x, next_y) = pixel(*point, image.width(), image.height());
        draw_pixel_line(image, previous_x, previous_y, next_x, next_y, color);
        (previous_x, previous_y) = (next_x, next_y);
    }
}

fn draw_pixel_line(
    image: &mut RgbaImage,
    start_x: u32,
    start_y: u32,
    end_x: u32,
    end_y: u32,
    color: Rgba<u8>,
) {
    let (mut x, mut y) = (i64::from(start_x), i64::from(start_y));
    let (end_x, end_y) = (i64::from(end_x), i64::from(end_y));
    let dx = (end_x - x).abs();
    let step_x = if x < end_x { 1 } else { -1 };
    let dy = -(end_y - y).abs();
    let step_y = if y < end_y { 1 } else { -1 };
    let mut error = dx + dy;
    loop {
        if let (Ok(px), Ok(py)) = (u32::try_from(x), u32::try_from(y)) {
            *image.get_pixel_mut(px, py) = color;
        }
        if x == end_x && y == end_y {
            break;
        }
        let doubled = error.saturating_mul(2);
        if doubled >= dy {
            error += dy;
            x += step_x;
        }
        if doubled <= dx {
            error += dx;
            y += step_y;
        }
    }
}

fn draw_airbrush(image: &mut RgbaImage, points: &[Point], style: Style, sizing: RenderSizing) {
    for_each_airbrush_dot(
        points,
        style,
        sizing,
        image.width(),
        image.height(),
        |x, y, radius| stamp_xy(image, x, y, style, radius),
    );
}

pub(crate) fn for_each_airbrush_dot(
    points: &[Point],
    style: Style,
    sizing: RenderSizing,
    width: u32,
    height: u32,
    mut draw: impl FnMut(f32, f32, f32),
) {
    let Some(first) = points.first().copied() else {
        return;
    };
    let spray_radius = sizing.radius(style) * 3.2;
    let spacing = (spray_radius * 0.45).max(1.0);
    let dots = ((spray_radius * spray_radius * 0.7).ceil() as u32).clamp(6, 64);
    let mut sample_index = 0_u64;
    let mut emit = |x: f32, y: f32, sample: u64| {
        for dot in 0..dots {
            let first_hash = scatter_hash(sample, u64::from(dot));
            let second_hash = scatter_hash(first_hash, u64::from(dot) ^ 0x9e37_79b9);
            let angle = hash_fraction(first_hash) * std::f32::consts::TAU;
            let distance = hash_fraction(second_hash).sqrt() * spray_radius;
            draw(x + angle.cos() * distance, y + angle.sin() * distance, 0.65);
        }
    };
    let (mut previous_x, mut previous_y) = float_pixel(first, width, height);
    emit(previous_x, previous_y, sample_index);
    sample_index = sample_index.wrapping_add(1);
    for point in &points[1..] {
        let (next_x, next_y) = float_pixel(*point, width, height);
        let distance = (next_x - previous_x).hypot(next_y - previous_y);
        let steps = (distance / spacing).ceil().max(1.0) as u32;
        for step in 1..=steps {
            let amount = step as f32 / steps as f32;
            emit(
                previous_x + (next_x - previous_x) * amount,
                previous_y + (next_y - previous_y) * amount,
                sample_index,
            );
            sample_index = sample_index.wrapping_add(1);
        }
        (previous_x, previous_y) = (next_x, next_y);
    }
}

fn scatter_hash(sample: u64, dot: u64) -> u64 {
    let mut value = sample
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        .wrapping_add(dot.wrapping_mul(0xbf58_476d_1ce4_e5b9))
        .wrapping_add(0x94d0_49bb_1331_11eb);
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value.wrapping_mul(0x94d0_49bb_1331_11eb) ^ (value >> 31)
}

fn hash_fraction(value: u64) -> f32 {
    (value >> 40) as f32 / (1_u32 << 24) as f32
}

fn draw_path(image: &mut RgbaImage, points: &[Point], style: Style, radius: f32) {
    if let Some(first) = points.first().copied() {
        stamp(image, first, style, radius);
    }
    for pair in points.windows(2) {
        draw_segment(image, pair[0], pair[1], style, radius);
    }
}

fn draw_segment(image: &mut RgbaImage, start: Point, end: Point, style: Style, radius: f32) {
    let (x1, y1) = float_pixel(start, image.width(), image.height());
    let (x2, y2) = float_pixel(end, image.width(), image.height());
    let distance = (x2 - x1).hypot(y2 - y1);
    let steps = (distance / (radius * 0.5).max(0.5)).ceil().max(1.0) as u32;
    for index in 0..=steps {
        let t = index as f32 / steps as f32;
        stamp_xy(image, x1 + (x2 - x1) * t, y1 + (y2 - y1) * t, style, radius);
    }
}

fn draw_rectangle(image: &mut RgbaImage, start: Point, end: Point, style: Style, radius: f32) {
    let a = Point {
        x: start.x,
        y: start.y,
    };
    let b = Point {
        x: end.x,
        y: start.y,
    };
    let c = Point { x: end.x, y: end.y };
    let d = Point {
        x: start.x,
        y: end.y,
    };
    for (from, to) in [(a, b), (b, c), (c, d), (d, a)] {
        draw_segment(image, from, to, style, radius);
    }
}

fn draw_ellipse(image: &mut RgbaImage, start: Point, end: Point, style: Style, radius: f32) {
    let (x1, y1) = float_pixel(start, image.width(), image.height());
    let (x2, y2) = float_pixel(end, image.width(), image.height());
    let (cx, cy) = ((x1 + x2) * 0.5, (y1 + y2) * 0.5);
    let (rx, ry) = ((x2 - x1).abs() * 0.5, (y2 - y1).abs() * 0.5);
    let steps = ((rx + ry) * std::f32::consts::PI).ceil().max(24.0) as u32;
    let mut previous = Point::new((cx + rx) / image.width() as f32, cy / image.height() as f32);
    for index in 1..=steps {
        let angle = std::f32::consts::TAU * index as f32 / steps as f32;
        let current = Point::new(
            (cx + rx * angle.cos()) / image.width() as f32,
            (cy + ry * angle.sin()) / image.height() as f32,
        );
        draw_segment(image, previous, current, style, radius);
        previous = current;
    }
}

fn draw_rounded_rectangle(
    image: &mut RgbaImage,
    start: Point,
    end: Point,
    style: Style,
    radius: f32,
) {
    let (x1, y1) = float_pixel(start, image.width(), image.height());
    let (x2, y2) = float_pixel(end, image.width(), image.height());
    let (left, right) = (x1.min(x2), x1.max(x2));
    let (top, bottom) = (y1.min(y2), y1.max(y2));
    let corner = ((right - left).min(bottom - top) * 0.2)
        .min(radius * 16.0)
        .max(0.0);
    if corner < 1.0 {
        draw_rectangle(image, start, end, style, radius);
        return;
    }
    let point = |x: f32, y: f32| {
        Point::new(
            x / image.width().max(1) as f32,
            y / image.height().max(1) as f32,
        )
    };
    let mut points = Vec::with_capacity(37);
    let corners = [
        (right - corner, top + corner, -std::f32::consts::FRAC_PI_2),
        (right - corner, bottom - corner, 0.0),
        (left + corner, bottom - corner, std::f32::consts::FRAC_PI_2),
        (left + corner, top + corner, std::f32::consts::PI),
    ];
    for (center_x, center_y, start_angle) in corners {
        for step in 0..=8 {
            let angle = start_angle + std::f32::consts::FRAC_PI_2 * step as f32 / 8.0;
            points.push(point(
                center_x + corner * angle.cos(),
                center_y + corner * angle.sin(),
            ));
        }
    }
    points.push(points[0]);
    draw_path(image, &points, style, radius);
}

fn stamp(image: &mut RgbaImage, point: Point, style: Style, radius: f32) {
    let (x, y) = float_pixel(point, image.width(), image.height());
    stamp_xy(image, x, y, style, radius);
}

fn stamp_xy(image: &mut RgbaImage, x: f32, y: f32, style: Style, radius: f32) {
    let min_x = (x - radius).floor().max(0.0) as u32;
    let max_x = (x + radius)
        .ceil()
        .min(image.width().saturating_sub(1) as f32) as u32;
    let min_y = (y - radius).floor().max(0.0) as u32;
    let max_y = (y + radius)
        .ceil()
        .min(image.height().saturating_sub(1) as f32) as u32;
    let radius2 = radius * radius;
    for py in min_y..=max_y {
        for px in min_x..=max_x {
            let dx = px as f32 + 0.5 - x;
            let dy = py as f32 + 0.5 - y;
            if dx * dx + dy * dy <= radius2 {
                blend(image.get_pixel_mut(px, py), style.color, style.opacity);
            }
        }
    }
}

fn blend(destination: &mut Rgba<u8>, source: Rgba<u8>, opacity: f32) {
    let alpha = opacity.clamp(0.0, 1.0) * f32::from(source[3]) / 255.0;
    for channel in 0..3 {
        destination[channel] = (f32::from(destination[channel]) * (1.0 - alpha)
            + f32::from(source[channel]) * alpha)
            .round() as u8;
    }
    destination[3] = 255;
}

fn flood_fill(image: &mut RgbaImage, point: Point, replacement: Rgba<u8>) {
    let (start_x, start_y) = pixel(point, image.width(), image.height());
    let target = *image.get_pixel(start_x, start_y);
    if target == replacement {
        return;
    }
    let mut queue = VecDeque::from([(start_x, start_y)]);
    *image.get_pixel_mut(start_x, start_y) = replacement;
    while let Some((x, y)) = queue.pop_front() {
        for (nx, ny) in [
            x.checked_sub(1).map(|v| (v, y)),
            x.checked_add(1)
                .filter(|v| *v < image.width())
                .map(|v| (v, y)),
            y.checked_sub(1).map(|v| (x, v)),
            y.checked_add(1)
                .filter(|v| *v < image.height())
                .map(|v| (x, v)),
        ]
        .into_iter()
        .flatten()
        {
            if *image.get_pixel(nx, ny) == target {
                *image.get_pixel_mut(nx, ny) = replacement;
                queue.push_back((nx, ny));
            }
        }
    }
}

fn draw_text(image: &mut RgbaImage, position: Point, text: &str, style: Style, size: f32) {
    let Some(font) = font() else { return };
    let scaled = font.as_scaled(PxScale::from(size));
    let (x, y) = float_pixel(position, image.width(), image.height());
    let mut cursor = point(x, y + scaled.ascent());
    for character in text.chars() {
        if character == '\n' {
            cursor.x = x;
            cursor.y += scaled.height();
            continue;
        }
        let glyph_id = scaled.glyph_id(character);
        let glyph = glyph_id.with_scale_and_position(size, cursor);
        if let Some(outline) = font.outline_glyph(glyph) {
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, coverage| {
                let px = bounds.min.x as i32 + gx as i32;
                let py = bounds.min.y as i32 + gy as i32;
                if px >= 0 && py >= 0 && (px as u32) < image.width() && (py as u32) < image.height()
                {
                    blend(
                        image.get_pixel_mut(px as u32, py as u32),
                        style.color,
                        style.opacity * coverage,
                    );
                }
            });
        }
        cursor.x += scaled.h_advance(glyph_id) + scaled.kern(glyph_id, glyph_id) * 0.0;
    }
}

fn font() -> Option<&'static FontArc> {
    static FONT: OnceLock<Option<FontArc>> = OnceLock::new();
    FONT.get_or_init(|| FontArc::try_from_slice(annotation_font_bytes()).ok())
        .as_ref()
}

pub fn annotation_font_bytes() -> &'static [u8] {
    include_bytes!("../assets/NotoSans-Regular.ttf")
}

fn pixel(point: Point, width: u32, height: u32) -> (u32, u32) {
    let x = (point.x * width as f32)
        .floor()
        .clamp(0.0, width.saturating_sub(1) as f32) as u32;
    let y = (point.y * height as f32)
        .floor()
        .clamp(0.0, height.saturating_sub(1) as f32) as u32;
    (x, y)
}

fn float_pixel(point: Point, width: u32, height: u32) -> (f32, f32) {
    (
        (point.x * width as f32).min(width as f32 - 0.5),
        (point.y * height as f32).min(height as f32 - 0.5),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style(color: Rgba<u8>) -> Style {
        Style::opaque(color, WidthPreset::Medium)
    }

    #[test]
    fn stroke_line_shapes_and_fill_are_deterministic() {
        let mut canvas = DrawingCanvas::blank(100, 50, Theme::Light);
        canvas.begin(
            Tool::Brush,
            Point::new(0.1, 0.2),
            style(Rgba([255, 0, 0, 255])),
        );
        canvas.extend(Point::new(0.9, 0.2));
        canvas.finish();
        assert_eq!(
            canvas.color_at(Point::new(0.5, 0.2)),
            Rgba([255, 0, 0, 255])
        );
        canvas.begin(
            Tool::Rectangle,
            Point::new(0.2, 0.3),
            style(Rgba([0, 0, 0, 255])),
        );
        canvas.extend(Point::new(0.8, 0.8));
        canvas.finish();
        assert!(canvas.fill(Point::new(0.5, 0.5), Rgba([0, 255, 0, 255])));
        assert_eq!(
            canvas.color_at(Point::new(0.5, 0.5)),
            Rgba([0, 255, 0, 255])
        );
        assert_eq!(
            canvas.color_at(Point::new(0.05, 0.5)),
            Rgba([255, 255, 255, 255])
        );
    }

    #[test]
    fn preview_does_not_mutate_until_commit() {
        let mut canvas = DrawingCanvas::blank(20, 20, Theme::Light);
        canvas.begin(
            Tool::Line,
            Point::new(0.0, 0.0),
            style(Rgba([0, 0, 0, 255])),
        );
        canvas.extend(Point::new(1.0, 1.0));
        assert_ne!(canvas.render(), canvas.render_canvas_export());
        assert!(!canvas.is_dirty());
        canvas.finish();
        assert!(canvas.is_dirty());
    }

    #[test]
    fn undo_redo_clear_and_branching_work() {
        let mut canvas = DrawingCanvas::blank(30, 20, Theme::Light);
        for y in [0.2, 0.4] {
            canvas.begin(Tool::Brush, Point::new(0.2, y), style(Rgba([0, 0, 0, 255])));
            canvas.finish();
        }
        assert!(canvas.clear());
        assert_eq!(canvas.active_elements().len(), 0);
        assert!(canvas.undo());
        assert_eq!(canvas.active_elements().len(), 2);
        assert!(canvas.redo());
        assert!(canvas.undo());
        canvas.begin(
            Tool::Line,
            Point::new(0.0, 0.0),
            style(Rgba([255, 0, 0, 255])),
        );
        canvas.finish();
        assert!(!canvas.redo());
        while canvas.undo() {}
        assert!(!canvas.is_dirty());
    }

    #[test]
    fn large_fill_is_iterative() {
        let mut canvas = DrawingCanvas::blank(1024, 512, Theme::Light);
        assert!(canvas.fill(Point::new(0.5, 0.5), Rgba([1, 2, 3, 255])));
        assert_eq!(
            canvas.color_at(Point::new(0.99, 0.99)),
            Rgba([1, 2, 3, 255])
        );
    }

    #[test]
    fn resize_replays_normalized_operations() {
        let mut canvas = DrawingCanvas::blank(20, 10, Theme::Light);
        canvas.begin(
            Tool::Line,
            Point::new(0.1, 0.5),
            style(Rgba([0, 0, 0, 255])),
        );
        canvas.extend(Point::new(0.9, 0.5));
        canvas.finish();
        canvas.resize(200, 100);
        assert_eq!(canvas.color_at(Point::new(0.5, 0.5)), Rgba([0, 0, 0, 255]));
    }

    #[test]
    fn pencil_is_one_pixel_wide() {
        let mut canvas = DrawingCanvas::blank(40, 20, Theme::Light);
        canvas.begin(
            Tool::Pencil,
            Point::new(0.1, 0.5),
            style(Rgba([0, 0, 0, 255])),
        );
        canvas.extend(Point::new(0.9, 0.5));
        canvas.finish();
        let image = canvas.render_canvas_export();
        let painted_rows: Vec<u32> = (0..image.height())
            .filter(|y| (0..image.width()).any(|x| image.get_pixel(x, *y)[0] == 0))
            .collect();
        assert_eq!(painted_rows, vec![10]);
    }

    #[test]
    fn airbrush_is_deterministic_and_replays_after_resize() {
        let mut canvas = DrawingCanvas::blank(80, 40, Theme::Light);
        canvas.begin(
            Tool::Airbrush,
            Point::new(0.2, 0.4),
            style(Rgba([255, 0, 0, 255])),
        );
        canvas.extend(Point::new(0.8, 0.6));
        let preview = canvas.render();
        assert_eq!(preview, canvas.render());
        canvas.finish();
        assert_eq!(preview, canvas.render_canvas_export());
        canvas.resize(160, 80);
        let resized = canvas.render_canvas_export();
        canvas.resize(160, 80);
        assert_eq!(resized, canvas.render_canvas_export());
    }

    #[test]
    fn rounded_rectangle_keeps_rounded_corners() {
        let mut canvas = DrawingCanvas::blank(100, 50, Theme::Light);
        canvas.begin(
            Tool::RoundedRectangle,
            Point::new(0.2, 0.2),
            style(Rgba([0, 0, 0, 255])),
        );
        canvas.extend(Point::new(0.8, 0.8));
        canvas.finish();
        assert_eq!(canvas.color_at(Point::new(0.5, 0.2)), Rgba([0, 0, 0, 255]));
        assert_eq!(
            canvas.color_at(Point::new(0.2, 0.2)),
            Rgba([255, 255, 255, 255])
        );
    }
}
