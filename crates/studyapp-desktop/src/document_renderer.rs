//! Bounded, background-only PDF rasterization. No network, OCR or source writes.
use base64::Engine as _;
use hayro::{
    hayro_interpret::InterpreterSettings,
    hayro_syntax::{DecryptionError, LoadPdfError, Pdf},
};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use studyapp_core::documents::DocumentPreview;
use zeroize::Zeroizing;

static ACTIVE: AtomicBool = AtomicBool::new(false);
struct Permit;
impl Drop for Permit {
    fn drop(&mut self) {
        ACTIVE.store(false, Ordering::Release);
    }
}

pub fn render(
    document: &DocumentPreview,
    page: usize,
    width: u32,
    password: Zeroizing<String>,
) -> Value {
    if ACTIVE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return json!({"retry":true});
    }
    let _permit = Permit;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        render_inner(document, page, width, &password)
    }))
    .unwrap_or_else(
        |_| json!({"error":"此 PDF 的页面无法安全显示。请导出原文后使用其他阅读器查看。"}),
    )
}
fn render_inner(document: &DocumentPreview, page: usize, width: u32, password: &str) -> Value {
    if document.kind != "pdf" || !(96..=3200).contains(&width) || password.len() > 1024 {
        return json!({"error":"页面请求参数无效，请重新打开笔记。"});
    }
    let Ok(bytes) = document.bytes() else {
        return json!({"error":"原文档数据损坏，请从原文件重新导入。"});
    };
    let pdf = match Pdf::new_with_password(bytes, password) {
        Ok(pdf) => pdf,
        Err(LoadPdfError::Decryption(DecryptionError::PasswordProtected)) => {
            return json!({"passwordRequired":true});
        }
        Err(LoadPdfError::Decryption(_)) => {
            return json!({"error":"此 PDF 的加密方式不受支持或加密信息损坏。请导出原文后使用其他阅读器查看。"});
        }
        Err(_) => return json!({"error":"无法读取此 PDF 的页面结构，请确认原文件可正常打开。"}),
    };
    let pages = pdf.pages();
    if pages.is_empty() || pages.len() > 2000 {
        return json!({"error":"PDF 为空或超过 2,000 页，请拆分后导入。"});
    }
    if page == 0 {
        let sizes = pages
            .iter()
            .map(|page| {
                let (w, h) = page.render_dimensions();
                [w, h]
            })
            .collect::<Vec<_>>();
        if sizes.iter().any(|size| {
            size.iter()
                .any(|v| !v.is_finite() || *v <= 0. || *v > 100_000.)
        }) {
            return json!({"error":"PDF 页面尺寸无效或过大。"});
        }
        return json!({"pages":sizes});
    }
    let Some(page) = pages.get(page - 1) else {
        return json!({"error":"页码超出文档范围。"});
    };
    let (w, h) = page.render_dimensions();
    if !w.is_finite() || !h.is_finite() || w <= 0. || h <= 0. {
        return json!({"error":"此页尺寸无效。"});
    }
    // Bound both dimensions and the decoded canvas, including very tall pages.
    let scale = (width as f32 / w)
        .min(8192. / h)
        .min((8_000_000. / (w * h)).sqrt());
    if !scale.is_finite() || scale <= 0. || w * scale < 1. || h * scale < 1. {
        return json!({"error":"此页比例过于极端，无法生成预览。"});
    }
    let pixmap = hayro::render(
        page,
        &hayro::RenderCache::new(),
        &InterpreterSettings::default(),
        &hayro::RenderSettings::default(),
        &hayro::PixmapSettings {
            x_scale: scale,
            y_scale: scale,
            bg_color: hayro::vello_cpu::color::palette::css::WHITE,
        },
    );
    let Ok(png) = pixmap.into_png() else {
        return json!({"error":"此页图像无法生成，请导出原文后查看。"});
    };
    if png.len() > 16 * 1024 * 1024 {
        return json!({"error":"此页图像过大，请降低缩放后重试。"});
    }
    json!({"image":format!("data:image/png;base64,{}",base64::engine::general_purpose::STANDARD.encode(png))})
}

#[cfg(any(test, feature = "ui-test"))]
pub(crate) fn sample() -> DocumentPreview {
    let stream = "1 0 0 rg 10 10 80 80 re f";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Resources << >> >>"
            .to_owned(),
        format!(
            "<< /Length {} >>\nstream\n{stream}\nendstream",
            stream.len()
        ),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 200] /Rotate 90 /Resources << >> >>"
            .to_owned(),
    ];
    let mut bytes = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![0];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()).as_bytes());
    for offset in offsets.iter().skip(1) {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len()
        )
        .as_bytes(),
    );
    DocumentPreview {
        kind: "pdf".into(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_pdf_pages_render_colored_pixels_rotation_and_bounded_requests() {
        let document = sample();
        let original = document.data.clone();
        let metadata = render_inner(&document, 0, 200, "");
        assert_eq!(metadata["pages"], json!([[100., 100.], [200., 100.]]));
        let image = render_inner(&document, 1, 200, "");
        let encoded = image["image"]
            .as_str()
            .unwrap()
            .strip_prefix("data:image/png;base64,")
            .unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        let image = image::load_from_memory(&bytes).unwrap().to_rgb8();
        assert_eq!(image.dimensions(), (200, 200));
        let pixel = image.get_pixel(100, 100);
        assert!(
            pixel[0] > 240 && pixel[1] < 20 && pixel[2] < 20,
            "{pixel:?}"
        );
        assert!(render_inner(&document, 3, 200, "")["error"].is_string());
        assert!(render_inner(&document, 1, 3201, "")["error"].is_string());
        assert_eq!(document.data, original);
        assert!(
            render_inner(
                &DocumentPreview {
                    kind: "pdf".into(),
                    data: "not base64".into()
                },
                0,
                200,
                ""
            )["error"]
                .is_string()
        );
    }
}
