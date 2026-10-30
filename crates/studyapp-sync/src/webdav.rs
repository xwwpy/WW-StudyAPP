use crate::*;
use percent_encoding::percent_decode_str;
use quick_xml::{Reader, events::Event};
use reqwest::{
    Method, Url,
    blocking::{Client, RequestBuilder, Response},
    header::{ETAG, IF_MATCH, IF_NONE_MATCH},
};
use std::{collections::BTreeSet, net::IpAddr, sync::Mutex, time::Duration};
use zeroize::Zeroizing;

/// Credentials are never included in Debug, errors, persisted plans or URLs.
/// HTTPS certificate validation is enabled and every redirect is refused.
pub struct WebDavProvider {
    client: Client,
    base: Url,
    notes: Url,
    username: Zeroizing<String>,
    password: Zeroizing<String>,
    initialized: Mutex<bool>,
}

impl WebDavProvider {
    pub fn new(url: &str, username: &str, password: &str) -> Result<Self> {
        Self::build(url, username, password, false)
    }

    /// Explicit opt-in for loopback HTTP fixtures; production UI must use `new`.
    pub fn for_loopback_test(url: &str, username: &str, password: &str) -> Result<Self> {
        Self::build(url, username, password, true)
    }

    fn build(url: &str, username: &str, password: &str, allow_loopback: bool) -> Result<Self> {
        let mut base = Url::parse(url).map_err(|_| SyncError::InvalidUrl)?;
        let loopback = base
            .host_str()
            .and_then(|host| host.trim_matches(['[', ']']).parse::<IpAddr>().ok())
            .is_some_and(|ip| ip.is_loopback());
        if (base.scheme() != "https" && !(allow_loopback && base.scheme() == "http" && loopback))
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(SyncError::InvalidUrl);
        }
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        let notes = base
            .join("StudyApp/notes/")
            .map_err(|_| SyncError::InvalidUrl)?;
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .no_proxy()
            .build()
            .map_err(|_| SyncError::Transport)?;
        Ok(Self {
            client,
            base,
            notes,
            username: Zeroizing::new(username.to_owned()),
            password: Zeroizing::new(password.to_owned()),
            initialized: Mutex::new(false),
        })
    }

    fn request(&self, method: Method, url: Url) -> RequestBuilder {
        self.client
            .request(method, url)
            .basic_auth(self.username.as_str(), Some(self.password.as_str()))
    }

    fn send(request: RequestBuilder) -> Result<Response> {
        request.send().map_err(|_| SyncError::Transport)
    }

    fn ensure_dirs(&self) -> Result<()> {
        let mut initialized = self.initialized.lock().map_err(|_| SyncError::LockBusy)?;
        if *initialized {
            return Ok(());
        }
        for relative in ["StudyApp/", "StudyApp/notes/"] {
            let url = self
                .base
                .join(relative)
                .map_err(|_| SyncError::InvalidUrl)?;
            let method = Method::from_bytes(b"MKCOL").map_err(|_| SyncError::InvalidUrl)?;
            let response = Self::send(self.request(method, url))?;
            // WebDAV uses 405 for an already-existing collection. A subsequent
            // PROPFIND/GET/PUT still verifies that it actually is usable.
            if !matches!(response.status().as_u16(), 201 | 405) {
                return Err(SyncError::Http(response.status().as_u16()));
            }
        }
        *initialized = true;
        Ok(())
    }

    fn object_url(&self, id: &str) -> Result<Url> {
        validate_id(id)?;
        self.notes
            .join(&format!("{id}.md"))
            .map_err(|_| SyncError::InvalidUrl)
    }

    fn strong_etag(response: &Response) -> Option<String> {
        let etag = response.headers().get(ETAG)?.to_str().ok()?;
        if etag.len() >= 2
            && etag.starts_with('"')
            && etag.ends_with('"')
            && !etag[1..etag.len() - 1].contains('"')
        {
            Some(etag.to_owned())
        } else {
            None
        }
    }

    fn id_from_href(&self, href: &str) -> Option<String> {
        let decoded = percent_decode_str(href).decode_utf8().ok()?;
        if decoded.contains('\\')
            || decoded
                .split('/')
                .any(|segment| matches!(segment, "." | ".."))
        {
            return None;
        }
        let url = self.notes.join(href).ok()?;
        if url.origin() != self.notes.origin()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return None;
        }
        let tail = url.path().strip_prefix(self.notes.path())?;
        let name = percent_decode_str(tail).decode_utf8().ok()?;
        let id = name.strip_suffix(".md")?;
        validate_id(id).ok()?;
        Some(id.to_owned())
    }

    fn parse_listing(&self, xml: &str) -> Result<Vec<String>> {
        let mut reader = Reader::from_str(xml);
        reader.config_mut().check_end_names = true;
        let mut ids = BTreeSet::new();
        let mut depth = 0usize;
        let mut href_depth = None;
        let mut href = String::new();
        let mut responses = 0usize;
        let mut root_seen = false;
        loop {
            match reader.read_event().map_err(|_| SyncError::InvalidXml)? {
                Event::Start(event) => {
                    depth += 1;
                    if depth > 32 {
                        return Err(SyncError::LimitExceeded);
                    }
                    if depth == 1 {
                        if root_seen || event.local_name().as_ref() != b"multistatus" {
                            return Err(SyncError::InvalidXml);
                        }
                        root_seen = true;
                    }
                    if event.local_name().as_ref() == b"response" {
                        responses += 1;
                        if responses > MAX_ENTRIES {
                            return Err(SyncError::LimitExceeded);
                        }
                    }
                    if event.local_name().as_ref() == b"href" {
                        if href_depth.is_some() {
                            return Err(SyncError::InvalidXml);
                        }
                        href_depth = Some(depth);
                        href.clear();
                    }
                }
                Event::Empty(event) => {
                    if depth == 0 {
                        if root_seen || event.local_name().as_ref() != b"multistatus" {
                            return Err(SyncError::InvalidXml);
                        }
                        root_seen = true;
                    }
                    if event.local_name().as_ref() == b"response" {
                        responses += 1;
                        if responses > MAX_ENTRIES {
                            return Err(SyncError::LimitExceeded);
                        }
                    }
                }
                Event::End(event) => {
                    if depth == 0 {
                        return Err(SyncError::InvalidXml);
                    }
                    if event.local_name().as_ref() == b"href" && href_depth == Some(depth) {
                        let href = quick_xml::escape::unescape(&href)
                            .map_err(|_| SyncError::InvalidXml)?;
                        if let Some(id) = self.id_from_href(href.trim()) {
                            ids.insert(id);
                        }
                        if ids.len() > MAX_ENTRIES {
                            return Err(SyncError::LimitExceeded);
                        }
                        href_depth = None;
                    }
                    depth -= 1;
                }
                Event::Text(event) if href_depth.is_some() => {
                    href.push_str(&event.decode().map_err(|_| SyncError::InvalidXml)?)
                }
                Event::CData(event) if href_depth.is_some() => href.push_str(
                    &quick_xml::escape::escape(event.decode().map_err(|_| SyncError::InvalidXml)?),
                ),
                Event::GeneralRef(event) if href_depth.is_some() => {
                    href.push('&');
                    href.push_str(&event.decode().map_err(|_| SyncError::InvalidXml)?);
                    href.push(';');
                }
                Event::DocType(_) => return Err(SyncError::InvalidXml),
                Event::Eof => break,
                _ => {}
            }
        }
        if !root_seen || depth != 0 || href_depth.is_some() {
            return Err(SyncError::InvalidXml);
        }
        Ok(ids.into_iter().collect())
    }
}

impl Provider for WebDavProvider {
    fn read_asset(&self, key: &str) -> Result<Option<Vec<u8>>> {
        validate_asset_key(key)?;
        let url = self
            .base
            .join(&format!("StudyApp/{key}"))
            .map_err(|_| SyncError::InvalidUrl)?;
        let response = Self::send(self.request(Method::GET, url))?;
        match response.status().as_u16() {
            404 => return Ok(None),
            200 => {}
            status => return Err(SyncError::Http(status)),
        };
        if response
            .content_length()
            .is_some_and(|len| len > MAX_CONTENT_BYTES as u64)
        {
            return Err(SyncError::LimitExceeded);
        }
        bounded_bytes(response).map(Some)
    }
    fn write_asset(&self, key: &str, bytes: &[u8]) -> Result<()> {
        validate_asset_key(key)?;
        if bytes.len() > MAX_CONTENT_BYTES {
            return Err(SyncError::LimitExceeded);
        }
        self.ensure_dirs()?;
        let group = key.split('/').nth(1).ok_or(SyncError::InvalidPath)?;
        for relative in [
            "StudyApp/assets/".to_owned(),
            format!("StudyApp/assets/{group}/"),
        ] {
            let url = self
                .base
                .join(&relative)
                .map_err(|_| SyncError::InvalidUrl)?;
            let response = Self::send(self.request(
                Method::from_bytes(b"MKCOL").map_err(|_| SyncError::InvalidUrl)?,
                url,
            ))?;
            if !matches!(response.status().as_u16(), 201 | 405) {
                return Err(SyncError::Http(response.status().as_u16()));
            }
        }
        let url = self
            .base
            .join(&format!("StudyApp/{key}"))
            .map_err(|_| SyncError::InvalidUrl)?;
        let response = Self::send(
            self.request(Method::PUT, url)
                .header(IF_NONE_MATCH, "*")
                .header("Content-Type", "application/octet-stream")
                .body(bytes.to_owned()),
        )?;
        match response.status().as_u16() {
            200 | 201 | 204 => Ok(()),
            412 => Err(SyncError::Conflict),
            status => Err(SyncError::Http(status)),
        }
    }
    fn list(&self) -> Result<Vec<String>> {
        self.ensure_dirs()?;
        let response = Self::send(self.request(Method::from_bytes(b"PROPFIND").map_err(|_| SyncError::InvalidUrl)?, self.notes.clone())
            .header("Depth", "1").header("Content-Type", "application/xml; charset=utf-8")
            .body("<?xml version=\"1.0\"?><d:propfind xmlns:d=\"DAV:\"><d:prop><d:getetag/><d:resourcetype/></d:prop></d:propfind>"))?;
        if response.status().as_u16() != 207 {
            return Err(SyncError::Http(response.status().as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|len| len > MAX_XML_BYTES as u64)
        {
            return Err(SyncError::LimitExceeded);
        }
        let xml = bounded_read(response, MAX_XML_BYTES)?;
        self.parse_listing(&xml)
    }

    fn read(&self, id: &str) -> Result<Option<RemoteNote>> {
        let response = Self::send(self.request(Method::GET, self.object_url(id)?))?;
        if response.status().as_u16() == 404 {
            return Ok(None);
        }
        if response.status().as_u16() != 200 {
            return Err(SyncError::Http(response.status().as_u16()));
        }
        let revision = Self::strong_etag(&response);
        let content = read_note(response)?;
        validate_content(&content)?;
        let hash = content_hash(&content);
        Ok(Some(RemoteNote {
            content,
            hash,
            revision,
        }))
    }

    fn write(
        &self,
        id: &str,
        content: &str,
        expected_revision: Option<&str>,
    ) -> Result<RemoteNote> {
        let url = self.object_url(id)?;
        validate_content(content)?;
        if expected_revision.is_some_and(|r| {
            r.len() < 2
                || !r.starts_with('"')
                || !r.ends_with('"')
                || r[1..r.len() - 1].contains('"')
        }) {
            return Err(SyncError::MissingRevision);
        }
        self.ensure_dirs()?;
        let request = self
            .request(Method::PUT, url)
            .header("Content-Type", "text/markdown; charset=utf-8")
            .body(content.to_owned());
        let request = match expected_revision {
            Some(revision) => request.header(IF_MATCH, revision),
            None => request.header(IF_NONE_MATCH, "*"),
        };
        let response = Self::send(request)?;
        if response.status().as_u16() == 412 {
            return Err(SyncError::Conflict);
        }
        if !matches!(response.status().as_u16(), 200 | 201 | 204) {
            return Err(SyncError::Http(response.status().as_u16()));
        }
        Ok(RemoteNote {
            content: content.to_owned(),
            hash: content_hash(content),
            revision: Self::strong_etag(&response),
        })
    }

    fn test_connection(&self) -> Result<()> {
        self.list().map(|_| ())
    }
}
