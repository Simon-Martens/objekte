//! Semantic surface colors for Objekte's custom GPUI layout.
//!
//! GPUI Kit still owns the active component theme; these tokens keep the
//! application chrome coherent with its dark and light modes.


/// Colors for the workspace shell and data grid.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub canvas: u32,
    pub panel: u32,
    pub raised: u32,
    pub border: u32,
    pub text: u32,
    pub secondary: u32,
    pub selected: u32,
    pub accent: u32,
    pub row_alt: u32,
    pub error: u32,
}


/// Return the palette matching the stored GPUI Kit theme preference.
pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            canvas: 0x17191d,
            panel: 0x1c1f24,
            raised: 0x24282e,
            border: 0x30353c,
            text: 0xe5e7eb,
            secondary: 0xa0a7b1,
            selected: 0x293d55,
            accent: 0x8bb8f4,
            row_alt: 0x1a1d21,
            error: 0xf0a6a6,
        }
    } else {
        Palette {
            canvas: 0xf8fafc,
            panel: 0xffffff,
            raised: 0xf1f5f9,
            border: 0xd5dce5,
            text: 0x1f2937,
            secondary: 0x596579,
            selected: 0xdceafe,
            accent: 0x2563eb,
            row_alt: 0xf7f9fc,
            error: 0xb42318,
        }
    }
}
