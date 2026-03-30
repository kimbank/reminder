use std::fs;

use eframe::egui::{Context, FontData, FontDefinitions, FontFamily};

use super::{CJK_FONT_NAME, SYSTEM_FONT_CANDIDATES};

pub(super) fn install_international_fonts(ctx: &Context) {
    let Some(font_data) = resolve_cjk_font_data() else {
        eprintln!("Warning: no CJK-capable font found; Some glyphs may fail to render.");
        return;
    };

    let mut definitions = FontDefinitions::default();
    definitions
        .font_data
        .insert(CJK_FONT_NAME.to_owned(), font_data.into());

    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        definitions
            .families
            .entry(family)
            .or_default()
            .insert(0, CJK_FONT_NAME.to_owned());
    }

    ctx.set_fonts(definitions);
}

fn resolve_cjk_font_data() -> Option<FontData> {
    load_system_cjk_font()
}

fn load_system_cjk_font() -> Option<FontData> {
    for candidate in SYSTEM_FONT_CANDIDATES {
        if let Ok(bytes) = fs::read(candidate) {
            return Some(FontData::from_owned(bytes));
        }
    }
    None
}
