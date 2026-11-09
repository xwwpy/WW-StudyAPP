//! Original PDF/DOCX bytes are versioned with the library, not lossy text extraction.
use anyhow::{Context, Result, ensure};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::{io::Read, path::Path};

#[derive(Clone, Debug)]
pub(crate) struct ScanStamp {
    path: std::path::PathBuf,
    length: u64,
    modified: std::time::SystemTime,
    checked: i64,
}
impl ScanStamp {
    pub fn read(path: &Path, now: i64) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        Some(Self {
            path: path.into(),
            length: metadata.len(),
            modified: metadata.modified().ok()?,
            checked: now,
        })
    }
    pub fn unchanged(&self, other: &Self) -> bool {
        self.path == other.path
            && self.length == other.length
            && self.modified == other.modified
            && other.checked >= self.checked
            && other.checked.saturating_sub(self.checked) < 30
    }
}

pub const DOCUMENT_PREFIX: &str = "\u{001e}WW-STUDYAPP-DOCUMENT/1\n";
pub const MAX_DOCUMENT_BYTES: usize = 32 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentPreview {
    pub kind: String,
    pub data: String,
}
impl DocumentPreview {
    pub fn from_content(content: &str) -> Option<Self> {
        let body = content.strip_prefix(DOCUMENT_PREFIX)?;
        if body.len() > MAX_DOCUMENT_BYTES * 4 / 3 + 1024 {
            return None;
        }
        let value: Self = serde_json::from_str(body).ok()?;
        if !matches!(value.kind.as_str(), "pdf" | "docx") {
            return None;
        }
        Some(value)
    }
    /// Revalidate versioned envelopes too, not only files selected for import.
    pub fn validate(&self) -> Result<()> {
        validate_document(&self.kind, &self.bytes()?)
    }
    pub fn bytes(&self) -> Result<Vec<u8>> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&self.data)
            .context("文档数据编码无效")?;
        ensure!(bytes.len() <= MAX_DOCUMENT_BYTES, "文档超过 32 MiB");
        Ok(bytes)
    }
}
pub fn is_original_document(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| {
            value.eq_ignore_ascii_case("pdf") || value.eq_ignore_ascii_case("docx")
        })
}
pub fn supported_import(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| {
            [
                "md", "markdown", "pdf", "docx", "txt", "csv", "json", "yaml", "yml", "toml",
                "ini", "log", "rst", "html", "htm", "xml",
            ]
            .iter()
            .any(|extension| value.eq_ignore_ascii_case(extension))
        })
}
pub(crate) fn read_document(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).context("无法读取原文档")?;
    ensure!(
        file.metadata()?.len() <= MAX_DOCUMENT_BYTES as u64,
        "原文档超过 32 MiB，请拆分后导入"
    );
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_DOCUMENT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= MAX_DOCUMENT_BYTES, "原文档超过 32 MiB");
    let kind = if path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("pdf"))
    {
        "pdf"
    } else {
        "docx"
    };
    validate_document(kind, &bytes)?;
    let value = DocumentPreview {
        kind: kind.into(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    };
    Ok(format!(
        "{DOCUMENT_PREFIX}{}",
        serde_json::to_string(&value)?
    ))
}

fn validate_document(kind: &str, bytes: &[u8]) -> Result<()> {
    ensure!(bytes.len() <= MAX_DOCUMENT_BYTES, "原文档超过 32 MiB");
    if kind == "pdf" {
        ensure!(
            bytes.windows(5).take(1024).any(|part| part == b"%PDF-"),
            "不是有效的 PDF 文件"
        );
    } else {
        ensure!(kind == "docx", "不支持此原文档格式");
        let mut archive =
            zip::ZipArchive::new(std::io::Cursor::new(&bytes)).context("DOCX 压缩包无法读取")?;
        ensure!(archive.len() <= 5000, "DOCX 内部文件数量过多");
        let mut total = 0u64;
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index)?;
            let expected = entry.size();
            total = total.saturating_add(entry.size());
            ensure!(
                total <= 128 * 1024 * 1024 && entry.size() <= 32 * 1024 * 1024,
                "DOCX 解压内容过大"
            );
            let actual = std::io::copy(
                &mut entry.by_ref().take(32 * 1024 * 1024 + 1),
                &mut std::io::sink(),
            )
            .context("DOCX 内部文件损坏")?;
            ensure!(actual == expected, "DOCX 内部文件大小不一致");
        }
        ensure!(
            archive.by_name("[Content_Types].xml").is_ok()
                && archive.by_name("word/document.xml").is_ok(),
            "DOCX 缺少正文或格式信息"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn historical_envelopes_revalidate_before_office_rendering() {
        let value = DocumentPreview {
            kind: "docx".into(),
            data: base64::engine::general_purpose::STANDARD.encode(b"not a zip"),
        };
        let content = format!(
            "{DOCUMENT_PREFIX}{}",
            serde_json::to_string(&value).unwrap()
        );
        let parsed = DocumentPreview::from_content(&content).unwrap();
        assert!(parsed.validate().is_err());
        assert!(
            DocumentPreview {
                kind: "pdf".into(),
                data: base64::engine::general_purpose::STANDARD.encode(b"not a PDF")
            }
            .validate()
            .is_err()
        );
    }
    #[test]
    fn original_pdf_roundtrips_exact_bytes_without_changing_its_source() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("original.PDF");
        let bytes = b"%PDF-1.7\noriginal binary \xff\0 bytes\n%%EOF";
        std::fs::write(&path, bytes).unwrap();
        let content = read_document(&path).unwrap();
        let preview = DocumentPreview::from_content(&content).unwrap();
        assert_eq!(preview.kind, "pdf");
        assert_eq!(preview.bytes().unwrap(), bytes);
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    #[test]
    fn format_validation_rejects_fake_office_documents() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("fake.docx");
        std::fs::write(&path, "not a zip").unwrap();
        assert!(read_document(&path).is_err());
        assert!(!supported_import(Path::new("old.doc")));
        for extension in ["txt", "csv", "json", "yaml", "PDF", "DOCX"] {
            assert!(supported_import(Path::new(&format!("file.{extension}"))));
        }
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::{Command, Engine};
    #[test]
    fn original_versions_export_exact_bytes_and_survive_missing_sources() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("slides.pdf");
        let database = root.path().join("library.sqlite3");
        let original = b"%PDF-1.7\noriginal binary \0\xff\n%%EOF";
        std::fs::write(&source, original).unwrap();
        let mut engine = Engine::open(&database, 1000).unwrap();
        engine
            .apply(Command::Import(vec![source.clone()]), 1001)
            .unwrap();
        let snapshot = engine.snapshot();
        let note = snapshot.notes[0].id.clone();
        let version = snapshot.versions[0].id.clone();
        let export = root.path().join("exact.pdf");
        let command = Command::ExportNote {
            note_id: note.clone(),
            version_id: Some(version.clone()),
            path: export.clone(),
        };
        engine.apply(command.clone(), 1002).unwrap();
        assert_eq!(std::fs::read(&export).unwrap(), original);
        assert!(engine.apply(command, 1003).is_err());
        assert_eq!(std::fs::read(&source).unwrap(), original);
        std::fs::rename(&source, root.path().join("moved-source.pdf")).unwrap();
        drop(engine);
        let mut engine = Engine::open(&database, 1010).unwrap();
        assert!(engine.snapshot().notes[0].missing);
        let recovered = root.path().join("recovered.pdf");
        engine
            .apply(
                Command::ExportNote {
                    note_id: note,
                    version_id: Some(version),
                    path: recovered.clone(),
                },
                1011,
            )
            .unwrap();
        assert_eq!(std::fs::read(recovered).unwrap(), original);
    }
    #[test]
    fn docx_zip_is_bounded_and_roundtrips_without_extraction() {
        use std::io::Write as _;
        use zip::write::SimpleFileOptions;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("sample.docx");
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, body) in [
            ("[Content_Types].xml", "<Types/>"),
            ("word/document.xml", "<document>原文</document>"),
        ] {
            zip.start_file(name, SimpleFileOptions::default()).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        let bytes = zip.finish().unwrap().into_inner();
        std::fs::write(&path, &bytes).unwrap();
        let encoded = read_document(&path).unwrap();
        let document = DocumentPreview::from_content(&encoded).unwrap();
        assert_eq!(document.kind, "docx");
        assert_eq!(document.bytes().unwrap(), bytes);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }
    #[test]
    fn metadata_cache_expires_even_if_length_and_modified_time_are_unchanged() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("same.pdf");
        std::fs::write(&file, b"%PDF-1.7").unwrap();
        let first = ScanStamp::read(&file, 1000).unwrap();
        assert!(first.unchanged(&ScanStamp::read(&file, 1029).unwrap()));
        assert!(!first.unchanged(&ScanStamp::read(&file, 1030).unwrap()));
        assert!(!first.unchanged(&ScanStamp::read(&file, 999).unwrap()));
    }
}

#[cfg(test)]
mod text_import_tests {
    use crate::{Command, Engine};
    #[test]
    fn common_text_formats_and_windows_utf16_import_without_source_changes() {
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("library.sqlite3");
        let mut engine = Engine::open(&database, 1000).unwrap();
        let original = "中文 text\tcolumn\nnext line";
        let mut files = Vec::new();
        for extension in [
            "txt", "csv", "json", "yaml", "toml", "ini", "log", "rst", "html", "xml",
        ] {
            let path = root.path().join(format!("sample.{extension}"));
            std::fs::write(&path, original).unwrap();
            files.push(path);
        }
        let utf16 = root.path().join("windows.txt");
        let mut bytes = vec![0xff, 0xfe];
        for unit in original.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        std::fs::write(&utf16, &bytes).unwrap();
        files.push(utf16.clone());
        let result = engine.apply(Command::Import(files), 1001).unwrap();
        assert!(result.contains("成功 11"), "{result}");
        assert_eq!(engine.snapshot().notes.len(), 11);
        assert!(
            engine
                .snapshot()
                .versions
                .iter()
                .all(|version| version.content == original)
        );
        assert_eq!(std::fs::read(utf16).unwrap(), bytes);
    }
}
