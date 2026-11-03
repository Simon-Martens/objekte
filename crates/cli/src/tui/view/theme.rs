//! Colours. The terminal's own background shows everywhere except in
//! dialogs and on the selected row.

use ratatui::style::Color;

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
