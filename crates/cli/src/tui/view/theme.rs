//! Colours. The terminal's own background shows everywhere except in
//! dialogs and on the selected row.
//!
//! Views draw with the dark palette below. On light terminals the finished
//! frame is remapped to matching light colours, so no drawing code needs to
//! know the theme.

use ratatui::{buffer::Buffer, style::Color};
use std::sync::atomic::{AtomicBool, Ordering};

/// Default foreground: the terminal decides.
pub(super) const TEXT: Color = Color::Reset;
pub(super) const ACCENT: Color = Color::Rgb(122, 162, 247);
pub(super) const VIOLET: Color = Color::Rgb(187, 154, 247);
pub(super) const GREEN: Color = Color::Rgb(158, 206, 106);
pub(super) const RED: Color = Color::Rgb(247, 118, 142);
pub(super) const AMBER: Color = Color::Rgb(224, 175, 104);
pub(super) const TEAL: Color = Color::Rgb(125, 207, 255);
/// Secondary text: labels, units, field names.
pub(super) const DIM: Color = Color::Rgb(130, 137, 172);
/// Rules, hints, and placeholders.
pub(super) const FAINT: Color = Color::Rgb(78, 85, 120);
pub(super) const SELECTED: Color = Color::Rgb(41, 46, 70);
/// Dialog background.
pub(super) const PANEL: Color = Color::Rgb(30, 33, 50);
/// Text on an accent background.
pub(super) const BACKGROUND_ON: Color = Color::Rgb(26, 27, 38);
/// Background of highlighted search matches.
pub(super) const MATCH: Color = Color::Rgb(72, 62, 30);

/// Dark colours and their light-terminal counterparts.
const LIGHT: [(Color, Color); 13] = [
    (ACCENT, Color::Rgb(46, 92, 190)),
    (VIOLET, Color::Rgb(122, 68, 190)),
    (GREEN, Color::Rgb(52, 128, 40)),
    (RED, Color::Rgb(196, 40, 72)),
    (AMBER, Color::Rgb(160, 100, 10)),
    (TEAL, Color::Rgb(0, 116, 156)),
    (DIM, Color::Rgb(92, 98, 124)),
    (FAINT, Color::Rgb(158, 163, 184)),
    (SELECTED, Color::Rgb(218, 225, 245)),
    (PANEL, Color::Rgb(238, 240, 247)),
    (BACKGROUND_ON, Color::Rgb(255, 255, 255)),
    (MATCH, Color::Rgb(255, 228, 150)),
    (TEXT, TEXT),
];

static LIGHT_THEME: AtomicBool = AtomicBool::new(false);


pub(in super::super) fn set_light(light: bool) {
    LIGHT_THEME.store(light, Ordering::Relaxed);
}

pub(in super::super) fn is_light() -> bool {
    LIGHT_THEME.load(Ordering::Relaxed)
}

/// Guess the terminal background: `OBJEKTE_THEME=light|dark` wins, then
/// `COLORFGBG` (set by many terminals, e.g. `0;15` for dark on white).
pub(in super::super) fn detect_light() -> Option<bool> {
    if let Ok(theme) = std::env::var("OBJEKTE_THEME") {
        match theme.to_ascii_lowercase().as_str() {
            "light" => return Some(true),
            "dark" => return Some(false),
            _ => {}
        }
    }
    let colors = std::env::var("COLORFGBG").ok()?;
    let background = colors.rsplit(';').next()?.parse::<u8>().ok()?;
    Some(matches!(background, 7 | 9..=15))
}

/// Swap the dark palette for the light one in a drawn frame.
pub(super) fn remap(buffer: &mut Buffer) {
    if !is_light() {
        return;
    }
    let light = |color: Color| {
        LIGHT
            .iter()
            .find(|(dark, _)| *dark == color)
            .map_or(color, |(_, light)| *light)
    };
    for cell in &mut buffer.content {
        cell.fg = light(cell.fg);
        cell.bg = light(cell.bg);
    }
}
