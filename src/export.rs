use std::{fmt, io::Write, path::Path};

use anyhow::{Context, Result};
use base64::{Engine, prelude::BASE64_STANDARD};
use clap::ValueEnum;
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};

use crate::canvas::{
    BaseSource, DrawingCanvas, Element, Point, RenderSizing, Style, annotation_font_bytes,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ExportFormat {
    Png,
    Svg,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Svg => "svg",
        }
    }

    pub fn from_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "png" => Some(Self::Png),
            "svg" => Some(Self::Svg),
            _ => None,
        }
    }
}

impl fmt::Display for ExportFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.extension())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ExportSize {
    Original,
    Canvas,
}

impl fmt::Display for ExportSize {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Original => "original",
            Self::Canvas => "canvas",
        })
    }
}

pub fn save(
    path: &Path,
    format: ExportFormat,
    size: ExportSize,
    canvas: &DrawingCanvas,
) -> Result<()> {
    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent).with_context(|| {
        format!(
            "failed to create temporary output beside {}",
            path.display()
        )
    })?;
    match format {
        ExportFormat::Png => {
            DynamicImage::ImageRgba8(render_image(size, canvas))
                .write_to(temporary.as_file_mut(), ImageFormat::Png)
                .with_context(|| format!("failed to encode PNG output {}", path.display()))?;
        }
        ExportFormat::Svg => {
            temporary
                .write_all(render_svg(size, canvas)?.as_bytes())
                .with_context(|| format!("failed to encode SVG output {}", path.display()))?;
        }
    }
    temporary.as_file_mut().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to replace output {}", path.display()))?;
    Ok(())
}

pub fn render_image(size: ExportSize, canvas: &DrawingCanvas) -> RgbaImage {
    if size == ExportSize::Original && matches!(canvas.source(), BaseSource::Image(_)) {
        canvas.render_original_export()
    } else {
        canvas.render_canvas_export()
    }
}

fn render_svg(size: ExportSize, canvas: &DrawingCanvas) -> Result<String> {
    if canvas.requires_raster_svg() {
        let image = render_image(size, canvas);
        return raster_svg(&image);
    }
    let (base, elements, sizing) = if size == ExportSize::Original {
        match (canvas.original_base(), canvas.original_scale()) {
            (Some(base), Some(scale)) => (
                base,
                canvas.transformed_active_elements(),
                canvas.sizing().scaled(scale),
            ),
            _ => (
                canvas.canvas_base().clone(),
                canvas.active_elements(),
                canvas.sizing(),
            ),
        }
    } else {
        (
            canvas.canvas_base().clone(),
            canvas.active_elements(),
            canvas.sizing(),
        )
    };
    vector_svg(&base, &elements, sizing)
}

fn raster_svg(image: &RgbaImage) -> Result<String> {
    let href = png_uri(image)?;
    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\" overflow=\"hidden\"><image href=\"{}\" width=\"{}\" height=\"{}\"/></svg>\n",
        image.width(),
        image.height(),
        image.width(),
        image.height(),
        href,
        image.width(),
        image.height()
    ))
}

fn vector_svg(base: &RgbaImage, elements: &[Element], sizing: RenderSizing) -> Result<String> {
    let width = base.width();
    let height = base.height();
    let mut output = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\" overflow=\"hidden\">\n<defs><style><![CDATA[@font-face{{font-family:'vvpaint Noto Sans';src:url(data:font/ttf;base64,{}) format('truetype');}}]]></style></defs>\n<image href=\"{}\" width=\"{width}\" height=\"{height}\"/>\n",
        BASE64_STANDARD.encode(annotation_font_bytes()),
        png_uri(base)?
    );
    for element in elements {
        write_element(&mut output, element, width, height, sizing);
    }
    output.push_str("</svg>\n");
    Ok(output)
}

fn write_element(
    output: &mut String,
    element: &Element,
    width: u32,
    height: u32,
    sizing: RenderSizing,
) {
    match element {
        Element::Stroke { points, style } => write_path(
            output,
            points,
            *style,
            width,
            height,
            sizing.radius(*style) * 2.0,
        ),
        Element::Highlighter { points, style } => write_path(
            output,
            points,
            *style,
            width,
            height,
            sizing.radius(*style) * 6.4,
        ),
        Element::Line { start, end, style } => write_line(
            output,
            *start,
            *end,
            *style,
            width,
            height,
            sizing.radius(*style) * 2.0,
        ),
        Element::Rectangle { start, end, style } => {
            let (x1, y1) = svg_point(*start, width, height);
            let (x2, y2) = svg_point(*end, width, height);
            output.push_str(&format!("<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" fill=\"none\" {} />\n", x1.min(x2), y1.min(y2), (x2-x1).abs(), (y2-y1).abs(), stroke(*style, sizing.radius(*style)*2.0)));
        }
        Element::Ellipse { start, end, style } => {
            let (x1, y1) = svg_point(*start, width, height);
            let (x2, y2) = svg_point(*end, width, height);
            output.push_str(&format!("<ellipse cx=\"{:.2}\" cy=\"{:.2}\" rx=\"{:.2}\" ry=\"{:.2}\" fill=\"none\" {} />\n", (x1+x2)*0.5, (y1+y2)*0.5, (x2-x1).abs()*0.5, (y2-y1).abs()*0.5, stroke(*style, sizing.radius(*style)*2.0)));
        }
        Element::Arrow { start, end, style } => {
            write_line(
                output,
                *start,
                *end,
                *style,
                width,
                height,
                sizing.radius(*style) * 2.0,
            );
            let (sx, sy) = svg_point(*start, width, height);
            let (ex, ey) = svg_point(*end, width, height);
            let length = (ex - sx).hypot(ey - sy);
            if length > 0.5 {
                let ux = (ex - sx) / length;
                let uy = (ey - sy) / length;
                let radius = sizing.radius(*style);
                let head = (radius * 7.0).max(8.0);
                let wing = (radius * 4.5).max(5.0);
                let bx = ex - ux * head;
                let by = ey - uy * head;
                output.push_str(&format!("<polygon points=\"{ex:.2},{ey:.2} {:.2},{:.2} {:.2},{:.2}\" fill=\"{}\" fill-opacity=\"{:.3}\"/>\n", bx-uy*wing,by+ux*wing,bx+uy*wing,by-ux*wing,color(style.color),style.opacity));
            }
        }
        Element::Text {
            position,
            text,
            style,
        } => {
            let (x, y) = svg_point(*position, width, height);
            output.push_str(&format!("<text x=\"{x:.2}\" y=\"{y:.2}\" fill=\"{}\" fill-opacity=\"{:.3}\" font-family=\"vvpaint Noto Sans, sans-serif\" font-size=\"{:.2}\" dominant-baseline=\"hanging\">{}</text>\n", color(style.color),style.opacity,sizing.text_size(*style),escape_xml(text)));
        }
        Element::Redaction { .. } | Element::FloodFill { .. } => {
            unreachable!("raster-only elements are handled before vector export")
        }
    }
}

fn write_path(
    output: &mut String,
    points: &[Point],
    style: Style,
    width: u32,
    height: u32,
    stroke_width: f32,
) {
    let Some(first) = points.first() else { return };
    let (x, y) = svg_point(*first, width, height);
    if points.len() == 1 {
        output.push_str(&format!("<circle cx=\"{x:.2}\" cy=\"{y:.2}\" r=\"{:.2}\" fill=\"{}\" fill-opacity=\"{:.3}\"/>\n",stroke_width*0.5,color(style.color),style.opacity));
        return;
    }
    let mut data = format!("M {x:.2} {y:.2}");
    for point in &points[1..] {
        let (x, y) = svg_point(*point, width, height);
        data.push_str(&format!(" L {x:.2} {y:.2}"));
    }
    output.push_str(&format!(
        "<path d=\"{data}\" fill=\"none\" {} />\n",
        stroke(style, stroke_width)
    ));
}

fn write_line(
    output: &mut String,
    start: Point,
    end: Point,
    style: Style,
    width: u32,
    height: u32,
    stroke_width: f32,
) {
    let (x1, y1) = svg_point(start, width, height);
    let (x2, y2) = svg_point(end, width, height);
    output.push_str(&format!(
        "<line x1=\"{x1:.2}\" y1=\"{y1:.2}\" x2=\"{x2:.2}\" y2=\"{y2:.2}\" {} />\n",
        stroke(style, stroke_width)
    ));
}

fn stroke(style: Style, width: f32) -> String {
    format!(
        "stroke=\"{}\" stroke-width=\"{width:.2}\" stroke-opacity=\"{:.3}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"",
        color(style.color),
        style.opacity
    )
}

fn svg_point(point: Point, width: u32, height: u32) -> (f32, f32) {
    (point.x * width as f32, point.y * height as f32)
}
fn color(value: Rgba<u8>) -> String {
    format!("#{:02x}{:02x}{:02x}", value[0], value[1], value[2])
}

fn escape_xml(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '&' => "&amp;".to_owned(),
            '<' => "&lt;".to_owned(),
            '>' => "&gt;".to_owned(),
            '"' => "&quot;".to_owned(),
            '\'' => "&apos;".to_owned(),
            other => other.to_string(),
        })
        .collect()
}

fn png_uri(image: &RgbaImage) -> Result<String> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image.clone()).write_to(&mut cursor, ImageFormat::Png)?;
    Ok(format!(
        "data:image/png;base64,{}",
        BASE64_STANDARD.encode(cursor.into_inner())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        canvas::{Point, Style, Tool, WidthPreset},
        theme::Theme,
    };

    fn temporary(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("vvpaint-{}-{name}", std::process::id()))
    }

    #[test]
    fn exports_vector_text_and_rasterizes_sensitive_operations() {
        let style = Style::opaque(Rgba([255, 0, 0, 255]), WidthPreset::Medium);
        let mut canvas = DrawingCanvas::blank(80, 40, Theme::Light);
        canvas.add_text(Point::new(0.2, 0.2), "A&B".into(), style);
        let vector = temporary("vector.svg");
        save(&vector, ExportFormat::Svg, ExportSize::Canvas, &canvas).unwrap();
        let svg = std::fs::read_to_string(&vector).unwrap();
        let _ = std::fs::remove_file(&vector);
        assert!(svg.contains("<text"));
        assert!(svg.contains("A&amp;B"));
        canvas.begin(Tool::Redaction, Point::new(0.1, 0.1), style);
        canvas.extend(Point::new(0.9, 0.9));
        canvas.finish();
        let raster = temporary("raster.svg");
        save(&raster, ExportFormat::Svg, ExportSize::Canvas, &canvas).unwrap();
        let svg = std::fs::read_to_string(&raster).unwrap();
        let _ = std::fs::remove_file(&raster);
        assert!(!svg.contains("<text"));
        assert!(!svg.contains("A&amp;B"));
    }
}
