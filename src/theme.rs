use image::Rgba;

use crate::args::ThemeArg;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
}

impl Theme {
    pub fn background(self) -> Rgba<u8> {
        match self {
            Self::Dark => Rgba([0, 0, 0, 255]),
            Self::Light => Rgba([255, 255, 255, 255]),
        }
    }

    pub fn foreground(self) -> Rgba<u8> {
        match self {
            Self::Dark => Rgba([255, 255, 255, 255]),
            Self::Light => Rgba([0, 0, 0, 255]),
        }
    }
}

pub fn resolve(value: ThemeArg) -> Theme {
    match value {
        ThemeArg::Dark => Theme::Dark,
        ThemeArg::Light => Theme::Light,
        ThemeArg::Auto => std::env::var("COLORFGBG")
            .ok()
            .and_then(|value| value.split(';').next_back()?.parse::<u8>().ok())
            .map(|background| {
                if matches!(background, 7 | 9..=15) {
                    Theme::Light
                } else {
                    Theme::Dark
                }
            })
            .unwrap_or(Theme::Dark),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_are_opposites() {
        assert_eq!(Theme::Dark.background(), Rgba([0, 0, 0, 255]));
        assert_eq!(Theme::Dark.foreground(), Rgba([255, 255, 255, 255]));
    }
}
