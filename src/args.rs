use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use clap::{Parser, ValueEnum};

use crate::export::{ExportFormat, ExportSize};

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Paint and annotate through a Vivid 1.5 terminal presenter"
)]
pub struct Args {
    /// Optional raster image to annotate.
    pub input_image: Option<PathBuf>,

    /// Output path. Supports PNG and SVG.
    #[arg(short = 'o', long, value_parser = parse_output_path)]
    pub output: Option<PathBuf>,

    /// Output format. Inferred from --output, or PNG by default.
    #[arg(long, value_enum)]
    pub format: Option<ExportFormat>,

    /// Export at the imported image size or at the live canvas size.
    #[arg(long, value_enum)]
    pub export_size: Option<ExportSize>,

    /// Blank-canvas contrast theme.
    #[arg(long, value_enum, default_value_t = ThemeArg::Auto)]
    pub theme: ThemeArg,

    /// Raster backing scale relative to the presenter's physical canvas.
    #[arg(long, default_value_t = 0.5, value_parser = parse_resolution_scale)]
    pub resolution_scale: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ThemeArg {
    Auto,
    Dark,
    Light,
}

fn parse_output_path(value: &str) -> std::result::Result<PathBuf, String> {
    let path = PathBuf::from(value);
    ExportFormat::from_path(&path)
        .map(|_| path)
        .ok_or_else(|| "output file must have a .png or .svg extension".to_owned())
}

fn parse_resolution_scale(value: &str) -> std::result::Result<f32, String> {
    let scale = value
        .parse::<f32>()
        .map_err(|_| format!("invalid resolution scale: {value}"))?;
    if scale.is_finite() && (0.1..=1.0).contains(&scale) {
        Ok(scale)
    } else {
        Err("resolution scale must be between 0.1 and 1.0".to_owned())
    }
}

pub fn resolve_format(
    explicit: Option<ExportFormat>,
    output: Option<&Path>,
) -> Result<ExportFormat> {
    let inferred = output.and_then(ExportFormat::from_path);
    match (explicit, inferred) {
        (Some(value), Some(extension)) if value != extension => Err(anyhow!(
            "--format {} conflicts with output extension .{}",
            value,
            extension.extension()
        )),
        (Some(value), _) => Ok(value),
        (None, Some(value)) => Ok(value),
        (None, None) => Ok(ExportFormat::Png),
    }
}

pub fn default_export_size(input: Option<&Path>) -> ExportSize {
    if input.is_some() {
        ExportSize::Original
    } else {
        ExportSize::Canvas
    }
}

pub fn default_output_path(input: Option<&Path>, format: ExportFormat) -> PathBuf {
    let extension = format.extension();
    let Some(input) = input else {
        for index in 1.. {
            let candidate = PathBuf::from(format!("vvpaint{index}.{extension}"));
            if !candidate.exists() {
                return candidate;
            }
        }
        unreachable!();
    };
    let stem = input
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("image");
    let name = format!("{stem}-vvpaint.{extension}");
    match input.parent().filter(|path| !path.as_os_str().is_empty()) {
        Some(parent) => parent.join(&name),
        None => PathBuf::from(name),
    }
}

pub fn validate_output(path: &Path) -> Result<()> {
    if ExportFormat::from_path(path).is_none() {
        return Err(anyhow!("output file must have a .png or .svg extension"));
    }
    if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty())
        && !parent.is_dir()
    {
        return Err(anyhow!(
            "output parent is not a directory: {}",
            parent.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_public_cli() {
        let args = Args::try_parse_from([
            "vvpaint",
            "in.jpg",
            "-o",
            "out.svg",
            "--format",
            "svg",
            "--export-size",
            "original",
            "--theme",
            "light",
            "--resolution-scale",
            "0.25",
        ])
        .unwrap();
        assert_eq!(args.input_image.as_deref(), Some(Path::new("in.jpg")));
        assert_eq!(args.output.as_deref(), Some(Path::new("out.svg")));
        assert_eq!(args.resolution_scale, 0.25);
    }

    #[test]
    fn rejects_conflicting_format_and_bad_scale() {
        assert!(resolve_format(Some(ExportFormat::Svg), Some(Path::new("x.png"))).is_err());
        assert!(parse_resolution_scale("0.01").is_err());
    }
}
