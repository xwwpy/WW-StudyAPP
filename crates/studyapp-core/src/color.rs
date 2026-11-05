//! Explicit unpremultiplied sRGB + alpha. Packed RGB and RGBA never alias.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RgbaColor {
    rgb: u32,
    alpha: u8,
}
impl RgbaColor {
    pub const fn opaque(rgb: u32) -> Self {
        Self::new(rgb, 255)
    }
    pub const fn new(rgb: u32, alpha: u8) -> Self {
        Self {
            rgb: rgb & 0xFFFFFF,
            alpha,
        }
    }
    pub const fn rgb(self) -> u32 {
        self.rgb
    }
    pub const fn alpha(self) -> u8 {
        self.alpha
    }
    /// GPUI/CSS RGBA ordering: RRGGBBAA, not ARGB or RGB.
    pub const fn packed_rgba(self) -> u32 {
        self.rgb << 8 | self.alpha as u32
    }
    pub fn to_hex(self) -> String {
        if self.alpha == 255 {
            format!("#{:06X}", self.rgb)
        } else {
            format!("#{:08X}", self.packed_rgba())
        }
    }
    /// Source-over an opaque actual backdrop, in CSS sRGB channel space.
    pub fn over(self, background: u32) -> u32 {
        let alpha = u32::from(self.alpha);
        let channel = |shift: u32| {
            (((self.rgb >> shift) & 255u32) * alpha
                + ((background >> shift) & 255u32) * (255u32 - alpha)
                + 127u32)
                / 255u32
        };
        channel(16) << 16 | channel(8) << 8 | channel(0)
    }
}
pub fn parse_rgba_color(value: &str) -> Option<RgbaColor> {
    let hex = value.strip_prefix('#')?;
    if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some(if hex.len() == 6 {
        RgbaColor::opaque(value)
    } else {
        RgbaColor::new(value >> 8, value as u8)
    })
}
#[cfg(test)]
mod tests {
    use super::{RgbaColor, parse_rgba_color};
    #[test]
    fn strict_hex_preserves_all_alpha_bytes_and_legacy_rgb() {
        assert_eq!(
            parse_rgba_color("#12abef"),
            Some(RgbaColor::opaque(0x12ABEF))
        );
        for alpha in 0..=255 {
            let color = RgbaColor::new(0x12ABEF, alpha);
            assert_eq!(parse_rgba_color(&color.to_hex()), Some(color));
        }
        assert_eq!(parse_rgba_color("#12abefFF").unwrap().to_hex(), "#12ABEF");
        for bad in [
            "#abc",
            "#1234567",
            "#123456789",
            "#12gg0080",
            "rgba(1,2,3,.5)",
            "#123456;url(x)",
            " #123456",
            "#123456\n",
        ] {
            assert_eq!(parse_rgba_color(bad), None, "{bad}");
        }
    }
    #[test]
    fn source_over_is_not_truncation_or_parent_opacity() {
        assert_eq!(RgbaColor::new(0xFF0000, 128).over(0xFFFFFF), 0xFF7F7F);
        assert_eq!(RgbaColor::new(0x112233, 0).over(0xABCDEF), 0xABCDEF);
        assert_eq!(RgbaColor::opaque(0x112233).over(0xABCDEF), 0x112233);
        assert_eq!(RgbaColor::new(0x112233, 128).packed_rgba(), 0x11223380);
    }
}
