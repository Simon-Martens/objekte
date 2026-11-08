//! Bundled fonts and the shared family choices used by desktop controls.

use std::borrow::Cow;

use gpui_kit::App;
use gpui_kit::component::Theme;

/// The family used for application chrome and ordinary record text.
pub const SANS: &str = "Source Sans 3";
/// The family used for expressions, source text, and fixed-width identifiers.
pub const MONO: &str = "Source Code Pro";

/// Register the pinned static fonts before constructing the first window.
pub fn register_fonts(cx: &mut App) -> Result<(), String> {
    let fonts: Vec<Cow<'static, [u8]>> = vec![
        Cow::Borrowed(include_bytes!(
            "../../assets/fonts/source-sans-3/SourceSans3-Regular.ttf"
        )),
        Cow::Borrowed(include_bytes!(
            "../../assets/fonts/source-sans-3/SourceSans3-Semibold.ttf"
        )),
        Cow::Borrowed(include_bytes!(
            "../../assets/fonts/source-sans-3/SourceSans3-Bold.ttf"
        )),
        Cow::Borrowed(include_bytes!(
            "../../assets/fonts/source-code-pro/SourceCodePro-Regular.ttf"
        )),
        Cow::Borrowed(include_bytes!(
            "../../assets/fonts/source-code-pro/SourceCodePro-Bold.ttf"
        )),
    ];
    cx.text_system()
        .add_fonts(fonts)
        .map_err(|error| format!("failed to register bundled fonts: {error}"))?;
    apply_theme_fonts(cx);
    Ok(())
}

/// Reapply families after GPUI Kit changes its light/dark theme configuration.
pub fn apply_theme_fonts(cx: &mut App) {
    Theme::update(cx, |theme| {
        theme.font_family = SANS.into();
        theme.mono_font_family = MONO.into();
    });
}
