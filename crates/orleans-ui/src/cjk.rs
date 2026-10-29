//! Load a system CJK font as an egui fallback when Chinese is selected.

use std::sync::{Arc, OnceLock};

use eframe::egui::{Context, FontData, FontDefinitions, FontFamily};
use fontdb::{Family, Query, Stretch, Style, Weight};

struct CjkFont {
    data: Vec<u8>,
    index: u32,
}

fn find_font() -> Option<CjkFont> {
    let mut database = fontdb::Database::new();
    database.load_system_fonts();
    let normal = Query {
        families: &[],
        weight: Weight::NORMAL,
        stretch: Stretch::Normal,
        style: Style::Normal,
    };
    for family in [
        "Source Han Sans CN",
        "Noto Sans CJK SC",
        "Microsoft YaHei",
        "PingFang SC",
        "Droid Sans Fallback",
        "WenQuanYi Zen Hei",
        "SimSun",
        "Heiti SC",
    ] {
        let family = Family::Name(family);
        let query = Query {
            families: std::slice::from_ref(&family),
            ..normal
        };
        if let Some(id) = database.query(&query) {
            if let Some(font) = database.with_face_data(id, |data, index| CjkFont {
                data: data.to_vec(),
                index,
            }) {
                return Some(font);
            }
        }
    }
    // Last-resort scan for common CJK family names.
    for face in database.faces() {
        let cjk = face.families.iter().any(|(family, _)| {
            let family = family.to_ascii_lowercase();
            family.contains("cjk")
                || family.contains("han")
                || family.contains("hei")
                || family.contains("song")
                || family.contains("droid sans fallback")
        });
        if cjk {
            if let Some(font) = database.with_face_data(face.id, |data, index| CjkFont {
                data: data.to_vec(),
                index,
            }) {
                return Some(font);
            }
        }
    }
    None
}

/// Install one CJK fallback font, once. Returns true when a font was found.
pub fn install(ctx: &Context) -> bool {
    static FONT: OnceLock<Option<CjkFont>> = OnceLock::new();
    static APPLIED: OnceLock<()> = OnceLock::new();
    let Some(font) = FONT.get_or_init(find_font) else {
        return false;
    };
    if APPLIED.set(()).is_err() {
        return true;
    }
    let name = "orleans-cjk".to_owned();
    let mut definitions = FontDefinitions::default();
    definitions.font_data.insert(
        name.clone(),
        Arc::new(FontData {
            index: font.index,
            ..FontData::from_owned(font.data.clone())
        }),
    );
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        definitions
            .families
            .entry(family)
            .or_default()
            .push(name.clone());
    }
    ctx.set_fonts(definitions);
    true
}
