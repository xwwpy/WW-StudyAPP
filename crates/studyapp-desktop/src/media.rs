//! Bounded image ownership. External sources and retained staged files are never removed.
use anyhow::{Context, Result, ensure};
use base64::Engine as _;
use pulldown_cmark::{Event, LinkType, Options, Parser, Tag};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read, Write},
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use studyapp_core::{ImageSettings, ImageStorage};
use studyapp_sync::AssetProvider;

fn remote_image_url(value: &str) -> Result<reqwest::Url> {
    ensure!(value.len() <= 8192, "图片网址过长");
    let url = reqwest::Url::parse(value).context("图片网址无效")?;
    ensure!(
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none(),
        "网络图片仅支持不含用户名和密码的 HTTPS 地址"
    );
    Ok(url)
}
fn bounded_image_bytes(reader: impl Read) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    validated_image(&bytes)?;
    Ok(bytes)
}
fn fetch_remote_image(value: &str, timeout: Duration) -> Result<Vec<u8>> {
    let url = remote_image_url(value)?;
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() > 3 || remote_image_url(attempt.url().as_str()).is_err() {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(timeout.min(Duration::from_secs(15)))
        .referer(false)
        .no_proxy()
        .build()
        .context("图片网络连接无法建立")?;
    // This client has no cookie store, authorization, provider credentials, or referrer.
    let response = client.get(url).send().context("网络图片下载失败")?;
    ensure!(
        response.status().as_u16() == 200,
        "网络图片服务器未返回可读取图片"
    );
    ensure!(
        response
            .content_length()
            .is_none_or(|n| n <= MAX_IMAGE_BYTES as u64),
        "网络图片超过 10 MiB"
    );
    bounded_image_bytes(response)
}

pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const MAX_BATCH_BYTES: usize = 64 * 1024 * 1024;
const MAX_IMAGE_OPERATIONS: usize = 200;
const PREFIX: &str = "studyapp-media:";
const MAX_PREVIEW_CACHE_BYTES: usize = 32 * 1024 * 1024;
struct CacheEntry {
    url: String,
    expires: Instant,
    tick: u64,
    cost: usize,
}
#[derive(Default)]
struct PreviewCache {
    entries: BTreeMap<String, CacheEntry>,
    bytes: usize,
    tick: u64,
}
impl PreviewCache {
    fn get(&mut self, key: &str, now: Instant) -> Option<String> {
        if self
            .entries
            .get(key)
            .is_some_and(|entry| entry.expires <= now)
            && let Some(entry) = self.entries.remove(key)
        {
            self.bytes -= entry.cost;
        }
        self.tick = self.tick.saturating_add(1);
        let entry = self.entries.get_mut(key)?;
        entry.tick = self.tick;
        Some(entry.url.clone())
    }
    fn insert(&mut self, key: String, url: String, now: Instant, budget: usize) {
        let cost = key.len() + url.len() + 192;
        if cost > budget {
            return;
        }
        if let Some(old) = self.entries.remove(&key) {
            self.bytes -= old.cost;
        }
        while self.bytes + cost > budget {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.tick)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(old) = self.entries.remove(&oldest) {
                self.bytes -= old.cost;
            }
        }
        self.tick = self.tick.saturating_add(1);
        self.bytes += cost;
        self.entries.insert(
            key,
            CacheEntry {
                url,
                expires: now + Duration::from_secs(300),
                tick: self.tick,
                cost,
            },
        );
    }
}
fn cached_data_url(
    key: Option<&str>,
    control: Option<&ResolveControl>,
    load: impl FnOnce() -> Result<String>,
) -> Result<String> {
    static CACHE: OnceLock<Mutex<PreviewCache>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(PreviewCache::default()));
    if let Some(key) = key
        && let Some(url) = cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(key, Instant::now())
    {
        return Ok(url);
    }
    // No network, file access, raster decode, or base64 work holds the cache mutex.
    let url = load()?;
    if let Some(control) = control {
        control.remaining()?;
    }
    if let Some(key) = key {
        cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                key.to_owned(),
                url.clone(),
                Instant::now(),
                MAX_PREVIEW_CACHE_BYTES,
            );
    }
    Ok(url)
}
fn preview_cache_key(
    database: &Path,
    settings: &ImageSettings,
    dest: &str,
    source: Option<&Path>,
) -> Option<String> {
    if dest
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
    {
        return remote_image_url(dest)
            .ok()
            .map(|url| format!("https:{url}"));
    }
    let path = if dest.starts_with(PREFIX) {
        local_asset(database, settings, &asset_key(dest).ok()?).ok()?
    } else {
        let decoded = percent_encoding::percent_decode_str(dest)
            .decode_utf8()
            .ok()?;
        let path = Path::new(decoded.as_ref());
        if path.is_absolute() {
            path.to_owned()
        } else {
            source?.parent()?.join(path)
        }
    };
    let path = std::fs::canonicalize(path).ok()?;
    let metadata = std::fs::metadata(&path).ok()?;
    Some(format!(
        "file:{}:{}:{:?}",
        path.to_string_lossy(),
        metadata.len(),
        metadata.modified().ok()?
    ))
}

#[derive(Clone, Debug)]
pub enum MediaInput {
    Path(PathBuf),
    Bytes(Vec<u8>),
    Encoded(String),
}
fn decode_image_input(encoded: &str) -> Result<Vec<u8>> {
    ensure!(
        encoded.len() <= MAX_IMAGE_BYTES.div_ceil(3) * 4,
        "图片超过 10 MiB"
    );
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .context("剪贴板图片编码无效")?;
    validated_image(&bytes)?;
    Ok(bytes)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaAdoptionPolicy {
    PreserveManaged,
    CopyToGroup,
}

#[derive(Clone, Debug)]
pub enum MediaRequest {
    ResolveRetained {
        control: ResolveControl,
        generation: u64,
        request: u64,
        content: String,
        note_path: Option<PathBuf>,
        settings: ImageSettings,
        bindings: BTreeMap<String, String>,
    },
    Resolve {
        control: ResolveControl,
        generation: u64,
        request: u64,
        content: String,
        note_path: Option<PathBuf>,
        settings: ImageSettings,
    },
    Import {
        generation: u64,
        request: u64,
        group: String,
        title: String,
        input: MediaInput,
        settings: ImageSettings,
    },
    Adopt {
        policy: MediaAdoptionPolicy,
        generation: u64,
        request: u64,
        group: String,
        title: String,
        content: String,
        source_path: Option<PathBuf>,
        settings: ImageSettings,
    },
    Retain {
        generation: u64,
        request: u64,
        group: String,
        title: String,
        content: String,
        source_path: Option<PathBuf>,
        settings: ImageSettings,
    },
}
/// Shared with the UI so obsolete requests stop without losing their completion.
#[derive(Clone, Debug)]
pub struct ResolveControl {
    cancelled: Arc<AtomicBool>,
    deadline: Instant,
}
impl Default for ResolveControl {
    fn default() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            deadline: Instant::now() + Duration::from_secs(30),
        }
    }
}
impl ResolveControl {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
    fn remaining(&self) -> Result<Duration> {
        ensure!(!self.is_cancelled(), "图片预览已取消");
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
            .context("图片预览超过 30 秒，请重试未加载的图片")
    }
}
impl MediaRequest {
    pub fn generation(&self) -> u64 {
        match self {
            Self::Resolve { generation, .. }
            | Self::ResolveRetained { generation, .. }
            | Self::Retain { generation, .. }
            | Self::Import { generation, .. }
            | Self::Adopt { generation, .. } => *generation,
        }
    }
    pub fn request(&self) -> u64 {
        match self {
            Self::Resolve { request, .. }
            | Self::ResolveRetained { request, .. }
            | Self::Retain { request, .. }
            | Self::Import { request, .. }
            | Self::Adopt { request, .. } => *request,
        }
    }
    pub fn settings(&self) -> &ImageSettings {
        match self {
            Self::Resolve { settings, .. }
            | Self::ResolveRetained { settings, .. }
            | Self::Import { settings, .. }
            | Self::Adopt { settings, .. }
            | Self::Retain { settings, .. } => settings,
        }
    }
}
#[derive(Debug)]
pub struct MediaResponse {
    pub library: String,
    pub generation: u64,
    pub request: u64,
    pub result: std::result::Result<MediaOutcome, String>,
}
#[derive(Debug)]
pub enum MediaOutcome {
    Retained {
        bindings: BTreeMap<String, String>,
        missing: Vec<String>,
    },
    Resolved {
        images: BTreeMap<String, String>,
        missing: Vec<String>,
    },
    Inserted {
        markdown: String,
    },
    Adopted {
        content: String,
        missing: Vec<String>,
    },
}
pub fn new_group() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub(crate) fn asset_group(note_id: Option<&str>, _content: &str) -> String {
    if let Some(id) =
        note_id.filter(|id| uuid::Uuid::parse_str(id).is_ok_and(|v| v.to_string() == *id))
    {
        return id.to_owned();
    }
    new_group()
}

fn validated_image(bytes: &[u8]) -> Result<(&'static str, &'static str)> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_IMAGE_BYTES,
        "图片须小于 10 MiB"
    );
    let format = image::guess_format(bytes).context("图片格式无法识别")?;
    let kind = match format {
        image::ImageFormat::Png => ("png", "image/png"),
        image::ImageFormat::Jpeg => ("jpg", "image/jpeg"),
        image::ImageFormat::Gif => ("gif", "image/gif"),
        image::ImageFormat::WebP => ("webp", "image/webp"),
        image::ImageFormat::Bmp => ("bmp", "image/bmp"),
        _ => anyhow::bail!("仅支持 PNG、JPEG、GIF、WebP、BMP 图片"),
    };
    let (width, height) = image::ImageReader::with_format(Cursor::new(bytes), format)
        .into_dimensions()
        .context("图片损坏")?;
    ensure!(
        width > 0
            && height > 0
            && width <= 16384
            && height <= 16384
            && u64::from(width) * u64::from(height) <= 16_000_000,
        "图片尺寸超过限制"
    );
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(64 * 1024 * 1024);
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    reader.limits(limits);
    reader.decode().context("图片损坏或解码容量超过限制")?;
    Ok(kind)
}
fn read_file(path: &Path) -> Result<Vec<u8>> {
    let absolute = std::path::absolute(path)?;
    // Relative external images may legitimately contain `..`; normalize before guarding.
    let canonical = std::fs::canonicalize(&absolute).context("图片文件不存在")?;
    let _guards = crate::storage_backend::guard_directories(
        canonical.parent().context("图片目录不存在")?,
        false,
    )?;
    let file = crate::storage_backend::plain_handle(&canonical, false, true)?;
    ensure!(
        file.metadata()?.len() <= MAX_IMAGE_BYTES as u64,
        "图片超过 10 MiB"
    );
    let mut bytes = Vec::new();
    file.take((MAX_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    validated_image(&bytes)?;
    Ok(bytes)
}
fn asset_key(reference: &str) -> Result<String> {
    let tail = reference.strip_prefix(PREFIX).context("不是托管图片引用")?;
    let key = format!("assets/{tail}");
    studyapp_sync::validate_asset_key(&key)?;
    Ok(key)
}
fn image_root(database: &Path, settings: &ImageSettings) -> Result<PathBuf> {
    studyapp_core::media_settings::validate_images(settings)?;
    Ok(settings.root.as_ref().map(PathBuf::from).unwrap_or(
        database
            .parent()
            .context("资料库目录不存在")?
            .join("images"),
    ))
}
fn index_path(database: &Path, key: &str) -> Result<PathBuf> {
    studyapp_sync::validate_asset_key(key)?;
    Ok(database
        .parent()
        .context("资料库目录不存在")?
        .join("image-index")
        .join(key.strip_prefix("assets/").unwrap())
        .with_extension(format!(
            "{}.json",
            Path::new(key).extension().unwrap().to_string_lossy()
        )))
}
fn local_asset(database: &Path, settings: &ImageSettings, key: &str) -> Result<PathBuf> {
    let current =
        image_root(database, settings)?.join(key.strip_prefix("assets/").context("图片引用无效")?);
    // A migrated library carries its images directory; prefer that local copy
    // before the retained absolute index path in the previous library.
    if current.try_exists()? {
        return Ok(current);
    }
    let index = index_path(database, key)?;
    if index.try_exists()? {
        let _guards = crate::storage_backend::guard_directories(
            index.parent().context("图片索引目录不存在")?,
            false,
        )?;
        let file = crate::storage_backend::plain_handle(&index, false, true)?;
        ensure!(file.metadata()?.len() <= 32_768, "图片索引过大");
        let path: PathBuf = serde_json::from_reader(file)?;
        return Ok(path);
    }
    Ok(current)
}
fn publish_local(
    database: &Path,
    settings: &ImageSettings,
    key: &str,
    bytes: &[u8],
) -> Result<PathBuf> {
    let path =
        image_root(database, settings)?.join(key.strip_prefix("assets/").context("图片引用无效")?);
    let _guards =
        crate::storage_backend::guard_directories(path.parent().context("图片目录不存在")?, true)?;
    atomic_publish(&path, bytes)?;
    let index = index_path(database, key)?;
    let _index_guards = crate::storage_backend::guard_directories(index.parent().unwrap(), true)?;
    if !index.try_exists()? {
        atomic_publish(&index, &serde_json::to_vec(&path)?)?;
    }
    Ok(path)
}
fn atomic_publish(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("图片目录不存在")?;
    let _guards = crate::storage_backend::guard_directories(parent, true)?;
    let staging = parent.join(format!(".studyapp-image-{}.pending", new_group()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    // Retain staging on success and failure; create-only publication cannot expose partial bytes.
    std::fs::hard_link(&staging, path).context("图片发布失败；已保留完整暂存文件")?;
    #[cfg(unix)]
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}
fn load_image(
    database: &Path,
    settings: &ImageSettings,
    dest: &str,
    source: Option<&Path>,
    provider: Option<&dyn AssetProvider>,
    control: Option<&ResolveControl>,
) -> Result<Vec<u8>> {
    if let Some(control) = control {
        control.remaining()?;
    }
    if dest.starts_with(PREFIX) {
        let key = asset_key(dest)?;
        let path = local_asset(database, settings, &key)?;
        if path.try_exists()? {
            match read_file(&path) {
                Ok(bytes) => return Ok(bytes),
                Err(error) if provider.is_none() => return Err(error),
                Err(_) => {}
            }
        }
        // Provider reads are already bounded to 20s. Do not start one that can
        // outlive this batch; the next request can retry uncached images.
        if let Some(control) = control {
            ensure!(
                control.remaining()? >= Duration::from_secs(20),
                "图片预览剩余时间不足，请重试未缓存图片"
            );
        }
        let bytes = provider
            .context("云端图片尚未缓存，请配置同步服务并连接后重试")?
            .get_asset(&key)?
            .context("云端图片不存在")?;
        validated_image(&bytes)?;
        // A cancelled request must not publish downloaded bytes into storage.
        if let Some(control) = control {
            control.remaining()?;
        }
        // Legacy interrupted writes may have left a partial final file. Preserve
        // it under a unique retained name before publishing validated recovery bytes.
        if path.try_exists()? && read_file(&path).is_err() {
            let _guards = crate::storage_backend::guard_directories(
                path.parent().context("图片目录不存在")?,
                false,
            )?;
            let _file = crate::storage_backend::plain_handle(&path, false, false)?;
            std::fs::rename(
                &path,
                path.parent()
                    .unwrap()
                    .join(format!(".studyapp-image-{}.invalid", new_group())),
            )?;
        }
        // Cache is immutable; concurrent resolves may already have published it.
        if let Err(error) = publish_local(database, settings, &key, &bytes)
            && !path.try_exists()?
        {
            return Err(error);
        }
        return Ok(bytes);
    }
    if dest
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
        || dest
            .get(..7)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
    {
        let timeout = control
            .map(ResolveControl::remaining)
            .transpose()?
            .unwrap_or(Duration::from_secs(15));
        return fetch_remote_image(dest, timeout);
    }
    let decoded = percent_encoding::percent_decode_str(dest)
        .decode_utf8()
        .context("图片路径编码无效")?;
    ensure!(
        !decoded.contains('\0') && !decoded.contains('\r') && !decoded.contains('\n'),
        "图片路径无效"
    );
    ensure!(
        !decoded.starts_with("//")
            && !decoded.contains("://")
            && !decoded.starts_with("data:")
            && !decoded.starts_with("javascript:"),
        "网络图片尚未导入；请下载后插入图片"
    );
    let relative = Path::new(decoded.as_ref());
    let path = if relative.is_absolute() {
        relative.to_path_buf()
    } else {
        source
            .and_then(Path::parent)
            .context("请先保存笔记或导入图片")?
            .join(relative)
    };
    read_file(&path)
}
fn store_image(
    database: &Path,
    settings: &ImageSettings,
    group: &str,
    title: &str,
    bytes: &[u8],
    provider: Option<&dyn AssetProvider>,
) -> Result<String> {
    let (extension, _) = validated_image(bytes)?;
    ensure!(
        uuid::Uuid::parse_str(group).is_ok_and(|id| id.to_string() == group),
        "图片分组无效"
    );
    let title: String = title
        .chars()
        .take(40)
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let filename = format!(
        "{}-{}-{}.{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%S%3f"),
        if title.is_empty() { "note" } else { &title },
        uuid::Uuid::new_v4().simple(),
        extension
    );
    let key = format!("assets/{group}/{filename}");
    studyapp_sync::validate_asset_key(&key)?;
    let staged = publish_local(database, settings, &key, bytes)?;
    if settings.storage != ImageStorage::Local {
        (|| -> Result<()> {
            provider
                .context("请先配置可用的 WebDAV 或 NAS 同步目标")?
                .put_asset(&key, bytes)?;
            if let Some(provider) = provider {
                record_uploaded(database, provider, &key, bytes)?;
            }
            Ok(())
        })()
        .with_context(|| {
            format!(
                "图片上传失败，已保留本地待重试文件：{}",
                studyapp_desktop::presentation::display_path(&staged.to_string_lossy())
            )
        })?;
    }
    Ok(format!("{PREFIX}{group}/{filename}"))
}

#[derive(Clone)]
struct ImageUse {
    destination: String,
    range: Range<usize>,
    image_range: Range<usize>,
    definition: Option<Range<usize>>,
    title: String,
}
fn label_end(raw: &str, open: usize) -> Result<usize> {
    let bytes = raw.as_bytes();
    ensure!(bytes.get(open) == Some(&b'['), "图片标签无效");
    let mut level = 1;
    let mut at = open + 1;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => {
                at += 2;
                continue;
            }
            b'[' => level += 1,
            b']' => {
                level -= 1;
                if level == 0 {
                    return Ok(at);
                }
            }
            _ => {}
        }
        at += 1;
    }
    anyhow::bail!("图片标签没有结束")
}
fn destination_range(text: &str, range: Range<usize>, reference: bool) -> Result<Range<usize>> {
    let raw = &text[range.clone()];
    let mut at = if reference {
        let end = label_end(raw, raw.find('[').context("图片引用定义无效")?)?;
        ensure!(
            raw.as_bytes().get(end + 1) == Some(&b':'),
            "图片引用定义无效"
        );
        end + 2
    } else {
        let end = label_end(raw, 1)?;
        ensure!(
            raw.as_bytes().get(end + 1) == Some(&b'('),
            "图片目标位置无法定位"
        );
        end + 2
    };
    let bytes = raw.as_bytes();
    while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
        at += 1;
    }
    if bytes.get(at) == Some(&b'<') {
        let start = at + 1;
        let mut end = start;
        while end < bytes.len() {
            if bytes[end] == b'\\' {
                end += 2;
                continue;
            }
            if bytes[end] == b'>' {
                return Ok(range.start + start..range.start + end);
            }
            end += 1;
        }
        anyhow::bail!("图片目标位置无效");
    }
    let start = at;
    let mut depth = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => {
                at += 2;
                continue;
            }
            b'(' => depth += 1,
            b')' if depth > 0 => depth -= 1,
            b')' => break,
            c if c.is_ascii_whitespace() => break,
            _ => {}
        }
        at += 1;
    }
    Ok(range.start + start..range.start + at)
}
fn image_uses(text: &str) -> Result<Vec<ImageUse>> {
    ensure!(text.len() <= 10 * 1024 * 1024, "笔记超过图片解析容量限制");
    let definition_parser = Parser::new_ext(text, Options::all());
    let definitions = definition_parser.reference_definitions();
    let parser = Parser::new_ext(text, Options::all());
    let mut uses = Vec::new();
    for (event, span) in parser.into_offset_iter() {
        if let Event::Start(Tag::Image {
            link_type,
            dest_url,
            id,
            title,
        }) = event
        {
            let definition = if link_type == LinkType::Inline {
                None
            } else {
                Some(
                    definitions
                        .get(id.as_ref())
                        .context("图片引用定义不存在")?
                        .span
                        .clone(),
                )
            };
            let range = destination_range(
                text,
                definition.clone().unwrap_or_else(|| span.clone()),
                definition.is_some(),
            )?;
            uses.push(ImageUse {
                destination: dest_url.into_string(),
                range,
                image_range: span,
                definition,
                title: title.into_string(),
            });
        }
    }
    Ok(uses)
}
pub fn rewrite_images(text: &str, replacements: &BTreeMap<String, String>) -> Result<String> {
    let definition_parser = Parser::new_ext(text, Options::all());
    let definitions = definition_parser.reference_definitions();
    let parser = Parser::new_ext(text, Options::all());
    let mut shared = BTreeSet::new();
    for event in parser {
        if let Event::Start(Tag::Link { link_type, id, .. }) = event
            && link_type != LinkType::Inline
            && let Some(def) = definitions.get(id.as_ref())
        {
            shared.insert((def.span.start, def.span.end));
        }
    }
    let mut edits = BTreeMap::new();
    for image in image_uses(text)? {
        if let Some(replacement) = replacements.get(&image.destination) {
            if image
                .definition
                .as_ref()
                .is_some_and(|span| shared.contains(&(span.start, span.end)))
            {
                let raw = &text[image.image_range.clone()];
                let end = label_end(raw, 1)?;
                let title = if image.title.is_empty() {
                    String::new()
                } else {
                    format!(
                        " \"{}\"",
                        image.title.replace('\\', "\\\\").replace('"', "\\\"")
                    )
                };
                edits.insert(
                    (image.image_range.start, image.image_range.end),
                    format!("{}({replacement}{title})", &raw[..=end]),
                );
            } else {
                edits.insert((image.range.start, image.range.end), replacement.clone());
            }
        }
    }
    let mut result = text.to_owned();
    for ((start, end), value) in edits.into_iter().rev() {
        result.replace_range(start..end, &value);
    }
    Ok(result)
}
pub fn execute(
    database: &Path,
    request: MediaRequest,
    provider: Option<Box<dyn AssetProvider>>,
) -> Result<MediaOutcome> {
    let mut bindings = BTreeMap::new();
    let mut retain_only = false;
    let request = match request {
        MediaRequest::ResolveRetained {
            control,
            generation,
            request,
            content,
            note_path,
            settings,
            bindings: retained,
        } => {
            bindings = retained;
            MediaRequest::Resolve {
                control,
                generation,
                request,
                content,
                note_path,
                settings,
            }
        }
        MediaRequest::Retain {
            generation,
            request,
            group,
            title,
            content,
            source_path,
            mut settings,
        } => {
            retain_only = true;
            settings.storage = ImageStorage::Local;
            MediaRequest::Adopt {
                policy: MediaAdoptionPolicy::PreserveManaged,
                generation,
                request,
                group,
                title,
                content,
                source_path,
                settings,
            }
        }
        request => request,
    };
    match request {
        MediaRequest::Import {
            group,
            title,
            input,
            settings,
            ..
        } => {
            let bytes = match input {
                MediaInput::Path(path) => read_file(&path)?,
                MediaInput::Bytes(bytes) => bytes,
                MediaInput::Encoded(encoded) => decode_image_input(&encoded)?,
            };
            let reference = store_image(
                database,
                &settings,
                &group,
                &title,
                &bytes,
                provider.as_deref(),
            )?;
            Ok(MediaOutcome::Inserted {
                markdown: format!("![图片]({reference})"),
            })
        }
        MediaRequest::Resolve {
            control,
            content,
            note_path,
            settings,
            ..
        } => {
            control.remaining()?;
            let uses = image_uses(&content)?;
            let mut images = BTreeMap::new();
            let mut missing = Vec::new();
            let mut total = 0;
            for (index, dest) in uses
                .into_iter()
                .map(|u| u.destination)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .enumerate()
            {
                control.remaining()?;
                if index >= MAX_IMAGE_OPERATIONS {
                    missing.push(format!("{dest}：本次最多预览 200 张不同图片，引用已保留"));
                    continue;
                }
                let result = (|| -> Result<String> {
                    let load_dest = bindings.get(&dest).unwrap_or(&dest);
                    let cache_key =
                        preview_cache_key(database, &settings, load_dest, note_path.as_deref());
                    let url = cached_data_url(cache_key.as_deref(), Some(&control), || {
                        let bytes = load_image(
                            database,
                            &settings,
                            load_dest,
                            note_path.as_deref(),
                            provider.as_deref(),
                            Some(&control),
                        )?;
                        let (_, mime) = validated_image(&bytes)?;
                        control.remaining()?;
                        Ok(format!(
                            "data:{mime};base64,{}",
                            base64::engine::general_purpose::STANDARD.encode(bytes)
                        ))
                    })?;
                    control.remaining()?;
                    total += url.len();
                    ensure!(total <= MAX_BATCH_BYTES, "图片预览总量超过 64 MiB");
                    Ok(url)
                })();
                match result {
                    Ok(url) => {
                        images.insert(dest, url);
                    }
                    Err(error) => missing.push(format!("{dest}：{error}")),
                }
            }
            control.remaining()?;
            Ok(MediaOutcome::Resolved { images, missing })
        }
        MediaRequest::Adopt {
            policy,
            group,
            title,
            content,
            source_path,
            settings,
            ..
        } => {
            let mut replacements = BTreeMap::new();
            let mut missing = Vec::new();
            let mut total = 0;
            let mut attempted = 0;
            for dest in image_uses(&content)?
                .into_iter()
                .map(|u| u.destination)
                .collect::<BTreeSet<_>>()
            {
                if managed_reference_is_preserved(&dest, &group, policy) {
                    // Immutable managed references remain valid across historic groups.
                    // Only an explicit copy adopts their bytes into another group.
                    continue;
                }
                if attempted >= MAX_IMAGE_OPERATIONS {
                    missing.push(format!("{dest}：本次最多导入 200 张不同图片，原引用已保留"));
                    continue;
                }
                attempted += 1;
                let bytes = match load_image(
                    database,
                    &settings,
                    &dest,
                    source_path.as_deref(),
                    provider.as_deref(),
                    None,
                ) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        missing.push(format!("{dest}：{error}"));
                        continue;
                    }
                };
                total += bytes.len();
                ensure!(
                    total <= MAX_BATCH_BYTES,
                    "图片总量超过 64 MiB；原笔记已保留"
                );
                replacements.insert(
                    dest,
                    store_image(
                        database,
                        &settings,
                        &group,
                        &title,
                        &bytes,
                        provider.as_deref(),
                    )?,
                );
            }
            if retain_only {
                return Ok(MediaOutcome::Retained {
                    bindings: replacements,
                    missing,
                });
            }
            Ok(MediaOutcome::Adopted {
                content: rewrite_images(&content, &replacements)?,
                missing,
            })
        }
        MediaRequest::Retain { .. } | MediaRequest::ResolveRetained { .. } => unreachable!(),
    }
}

fn managed_reference_is_preserved(dest: &str, group: &str, policy: MediaAdoptionPolicy) -> bool {
    asset_key(dest).is_ok_and(|key| {
        policy == MediaAdoptionPolicy::PreserveManaged || key.split('/').nth(1) == Some(group)
    })
}

pub fn execute_with_provider_factory(
    database: &Path,
    request: MediaRequest,
    factory: impl FnOnce() -> Result<Option<Box<dyn AssetProvider>>>,
) -> Result<MediaOutcome> {
    let adoption = match &request {
        MediaRequest::Adopt {
            group,
            content,
            policy,
            ..
        } => Some((group, content, *policy)),
        MediaRequest::Retain { group, content, .. } => {
            Some((group, content, MediaAdoptionPolicy::PreserveManaged))
        }
        _ => None,
    };
    if let Some((group, content, policy)) = adoption
        && image_uses(content)?
            .iter()
            .all(|image| managed_reference_is_preserved(&image.destination, group, policy))
    {
        return execute(database, request, None);
    }
    execute_with_provider(database, request, factory())
}

pub fn execute_with_provider(
    database: &Path,
    request: MediaRequest,
    provider: Result<Option<Box<dyn AssetProvider>>>,
) -> Result<MediaOutcome> {
    match provider {
        Ok(provider) => execute(database, request, provider),
        Err(error) => {
            // Preserve clipboard/file bytes before reporting a credential error.
            let outcome = execute(database, request, None);
            match outcome {
                Ok(MediaOutcome::Resolved {
                    images,
                    mut missing,
                }) => {
                    if !missing.is_empty() {
                        missing.push(format!("图片云端不可用：{error:#}"));
                    }
                    Ok(MediaOutcome::Resolved { images, missing })
                }
                Ok(other) => Ok(other),
                Err(local) => Err(local).with_context(|| format!("图片云端不可用：{error:#}")),
            }
        }
    }
}

fn upload_marker(
    database: &Path,
    provider: &dyn AssetProvider,
    key: &str,
    bytes: &[u8],
) -> Option<PathBuf> {
    let identity = provider.identity()?;
    let digest = studyapp_sync::content_hash_bytes;
    Some(
        database
            .parent()?
            .join("image-cloud-state")
            .join(digest(identity.as_bytes()))
            .join(format!("{}-{}.done", digest(key.as_bytes()), digest(bytes))),
    )
}
fn already_uploaded(
    database: &Path,
    provider: &dyn AssetProvider,
    key: &str,
    bytes: &[u8],
) -> Result<bool> {
    let Some(marker) = upload_marker(database, provider, key, bytes) else {
        return Ok(false);
    };
    if !marker.try_exists()? {
        return Ok(false);
    }
    let _guards = crate::storage_backend::guard_directories(
        marker.parent().context("图片上传状态目录不存在")?,
        false,
    )?;
    let mut file = crate::storage_backend::plain_handle(&marker, false, true)?;
    let mut marker_bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(64)
        .read_to_end(&mut marker_bytes)?;
    Ok(marker_bytes == b"uploaded")
}
fn record_uploaded(
    database: &Path,
    provider: &dyn AssetProvider,
    key: &str,
    bytes: &[u8],
) -> Result<()> {
    let Some(marker) = upload_marker(database, provider, key, bytes) else {
        return Ok(());
    };
    if already_uploaded(database, provider, key, bytes)? {
        return Ok(());
    }
    match atomic_publish(&marker, b"uploaded") {
        Ok(()) => Ok(()),
        // An import and an explicit retry may confirm the same immutable object.
        // Losing create-only publication is successful when the winner wrote it.
        Err(_) if already_uploaded(database, provider, key, bytes)? => Ok(()),
        Err(error) => Err(error).context("云端已上传，但本地上传状态无法保存；可安全重试"),
    }
}

fn retry_cursor_path(database: &Path, provider: &dyn AssetProvider) -> Result<PathBuf> {
    let identity = provider.identity().unwrap_or_else(|| "unidentified".into());
    Ok(database
        .parent()
        .context("资料库目录不存在")?
        .join("image-cloud-state")
        .join(studyapp_sync::content_hash_bytes(identity.as_bytes()))
        .join("retry-cursor.json"))
}

fn read_retry_cursor(path: &Path) -> Result<Option<String>> {
    if !path.try_exists()? {
        return Ok(None);
    }
    let _guards = crate::storage_backend::guard_directories(path.parent().unwrap(), false)?;
    let file = crate::storage_backend::plain_handle(path, false, true)?;
    ensure!(file.metadata()?.len() <= 8192, "图片重试进度超过限制");
    let cursor: Option<String> = serde_json::from_reader(file.take(8193))?;
    if let Some(key) = &cursor {
        studyapp_sync::validate_asset_key(key)?;
    }
    Ok(cursor)
}

fn write_retry_cursor(path: &Path, cursor: Option<&str>) -> Result<()> {
    let parent = path.parent().context("图片重试进度目录不存在")?;
    let _guards = crate::storage_backend::guard_directories(parent, true)?;
    if path.try_exists()? {
        let _ = crate::storage_backend::plain_handle(path, false, false)?;
    }
    let temporary = parent.join(format!(".retry-{}.pending", new_group()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(&serde_json::to_vec(&cursor)?)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, path)?;
    #[cfg(unix)]
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

pub fn retry_existing_assets(
    database: &Path,
    settings: &ImageSettings,
    provider: &dyn AssetProvider,
) -> Result<String> {
    retry_existing_assets_bounded(
        database,
        settings,
        provider,
        MAX_IMAGE_OPERATIONS,
        MAX_BATCH_BYTES,
        Duration::from_secs(60),
    )
}

fn retry_existing_assets_bounded(
    database: &Path,
    settings: &ImageSettings,
    provider: &dyn AssetProvider,
    max_checks: usize,
    max_bytes: usize,
    max_time: Duration,
) -> Result<String> {
    ensure!(
        settings.storage != ImageStorage::Local,
        "当前使用本地图片模式，请先选择云端方式并保存设置"
    );
    let index = database
        .parent()
        .context("资料库目录不存在")?
        .join("image-index");
    if !index.try_exists()? {
        return Ok("没有待上传的本地图片".into());
    }
    let _guards = crate::storage_backend::guard_directories(&index, false)?;
    let started = Instant::now();
    let cursor_path = retry_cursor_path(database, provider)?;
    let cursor = read_retry_cursor(&cursor_path)?;
    let mut keys = Vec::new();
    let mut index_entries = 0;
    for group in std::fs::read_dir(&index)? {
        let group = group?;
        index_entries += 1;
        ensure!(index_entries <= 100_000, "图片索引数量超过本次检查限制");
        if !group.file_type()?.is_dir() {
            continue;
        }
        let name = group.file_name().to_string_lossy().into_owned();
        if uuid::Uuid::parse_str(&name).is_err() {
            continue;
        }
        let _group_guards = crate::storage_backend::guard_directories(&group.path(), false)?;
        for entry in std::fs::read_dir(group.path())? {
            let entry = entry?;
            index_entries += 1;
            ensure!(index_entries <= 100_000, "图片索引数量超过本次检查限制");
            let filename = entry.file_name().to_string_lossy().into_owned();
            let Some(filename) = filename.strip_suffix(".json") else {
                continue;
            };
            let key = format!("assets/{name}/{filename}");
            if studyapp_sync::validate_asset_key(&key).is_err() {
                continue;
            }
            keys.push(key);
        }
    }
    keys.sort();
    let mut next = cursor
        .as_ref()
        .map_or(0, |cursor| keys.partition_point(|key| key <= cursor));
    if next == keys.len() {
        next = 0;
    }
    let start = next;
    let mut uploaded = 0;
    let mut pending = 0;
    let mut checked = 0;
    let mut remaining_bytes = max_bytes;
    let mut errors = Vec::new();
    while next < keys.len() {
        // Reserve a whole bounded read before opening a file. Failed/invalid reads
        // consume that reservation too; successful reads refund unused capacity.
        // Confirmed assets count as checks, so a large cache cannot bypass limits.
        if checked >= max_checks
            || remaining_bytes < MAX_IMAGE_BYTES
            || started.elapsed() >= max_time
        {
            break;
        }
        checked += 1;
        remaining_bytes -= MAX_IMAGE_BYTES;
        let key = &keys[next];
        let result = (|| -> Result<Option<bool>> {
            let bytes = read_file(&local_asset(database, settings, key)?)?;
            remaining_bytes += MAX_IMAGE_BYTES - bytes.len();
            if started.elapsed() >= max_time {
                return Ok(None);
            }
            if already_uploaded(database, provider, key, &bytes)? {
                return Ok(Some(false));
            }
            if started.elapsed() >= max_time {
                return Ok(None);
            }
            provider.put_asset(key, &bytes)?;
            record_uploaded(database, provider, key, &bytes)?;
            Ok(Some(true))
        })();
        match result {
            Ok(None) => break,
            Ok(Some(true)) => uploaded += 1,
            Ok(Some(false)) => {}
            Err(error) => {
                pending += 1;
                if errors.len() < 3 {
                    errors.push(error.to_string());
                }
            }
        }
        next += 1;
    }
    if next > start {
        let cursor = (next < keys.len()).then(|| keys[next - 1].as_str());
        write_retry_cursor(&cursor_path, cursor).context("本地图片已保留，但重试进度无法保存")?;
    }
    let remaining = keys.len() - next;
    if pending == 0 && remaining == 0 {
        Ok(format!(
            "本批图片检查完成：本次上传 {uploaded} 张；已确认的图片无需重复上传，本地缓存保留"
        ))
    } else {
        let mut message = format!("本地图片已保留；本次上传 {uploaded} 张");
        if pending > 0 {
            message.push_str(&format!("，{pending} 张云端待重试：{}", errors.join("；")));
        }
        if remaining > 0 {
            message.push_str(&format!(
                "；另有 {remaining} 张尚未检查，请再次重试继续后续图片"
            ));
        }
        Ok(message)
    }
}

#[cfg(test)]
#[path = "media_stability_tests.rs"]
mod stability_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use studyapp_sync::Provider;

    struct RetryImages {
        uploads: Mutex<Vec<String>>,
    }
    impl AssetProvider for RetryImages {
        fn get_asset(&self, _: &str) -> studyapp_sync::Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn put_asset(&self, key: &str, _: &[u8]) -> studyapp_sync::Result<()> {
            self.uploads.lock().unwrap().push(key.to_owned());
            Ok(())
        }
        fn check_connection(&self) -> studyapp_sync::Result<()> {
            Ok(())
        }
        fn identity(&self) -> Option<String> {
            Some("bounded-retry-fixture".into())
        }
    }
    fn retry_fixture(count: usize) -> (PathBuf, ImageSettings, RetryImages, Vec<String>) {
        let database = database();
        let settings = ImageSettings {
            storage: ImageStorage::Both,
            ..Default::default()
        };
        let group = new_group();
        let keys: Vec<_> = (0..count)
            .map(|n| format!("assets/{group}/{n:04}.gif"))
            .collect();
        for key in &keys {
            publish_local(&database, &settings, key, GIF).unwrap();
        }
        (
            database,
            settings,
            RetryImages {
                uploads: Mutex::new(Vec::new()),
            },
            keys,
        )
    }

    #[test]
    fn retry_budget_stops_before_reading_the_next_image() {
        let (database, settings, provider, keys) = retry_fixture(2);
        let second = local_asset(&database, &settings, &keys[1]).unwrap();
        std::fs::write(&second, b"invalid image must not be opened in this batch").unwrap();
        let result = retry_existing_assets_bounded(
            &database,
            &settings,
            &provider,
            1,
            MAX_BATCH_BYTES,
            Duration::from_secs(60),
        )
        .unwrap();
        assert_eq!(*provider.uploads.lock().unwrap(), vec![keys[0].clone()]);
        assert!(result.contains("尚未检查"));
        assert!(!result.contains("格式无法识别"), "{result}");
        assert!(!result.contains("待重试"), "{result}");
    }

    #[test]
    fn retry_cursor_reaches_later_images_after_acknowledged_batches() {
        let (database, settings, provider, keys) = retry_fixture(3);
        record_uploaded(&database, &provider, &keys[0], GIF).unwrap();
        for expected in [0, 1, 2] {
            retry_existing_assets_bounded(
                &database,
                &settings,
                &provider,
                1,
                MAX_BATCH_BYTES,
                Duration::from_secs(60),
            )
            .unwrap();
            assert_eq!(provider.uploads.lock().unwrap().len(), expected);
        }
        assert_eq!(*provider.uploads.lock().unwrap(), keys[1..]);
    }

    #[test]
    fn retry_zero_time_or_insufficient_byte_budget_does_not_read_images() {
        let (database, settings, provider, keys) = retry_fixture(1);
        std::fs::write(
            local_asset(&database, &settings, &keys[0]).unwrap(),
            b"invalid",
        )
        .unwrap();
        for (bytes, time) in [
            (MAX_BATCH_BYTES, Duration::ZERO),
            (0, Duration::from_secs(60)),
        ] {
            let result =
                retry_existing_assets_bounded(&database, &settings, &provider, 200, bytes, time)
                    .unwrap();
            assert!(result.contains("尚未检查"));
            assert!(!result.contains("格式无法识别"), "{result}");
        }
        assert!(provider.uploads.lock().unwrap().is_empty());
    }

    #[test]
    fn concurrent_upload_acknowledgements_are_idempotent() {
        let (database, _, provider, keys) = retry_fixture(1);
        let barrier = std::sync::Barrier::new(8);
        std::thread::scope(|scope| {
            let jobs: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        record_uploaded(&database, &provider, &keys[0], GIF)
                    })
                })
                .collect();
            for job in jobs {
                job.join().unwrap().unwrap();
            }
        });
        assert!(already_uploaded(&database, &provider, &keys[0], GIF).unwrap());
    }

    #[test]
    fn explicit_retry_uploads_existing_local_assets_after_cloud_mode_change() {
        struct Images {
            uploads: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        }
        impl studyapp_sync::AssetProvider for Images {
            fn get_asset(&self, _: &str) -> studyapp_sync::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn put_asset(&self, _: &str, bytes: &[u8]) -> studyapp_sync::Result<()> {
                assert_eq!(bytes, GIF);
                self.uploads.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            fn check_connection(&self) -> studyapp_sync::Result<()> {
                Ok(())
            }
            fn identity(&self) -> Option<String> {
                Some("fixture-target".into())
            }
        }
        let database = database();
        let request = MediaRequest::Import {
            generation: 1,
            request: 1,
            group: new_group(),
            title: "local".into(),
            input: MediaInput::Bytes(GIF.to_vec()),
            settings: Default::default(),
        };
        let MediaOutcome::Inserted { markdown } = execute(&database, request, None).unwrap() else {
            panic!()
        };
        let uploads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider = Images {
            uploads: uploads.clone(),
        };
        let settings = ImageSettings {
            storage: ImageStorage::Both,
            ..Default::default()
        };
        let result = retry_existing_assets(&database, &settings, &provider).unwrap();
        assert!(result.contains("1"));
        assert_eq!(uploads.load(Ordering::SeqCst), 1);
        retry_existing_assets(&database, &settings, &provider).unwrap();
        assert_eq!(
            uploads.load(Ordering::SeqCst),
            1,
            "acknowledged stable assets need no duplicate upload"
        );
        let key = asset_key(&image_uses(&markdown).unwrap()[0].destination).unwrap();
        assert_eq!(
            std::fs::read(local_asset(&database, &settings, &key).unwrap()).unwrap(),
            GIF
        );
    }

    #[test]
    fn provider_factory_error_is_preserved_after_import_bytes_are_staged_locally() {
        let database = database();
        let group = new_group();
        let result = execute_with_provider(
            &database,
            MediaRequest::Import {
                generation: 1,
                request: 1,
                group: group.clone(),
                title: "pending".into(),
                input: MediaInput::Bytes(GIF.to_vec()),
                settings: ImageSettings {
                    storage: ImageStorage::Both,
                    ..Default::default()
                },
            },
            Err(anyhow::anyhow!("fixture credential cannot decrypt")),
        );
        let error = format!("{:#}", result.unwrap_err());
        assert!(error.contains("fixture credential cannot decrypt"));
        assert!(error.contains("本地"));
        let files: Vec<_> =
            std::fs::read_dir(database.parent().unwrap().join("images").join(group))
                .unwrap()
                .collect();
        assert!(
            files
                .iter()
                .any(|file| std::fs::read(file.as_ref().unwrap().path()).unwrap() == GIF)
        );
    }

    #[test]
    fn retained_draft_images_resolve_original_markdown_after_source_is_moved() {
        let database = database();
        let source = database.parent().unwrap().join("source.md");
        let image = database.parent().unwrap().join("external.gif");
        std::fs::write(&source, "![old](external.gif)").unwrap();
        std::fs::write(&image, GIF).unwrap();
        let content = std::fs::read_to_string(&source).unwrap();
        let MediaOutcome::Retained { bindings, missing } = execute(
            &database,
            MediaRequest::Retain {
                generation: 1,
                request: 1,
                group: new_group(),
                title: "draft".into(),
                content: content.clone(),
                source_path: Some(source),
                settings: Default::default(),
            },
            None,
        )
        .unwrap() else {
            panic!()
        };
        assert!(missing.is_empty());
        assert!(bindings["external.gif"].starts_with(PREFIX));
        std::fs::rename(
            image,
            database.parent().unwrap().join("retained-external.gif"),
        )
        .unwrap();
        let MediaOutcome::Resolved { images, missing } = execute(
            &database,
            MediaRequest::ResolveRetained {
                control: Default::default(),
                generation: 1,
                request: 2,
                content: content.clone(),
                note_path: None,
                bindings,
                settings: Default::default(),
            },
            None,
        )
        .unwrap() else {
            panic!()
        };
        assert!(missing.is_empty());
        assert!(images.contains_key("external.gif"));
        assert_eq!(content, "![old](external.gif)");
    }
    const GIF:&[u8]=b"GIF89a\x01\0\x01\0\x80\0\0\0\0\0\xff\xff\xff!\xf9\x04\x01\0\0\0\0,\0\0\0\0\x01\0\x01\0\0\x02\x02D\x01\0;";
    fn database() -> PathBuf {
        tempfile::tempdir().unwrap().keep().join("library.sqlite3")
    }
    fn imported(database: &Path, settings: ImageSettings) -> String {
        let request = MediaRequest::Import {
            generation: 7,
            request: 8,
            group: new_group(),
            title: "我的笔记".into(),
            input: MediaInput::Bytes(GIF.to_vec()),
            settings,
        };
        let MediaOutcome::Inserted { markdown } = execute(database, request, None).unwrap() else {
            panic!()
        };
        image_uses(&markdown).unwrap()[0].destination.clone()
    }
    #[test]
    fn imported_assets_have_separate_group_time_title_and_survive_root_changes() {
        let database = database();
        let dest = imported(&database, Default::default());
        assert!(dest.contains("我的笔记"));
        let key = asset_key(&dest).unwrap();
        let path = local_asset(&database, &Default::default(), &key).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), GIF);
        assert!(
            path.parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .len()
                == 36
        );
        let settings = ImageSettings {
            root: Some(
                database
                    .parent()
                    .unwrap()
                    .join("changed-root")
                    .to_string_lossy()
                    .into_owned(),
            ),
            storage: ImageStorage::Local,
            ..Default::default()
        };
        let MediaOutcome::Resolved { images, missing } = execute(
            &database,
            MediaRequest::Resolve {
                control: Default::default(),
                generation: 1,
                request: 1,
                content: format!("![a]({dest})"),
                note_path: None,
                settings,
            },
            None,
        )
        .unwrap() else {
            panic!()
        };
        assert!(missing.is_empty());
        assert!(images[&dest].starts_with("data:image/gif;base64,"));
    }
    #[test]
    fn adoption_copies_percent_encoded_external_image_and_rewrites_only_new_content() {
        let database = database();
        let source = database.parent().unwrap().join("source.md");
        let image = database.parent().unwrap().join("old image.gif");
        std::fs::write(&image, GIF).unwrap();
        let content =
            "![old](old%20image.gif \"title\")\r\n```\r\n![code](old%20image.gif)\r\n```\r\n";
        std::fs::write(&source, content).unwrap();
        let MediaOutcome::Adopted {
            content: copy,
            missing,
        } = execute(
            &database,
            MediaRequest::Adopt {
                policy: MediaAdoptionPolicy::CopyToGroup,
                generation: 1,
                request: 2,
                group: new_group(),
                title: "new".into(),
                content: content.into(),
                source_path: Some(source.clone()),
                settings: Default::default(),
            },
            None,
        )
        .unwrap()
        else {
            panic!()
        };
        assert!(missing.is_empty());
        assert!(copy.contains(PREFIX));
        assert!(copy.contains(" \"title\")\r\n"));
        assert!(copy.contains("![code](old%20image.gif)"));
        assert_eq!(std::fs::read_to_string(source).unwrap(), content);
        assert_eq!(std::fs::read(image).unwrap(), GIF);
    }
    #[test]
    fn cloud_assets_are_uploaded_before_success_and_can_be_downloaded_without_note_listing() {
        let database = database();
        let remote = tempfile::tempdir().unwrap().keep();
        let provider = studyapp_sync::LocalFolderProvider::new(&remote).unwrap();
        let request = MediaRequest::Import {
            generation: 1,
            request: 2,
            group: new_group(),
            title: "云图片".into(),
            input: MediaInput::Bytes(GIF.to_vec()),
            settings: ImageSettings {
                root: None,
                storage: ImageStorage::Cloud,
                ..Default::default()
            },
        };
        let MediaOutcome::Inserted { markdown } =
            execute(&database, request, Some(Box::new(provider))).unwrap()
        else {
            panic!()
        };
        let dest = image_uses(&markdown).unwrap()[0].destination.clone();
        let key = asset_key(&dest).unwrap();
        let provider = studyapp_sync::LocalFolderProvider::new(&remote).unwrap();
        assert_eq!(provider.read_asset(&key).unwrap().unwrap(), GIF);
        assert!(provider.list().unwrap().is_empty());
        // A second library has no cache or index, and recovers the immutable asset.
        let second = super::tests::database();
        let MediaOutcome::Resolved { images, missing } = execute(
            &second,
            MediaRequest::Resolve {
                control: Default::default(),
                generation: 1,
                request: 1,
                content: markdown,
                note_path: None,
                settings: Default::default(),
            },
            Some(Box::new(provider)),
        )
        .unwrap() else {
            panic!()
        };
        assert!(images.contains_key(&dest));
        assert!(missing.is_empty());
    }
    #[test]
    fn failed_cloud_upload_keeps_staged_files_and_never_claims_success() {
        let database = database();
        let group = new_group();
        let result = execute(
            &database,
            MediaRequest::Import {
                generation: 1,
                request: 2,
                group: group.clone(),
                title: "test".into(),
                input: MediaInput::Bytes(GIF.to_vec()),
                settings: ImageSettings {
                    root: None,
                    storage: ImageStorage::Cloud,
                    ..Default::default()
                },
            },
            None,
        );
        assert!(result.is_err());
        let paths: Vec<_> =
            std::fs::read_dir(database.parent().unwrap().join("images").join(group))
                .unwrap()
                .collect();
        assert_eq!(paths.len(), 2);
        assert_eq!(
            std::fs::read(paths[0].as_ref().unwrap().path()).unwrap(),
            GIF
        );
    }
    #[test]
    fn invalid_import_never_creates_owned_files_and_markdown_size_is_bounded() {
        let database = database();
        assert!(
            execute(
                &database,
                MediaRequest::Import {
                    generation: 1,
                    request: 2,
                    group: new_group(),
                    title: "bad".into(),
                    input: MediaInput::Bytes(b"<svg/>".to_vec()),
                    settings: Default::default()
                },
                None
            )
            .is_err()
        );
        assert!(!database.parent().unwrap().join("images").exists());
        assert_eq!(
            image_uses(&"![x](missing.png)\n".repeat(201))
                .unwrap()
                .len(),
            201
        );
        assert!(image_uses(&"x".repeat(10 * 1024 * 1024 + 1)).is_err());
        assert!(asset_key("studyapp-media:../escape.png").is_err());
    }
    #[test]
    fn reference_rewrite_preserves_eol_code_and_regular_links() {
        let text = "![a][pic]\r\n[regular](old.png)\r\n```\r\n![code](old.png)\r\n```\r\n[pic]: old.png \"caption\"\r\n";
        let replacements =
            BTreeMap::from([("old.png".into(), "studyapp-media:new/new.png".into())]);
        let changed = rewrite_images(text, &replacements).unwrap();
        assert!(changed.contains("[pic]: studyapp-media:new/new.png \"caption\"\r\n"));
        assert!(changed.contains("[regular](old.png)"));
        assert!(changed.contains("![code](old.png)"));
        assert_eq!(changed.matches('\n').count(), text.matches('\n').count());
    }
    #[test]
    fn shared_reference_rewrite_keeps_non_image_link_destination() {
        let text = "![image][same]\r\n[regular][same]\r\n\r\n[same]: old.png \"caption\"\r\n";
        let changed = rewrite_images(
            text,
            &BTreeMap::from([("old.png".into(), "studyapp-media:owned/image.png".into())]),
        )
        .unwrap();
        assert!(changed.contains("[same]: old.png \"caption\"\r\n"));
        assert!(changed.contains("[regular][same]"));
        assert!(changed.contains("![image](studyapp-media:owned/image.png \"caption\")"));
    }
    #[test]
    fn network_images_allow_only_https_without_embedded_credentials() {
        assert!(remote_image_url("https://example.com/picture.png?size=2").is_ok());
        for url in [
            "http://example.com/a.png",
            "https://alice:secret@example.com/a.png",
            "https://alice@example.com/a.png",
            "file:///C:/private.png",
            "data:image/png;base64,AAAA",
            "javascript:alert(1)",
        ] {
            assert!(remote_image_url(url).is_err(), "{url}");
        }
        assert!(bounded_image_bytes(Cursor::new(vec![0; MAX_IMAGE_BYTES + 1])).is_err());
        assert_eq!(bounded_image_bytes(Cursor::new(GIF)).unwrap(), GIF);
    }
    #[test]
    fn encoded_clipboard_import_is_validated_and_decoded_in_backend() {
        let database = database();
        let encoded = base64::engine::general_purpose::STANDARD.encode(GIF);
        let request = MediaRequest::Import {
            generation: 1,
            request: 2,
            group: new_group(),
            title: "paste".into(),
            input: MediaInput::Encoded(encoded),
            settings: Default::default(),
        };
        assert!(matches!(
            execute(&database, request, None).unwrap(),
            MediaOutcome::Inserted { .. }
        ));
        assert!(decode_image_input("not base64").is_err());
        assert!(decode_image_input(&"A".repeat(MAX_IMAGE_BYTES.div_ceil(3) * 4 + 1)).is_err());
    }
    #[test]
    fn preview_cache_is_bounded_expires_and_reuses_without_calling_loader() {
        let now = std::time::Instant::now();
        let mut cache = PreviewCache::default();
        cache.insert("one".into(), "url-one".into(), now, 512);
        assert_eq!(cache.get("one", now).unwrap(), "url-one");
        cache.insert("two".into(), "url-two".into(), now, 300);
        assert!(cache.bytes <= 300);
        assert!(cache.get("one", now).is_none());
        assert!(
            cache
                .get("two", now + std::time::Duration::from_secs(301))
                .is_none()
        );
        let key = format!("test:{}", new_group());
        assert_eq!(
            cached_data_url(Some(&key), None, || Ok("cached-url".into())).unwrap(),
            "cached-url"
        );
        assert_eq!(
            cached_data_url(Some(&key), None, || panic!(
                "cache hit must skip image decode/network"
            ))
            .unwrap(),
            "cached-url"
        );
    }
    #[test]
    fn atomic_publication_retains_staging_and_never_replaces_existing_bytes() {
        let database = database();
        let path = database.parent().unwrap().join("asset.gif");
        atomic_publish(&path, GIF).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), GIF);
        assert!(atomic_publish(&path, b"replacement").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), GIF);
        let stages: Vec<_> = std::fs::read_dir(database.parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".studyapp-image-")
            })
            .collect();
        assert_eq!(stages.len(), 2);
    }
    #[test]
    fn corrupted_managed_cache_is_recovered_from_provider_without_losing_bytes() {
        let database = database();
        let remote = tempfile::tempdir().unwrap().keep();
        let provider = studyapp_sync::LocalFolderProvider::new(remote).unwrap();
        let key = format!("assets/{}/image.gif", new_group());
        provider.write_asset(&key, GIF).unwrap();
        let path =
            publish_local(&database, &Default::default(), &key, b"partial old cache").unwrap();
        let reference = format!("{PREFIX}{}", key.strip_prefix("assets/").unwrap());
        assert_eq!(
            load_image(
                &database,
                &Default::default(),
                &reference,
                None,
                Some(&provider),
                None,
            )
            .unwrap(),
            GIF
        );
        assert_eq!(read_file(&path).unwrap(), GIF);
        assert!(
            std::fs::read_dir(path.parent().unwrap())
                .unwrap()
                .flatten()
                .any(|entry| std::fs::read(entry.path())
                    .is_ok_and(|bytes| bytes == b"partial old cache"))
        );
    }
    #[test]
    fn cancellation_during_image_loading_prevents_preview_cache_publication() {
        let key = format!("cancelled:{}", new_group());
        let control = ResolveControl::default();
        assert!(
            cached_data_url(Some(&key), Some(&control), || {
                control.cancel();
                Ok("obsolete preview".into())
            })
            .is_err()
        );
        assert_eq!(
            cached_data_url(Some(&key), None, || Ok("fresh preview".into())).unwrap(),
            "fresh preview"
        );
    }
    #[test]
    fn cancelled_or_expired_preview_does_not_resolve_even_an_empty_document() {
        for expired in [false, true] {
            let control = ResolveControl::default();
            let control = if expired {
                ResolveControl {
                    deadline: Instant::now(),
                    ..control
                }
            } else {
                control.cancel();
                control
            };
            let result = execute(
                &database(),
                MediaRequest::Resolve {
                    control,
                    generation: 1,
                    request: 2,
                    content: String::new(),
                    note_path: None,
                    settings: Default::default(),
                },
                None,
            );
            assert!(
                result.is_err(),
                "obsolete preview must stop before resolving"
            );
        }
    }
    #[test]
    fn saving_201_repeated_owned_references_preserves_every_occurrence() {
        let database = database();
        let dest = imported(&database, Default::default());
        let key = asset_key(&dest).unwrap();
        let group = key.split('/').nth(1).unwrap().to_owned();
        let content = format!("![owned]({dest})\r\n").repeat(201);
        let MediaOutcome::Adopted {
            content: saved,
            missing,
        } = execute(
            &database,
            MediaRequest::Adopt {
                policy: MediaAdoptionPolicy::CopyToGroup,
                generation: 1,
                request: 1,
                group,
                title: "repeated".into(),
                content: content.clone(),
                source_path: None,
                settings: Default::default(),
            },
            None,
        )
        .unwrap()
        else {
            panic!()
        };
        assert_eq!(saved, content);
        assert_eq!(saved.matches(&dest).count(), 201);
        assert!(missing.is_empty());
    }
    #[test]
    fn preview_and_adoption_bound_distinct_work_without_losing_excess_references() {
        let database = database();
        let root = database.parent().unwrap();
        let source = root.join("source.md");
        let mut content = String::new();
        for index in 0..201 {
            let name = format!("image-{index:03}.gif");
            std::fs::write(root.join(&name), GIF).unwrap();
            content.push_str(&format!("![image]({name})\n"));
        }
        // Repeated uses do not consume an additional slot.
        content.push_str("![repeat](image-000.gif)\n");
        let MediaOutcome::Resolved { images, missing } = execute(
            &database,
            MediaRequest::Resolve {
                control: Default::default(),
                generation: 1,
                request: 1,
                content: content.clone(),
                note_path: Some(source.clone()),
                settings: Default::default(),
            },
            None,
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(images.len(), 200);
        assert_eq!(missing.len(), 1);
        assert!(missing[0].contains("image-200.gif"));
        let MediaOutcome::Adopted {
            content: saved,
            missing,
        } = execute(
            &database,
            MediaRequest::Adopt {
                policy: MediaAdoptionPolicy::CopyToGroup,
                generation: 1,
                request: 2,
                group: new_group(),
                title: "many images".into(),
                content: content.clone(),
                source_path: Some(source),
                settings: Default::default(),
            },
            None,
        )
        .unwrap()
        else {
            panic!()
        };
        assert_eq!(missing.len(), 1);
        assert!(missing[0].contains("image-200.gif"));
        assert!(saved.contains("![image](image-200.gif)"));
        assert_eq!(saved.matches("![").count(), 202);
        assert_eq!(saved.matches(PREFIX).count(), 201);
        assert_eq!(std::fs::read(root.join("image-200.gif")).unwrap(), GIF);
    }
    #[test]
    fn saving_owned_images_does_not_apply_the_copy_batch_budget() {
        let database = database();
        let settings = ImageSettings::default();
        let group = new_group();
        let mut bytes = GIF.to_vec();
        bytes.resize(9 * 1024 * 1024, 0);
        let mut content = String::new();
        for index in 0..8 {
            let key = format!("assets/{group}/image-{index}.gif");
            publish_local(&database, &settings, &key, &bytes).unwrap();
            content.push_str(&format!("![image]({PREFIX}{group}/image-{index}.gif)\n"));
        }
        let result = execute(
            &database,
            MediaRequest::Adopt {
                policy: MediaAdoptionPolicy::CopyToGroup,
                generation: 1,
                request: 1,
                group,
                title: "large note".into(),
                content: content.clone(),
                source_path: None,
                settings,
            },
            None,
        );
        let MediaOutcome::Adopted {
            content: saved,
            missing,
        } = result.unwrap()
        else {
            panic!()
        };
        assert_eq!(saved, content);
        assert!(missing.is_empty());
    }
    #[test]
    fn normal_save_adoption_keeps_owned_same_group_stable_and_copy_uses_new_group() {
        let database = database();
        let dest = imported(&database, Default::default());
        let key = asset_key(&dest).unwrap();
        let group = key.split('/').nth(1).unwrap().to_owned();
        let content = format!("![image]({dest})\r\n");
        let request = MediaRequest::Adopt {
            policy: MediaAdoptionPolicy::CopyToGroup,
            generation: 1,
            request: 2,
            group: group.clone(),
            title: "title".into(),
            content: content.clone(),
            source_path: None,
            settings: Default::default(),
        };
        let MediaOutcome::Adopted {
            content: saved,
            missing,
        } = execute(&database, request, None).unwrap()
        else {
            panic!()
        };
        assert_eq!(saved, content);
        assert!(missing.is_empty());
        let request = MediaRequest::Adopt {
            policy: MediaAdoptionPolicy::CopyToGroup,
            generation: 1,
            request: 3,
            group: new_group(),
            title: "copy".into(),
            content: content.clone(),
            source_path: None,
            settings: Default::default(),
        };
        let MediaOutcome::Adopted {
            content: copy,
            missing,
        } = execute(&database, request, None).unwrap()
        else {
            panic!()
        };
        assert_ne!(copy, content);
        assert!(missing.is_empty());
    }
    #[test]
    fn image_magic_and_decoded_dimensions_are_bounded() {
        assert!(validated_image(b"<svg onload='x'/>").is_err());
        assert!(validated_image(&vec![0; MAX_IMAGE_BYTES + 1]).is_err());
        let gif = b"GIF89a\x01\0\x01\0\x80\0\0\0\0\0\xff\xff\xff!\xf9\x04\x01\0\0\0\0,\0\0\0\0\x01\0\x01\0\0\x02\x02D\x01\0;";
        assert_eq!(validated_image(gif).unwrap().0, "gif");
    }
    #[test]
    fn adoption_preserves_original_and_reports_missing_images() {
        let root = tempfile::tempdir().unwrap().keep();
        let source = root.join("external.md");
        let content = "before\r\n![missing](missing.png)\r\nafter\r\n";
        std::fs::write(&source, content).unwrap();
        let request = MediaRequest::Adopt {
            policy: MediaAdoptionPolicy::CopyToGroup,
            generation: 1,
            request: 2,
            group: new_group(),
            title: "笔记".into(),
            content: content.into(),
            source_path: Some(source.clone()),
            settings: Default::default(),
        };
        let result = execute(&root.join("library.sqlite3"), request, None).unwrap();
        let MediaOutcome::Adopted {
            content: copied,
            missing,
        } = result
        else {
            panic!()
        };
        assert_eq!(copied, content);
        assert_eq!(missing.len(), 1);
        assert_eq!(std::fs::read_to_string(source).unwrap(), content);
    }
}
