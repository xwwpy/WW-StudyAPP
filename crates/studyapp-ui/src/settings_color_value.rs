//! Color draft rules, independent of GPUI and persistence.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorChoice {
    Automatic,
    Custom(u32),
    Invalid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColorDraft {
    raw: String,
    allow_automatic: bool,
}

impl ColorDraft {
    pub fn new(raw: impl Into<String>, allow_automatic: bool) -> Self {
        Self {
            raw: raw.into(),
            allow_automatic,
        }
    }

    pub fn raw(&self) -> &str {
        &self.raw
    }

    pub fn set_text(&mut self, raw: impl Into<String>) -> bool {
        let raw = raw.into();
        if raw == self.raw {
            return false;
        }
        self.raw = raw;
        true
    }

    pub fn choice(&self) -> ColorChoice {
        let value = self.raw.trim();
        if value.is_empty() && self.allow_automatic {
            return ColorChoice::Automatic;
        }
        let Some(hex) = value.strip_prefix('#') else {
            return ColorChoice::Invalid;
        };
        if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return ColorChoice::Invalid;
        }
        u32::from_str_radix(hex, 16)
            .map(ColorChoice::Custom)
            .unwrap_or(ColorChoice::Invalid)
    }

    pub fn choose_rgb(&mut self, rgb: u32) {
        self.raw = format!("#{:06X}", rgb & 0xFFFFFF);
    }

    pub fn use_automatic(&mut self) -> bool {
        if !self.allow_automatic {
            return false;
        }
        self.raw.clear();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{ColorChoice, ColorDraft};

    #[test]
    fn exact_opaque_hex_and_automatic_keep_existing_configuration_rules() {
        assert_eq!(
            ColorDraft::new(" #12AbEF ", false).choice(),
            ColorChoice::Custom(0x12ABEF)
        );
        assert_eq!(ColorDraft::new("", true).choice(), ColorChoice::Automatic);
        assert_eq!(ColorDraft::new("", false).choice(), ColorChoice::Invalid);
        for text in ["#abc", "#12345678", "123456", "#12GG00", "#"] {
            let draft = ColorDraft::new(text, true);
            assert_eq!(draft.choice(), ColorChoice::Invalid);
            assert_eq!(draft.raw(), text, "invalid hand input was destroyed");
        }
    }

    #[test]
    fn choosing_and_inheriting_only_modify_the_draft_value() {
        let mut draft = ColorDraft::new("#", true);
        draft.choose_rgb(0x1234AB);
        assert_eq!(draft.raw(), "#1234AB");
        assert_eq!(draft.choice(), ColorChoice::Custom(0x1234AB));
        assert!(draft.use_automatic());
        assert_eq!(draft.raw(), "");
        assert_eq!(draft.choice(), ColorChoice::Automatic);
        assert!(!draft.set_text(""));
        assert!(draft.set_text("#ABCDEF"));
        assert_eq!(draft.choice(), ColorChoice::Custom(0xABCDEF));
        let mut required = ColorDraft::new("#123456", false);
        assert!(!required.use_automatic());
        assert_eq!(required.raw(), "#123456");
    }
}
