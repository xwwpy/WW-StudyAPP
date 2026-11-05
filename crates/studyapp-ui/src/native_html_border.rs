//! Rounded CSS dotted outlines; native quad borders only provide solid/dashed.
use gpui_kit::{Bounds, Hsla, Pixels, Window, fill, point, px, size};

pub(super) fn paint(
    bounds: Bounds<Pixels>,
    width: Pixels,
    radius: Pixels,
    color: Hsla,
    allowance: usize,
    window: &mut Window,
) -> usize {
    let thickness = width
        .min(bounds.size.width / 2.)
        .min(bounds.size.height / 2.);
    if thickness <= px(0.) || allowance == 0 {
        return 0;
    }
    let half = thickness / 2.;
    let inner = Bounds::new(
        bounds.origin + point(half, half),
        bounds.size - size(thickness, thickness),
    );
    let r = (radius - half)
        .max(px(0.))
        .min(inner.size.width / 2.)
        .min(inner.size.height / 2.);
    let horizontal = inner.size.width - 2. * r;
    let vertical = inner.size.height - 2. * r;
    let arc = r * std::f32::consts::FRAC_PI_2;
    let perimeter = 2. * (horizontal + vertical) + 4. * arc;
    let count = (perimeter / (thickness * 2.)).floor().clamp(4., 2048.) as usize;
    let count = count.min(allowance);
    for i in 0..count {
        let mut d = perimeter * (i as f32 / count as f32);
        let mut center = inner.origin;
        for side in 0..4 {
            let straight = if side % 2 == 0 { horizontal } else { vertical };
            if d <= straight {
                center = match side {
                    0 => point(inner.left() + r + d, inner.top()),
                    1 => point(inner.right(), inner.top() + r + d),
                    2 => point(inner.right() - r - d, inner.bottom()),
                    _ => point(inner.left(), inner.bottom() - r - d),
                };
                break;
            }
            d -= straight;
            if d <= arc && r > px(0.) {
                let origin = match side {
                    0 => point(inner.right() - r, inner.top() + r),
                    1 => point(inner.right() - r, inner.bottom() - r),
                    2 => point(inner.left() + r, inner.bottom() - r),
                    _ => point(inner.left() + r, inner.top() + r),
                };
                let angle = (side as f32 - 1.) * std::f32::consts::FRAC_PI_2 + (d / r);
                center = origin + point(r * angle.cos(), r * angle.sin());
                break;
            }
            d -= arc;
        }
        window.paint_quad(
            fill(
                Bounds::new(center - point(half, half), size(thickness, thickness)),
                color,
            )
            .corner_radii(half),
        );
    }
    count
}
