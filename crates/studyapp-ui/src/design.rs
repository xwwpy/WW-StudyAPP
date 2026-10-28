//! Shared geometry for the application's visual hierarchy. Document surfaces
//! remain flat; cards, controls and compact rows each have one radius.
use gpui_kit::{Pixels, px};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeRole {
    Display,
    Title,
    Section,
    Body,
    Caption,
    Mono,
}

pub fn type_size(base: u16, role: TypeRole) -> Pixels {
    let base = base.clamp(12, 22) as f32;
    px(match role {
        TypeRole::Display => (base * 2.2).clamp(28., 44.),
        TypeRole::Title => (base * 1.45).clamp(18., 32.),
        TypeRole::Section => base * 1.15,
        TypeRole::Body => base,
        TypeRole::Caption => (base * 0.82).max(11.5),
        TypeRole::Mono => base * 0.93,
    })
}

pub const fn line_height(role: TypeRole) -> f32 {
    match role {
        TypeRole::Display => 1.1,
        TypeRole::Title => 1.3,
        TypeRole::Section => 1.4,
        TypeRole::Caption => 1.45,
        TypeRole::Body | TypeRole::Mono => 1.55,
    }
}

pub fn spacing_unit(base: u16) -> Pixels {
    px(base.clamp(4, 16) as f32)
}
pub fn space(base: u16, steps: f32) -> Pixels {
    spacing_unit(base) * steps
}

pub struct Motion;
impl Motion {
    pub const FAST: Duration = Duration::from_millis(80);
    pub const BASE: Duration = Duration::from_millis(140);
    pub const SLOW: Duration = Duration::from_millis(200);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Elevation {
    Border,
    ShadowSm,
    ShadowMd,
}

pub const UI_FONT: &str = "Segoe UI Variable Text";
pub const MONO_FONT: &str = "Cascadia Mono";

pub const CARD_RADIUS: f32 = 12.;
pub const CONTROL_RADIUS: f32 = 8.;
pub const ROW_RADIUS: f32 = 6.;
pub const OVERLAY_RADIUS: f32 = 8.;
pub const NAVIGATION_TINT_ALPHA: f32 = 0.88;

pub const TITLE_BAR_HEIGHT: f32 = 44.;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn type_hierarchy_is_readable_at_every_supported_scale() {
        for base in [12, 14, 18, 22] {
            let roles = [
                TypeRole::Display,
                TypeRole::Title,
                TypeRole::Section,
                TypeRole::Body,
                TypeRole::Caption,
            ];
            for pair in roles.windows(2) {
                assert!(type_size(base, pair[0]) > type_size(base, pair[1]));
            }
            assert!(type_size(base, TypeRole::Caption) >= px(11.5));
            assert!(type_size(base, TypeRole::Title) >= type_size(base, TypeRole::Body) * 1.4);
            assert_eq!(space(base, 2.), spacing_unit(base) * 2.);
        }
    }
}
