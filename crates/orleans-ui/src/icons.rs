//! Compact icon buttons using Phosphor Icons.
//!
//! Phosphor is a clean, modern outline icon set available under MIT / Apache-2.0.
//! The font is installed as an egui fallback in `cjk::install`; keeping icons in
//! a font means they stay crisp at every scale and need no runtime SVG loader.

use eframe::egui::{self, Response, RichText, Vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Undo,
    Redo,
    Fit,
    Group,
    Ungroup,
    AlignLeft,
    AlignCenterH,
    AlignRight,
    AlignTop,
    AlignCenterV,
    AlignBottom,
    DistributeH,
    DistributeV,
    Duplicate,
    Delete,
    Up,
    Down,
    Union,
    Subtract,
    Intersect,
}

fn glyph(icon: Icon) -> &'static str {
    use egui_phosphor::regular as ph;
    match icon {
        Icon::Undo => ph::ARROW_COUNTER_CLOCKWISE,
        Icon::Redo => ph::ARROW_CLOCKWISE,
        Icon::Fit => ph::FRAME_CORNERS,
        Icon::Group => ph::SQUARES_FOUR,
        Icon::Ungroup => ph::SQUARE_SPLIT_VERTICAL,
        Icon::AlignLeft => ph::ALIGN_LEFT,
        Icon::AlignCenterH => ph::ALIGN_CENTER_HORIZONTAL,
        Icon::AlignRight => ph::ALIGN_RIGHT,
        Icon::AlignTop => ph::ALIGN_TOP,
        Icon::AlignCenterV => ph::ALIGN_CENTER_VERTICAL,
        Icon::AlignBottom => ph::ALIGN_BOTTOM,
        Icon::DistributeH => ph::ARROWS_OUT_LINE_HORIZONTAL,
        Icon::DistributeV => ph::ARROWS_OUT_LINE_VERTICAL,
        Icon::Duplicate => ph::COPY_SIMPLE,
        Icon::Delete => ph::TRASH,
        Icon::Up => ph::ARROW_UP,
        Icon::Down => ph::ARROW_DOWN,
        Icon::Union => ph::UNITE_SQUARE,
        Icon::Subtract => ph::SUBTRACT_SQUARE,
        Icon::Intersect => ph::INTERSECT_SQUARE,
    }
}

/// Compact square icon button with a localized tooltip.
pub fn button(ui: &mut egui::Ui, icon: Icon, size: f32, tooltip: &str, enabled: bool) -> Response {
    let text = RichText::new(glyph(icon)).size((size * 0.64).max(12.0));
    ui.add_enabled(enabled, egui::Button::new(text).min_size(Vec2::splat(size)))
        .on_hover_text(tooltip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boolean_icons_use_the_square_variants() {
        assert_eq!(glyph(Icon::Union), "\u{E878}");
        assert_eq!(glyph(Icon::Subtract), "\u{EBD4}");
        assert_eq!(glyph(Icon::Intersect), "\u{E87A}");
    }
}
