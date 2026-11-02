//! Generic S3 compatible image storage with separately prepared conditional removal.

use crate::{
    AssetProvider, RemoteRemoval, RemovalOutcome, RemovalResidual, Result, SyncError,
    validate_asset_key,
};
use hmac::{Hmac, Mac};
use reqwest::{
    Method, Url,
    blocking::{Client, Response},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io::Read, net::IpAddr, time::Duration};
use zeroize::Zeroizing;

const MAX_ASSET_BYTES: usize = 10 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ObjectAddressing {
    #[default]
    VirtualHosted,
    Path,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObjectStoreConfig {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub prefix: String,
    pub addressing: ObjectAddressing,
}

pub struct ObjectStoreProvider {
    client: Client,
    base: Url,
    config: ObjectStoreConfig,
    access_key_id: Zeroizing<String>,
    secret: Zeroizing<String>,
    removal_supported: bool,
}

struct Signature {
    authorization: String,
    headers: BTreeMap<String, String>,
}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}
fn hash(bytes: &[u8]) -> String {
    hex(Sha256::digest(bytes))
}
fn hmac(key: &[u8], bytes: &[u8]) -> Zeroizing<Vec<u8>> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts arbitrary key lengths");
    mac.update(bytes);
    Zeroizing::new(mac.finalize().into_bytes().to_vec())
}
fn encode_key(value: &str) -> String {
    let mut result = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/') {
            result.push(char::from(byte));
        } else {
            result.push_str(&format!("%{byte:02X}"));
        }
    }
    result
}

impl ObjectStoreProvider {
    pub fn validate_config(config: &ObjectStoreConfig) -> Result<()> {
        Self::base(config, false).map(|_| ())
    }
    fn base(config: &ObjectStoreConfig, loopback: bool) -> Result<Url> {
        let base = Url::parse(&config.endpoint).map_err(|_| SyncError::InvalidUrl)?;
        let is_loopback = base
            .host_str()
            .and_then(|host| host.trim_matches(['[', ']']).parse::<IpAddr>().ok())
            .is_some_and(|ip| ip.is_loopback());
        if (base.scheme() != "https" && !(loopback && base.scheme() == "http" && is_loopback))
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || base.path() != "/"
        {
            return Err(SyncError::InvalidUrl);
        }
        if !(3..=63).contains(&config.bucket.len())
            || !config
                .bucket
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'.'))
            || !config
                .bucket
                .starts_with(|c: char| c.is_ascii_alphanumeric())
            || !config.bucket.ends_with(|c: char| c.is_ascii_alphanumeric())
            || config.region.is_empty()
            || config.region.len() > 128
            || !config
                .region
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            || config.prefix.len() > 512
            || config.prefix.chars().any(char::is_control)
            || config.prefix.contains(['\\', '%', '?', '#'])
            || (!config.prefix.is_empty()
                && config
                    .prefix
                    .split('/')
                    .any(|s| matches!(s, "" | "." | "..")))
        {
            return Err(SyncError::InvalidPath);
        }
        if config.addressing == ObjectAddressing::VirtualHosted
            && base
                .host_str()
                .and_then(|s| s.parse::<IpAddr>().ok())
                .is_some()
        {
            return Err(SyncError::InvalidUrl);
        }
        Ok(base)
    }
    pub fn new(config: ObjectStoreConfig, access_key_id: &str, secret: &str) -> Result<Self> {
        Self::build(config, access_key_id, secret, false)
    }
    #[cfg(test)]
    fn for_loopback_test(
        config: ObjectStoreConfig,
        access_key_id: &str,
        secret: &str,
    ) -> Result<Self> {
        Self::build(config, access_key_id, secret, true)
    }
    fn build(
        config: ObjectStoreConfig,
        access_key_id: &str,
        secret: &str,
        loopback: bool,
    ) -> Result<Self> {
        let base = Self::base(&config, loopback)?;
        if access_key_id.is_empty()
            || access_key_id.len() > 256
            || !access_key_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'@'))
            || secret.is_empty()
            || secret.len() > 16384
        {
            return Err(SyncError::InvalidContent);
        }
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|_| SyncError::Transport)?;
        let host = base.host_str().unwrap_or("");
        let removal_supported = loopback
            || host == "s3.amazonaws.com"
            || host == format!("s3.{}.amazonaws.com", config.region);
        Ok(Self {
            removal_supported,
            client,
            base,
            config,
            access_key_id: Zeroizing::new(access_key_id.into()),
            secret: Zeroizing::new(secret.into()),
        })
    }
    fn bucket_url(&self) -> Result<Url> {
        let mut url = self.base.clone();
        match self.config.addressing {
            ObjectAddressing::VirtualHosted => {
                let host = format!(
                    "{}.{}",
                    self.config.bucket,
                    self.base.host_str().ok_or(SyncError::InvalidUrl)?
                );
                url.set_host(Some(&host))
                    .map_err(|_| SyncError::InvalidUrl)?;
            }
            ObjectAddressing::Path => url.set_path(&format!("/{}/", self.config.bucket)),
        }
        Ok(url)
    }
    fn asset_url(&self, key: &str) -> Result<Url> {
        validate_asset_key(key)?;
        let mut url = self.bucket_url()?;
        let prefix = if self.config.prefix.is_empty() {
            String::new()
        } else {
            format!("{}/", self.config.prefix)
        };
        let path = format!("{}{}{}", url.path(), encode_key(&prefix), encode_key(key));
        url.set_path(&path);
        Ok(url)
    }
    fn sign(
        &self,
        method: &str,
        url: &Url,
        payload: &[u8],
        datetime: &str,
        mut headers: BTreeMap<String, String>,
    ) -> Result<Signature> {
        if datetime.len() != 16 || !datetime.is_ascii() {
            return Err(SyncError::InvalidContent);
        }
        let host = match url.port() {
            Some(port) => format!("{}:{port}", url.host_str().ok_or(SyncError::InvalidUrl)?),
            None => url.host_str().ok_or(SyncError::InvalidUrl)?.to_owned(),
        };
        let payload_hash = hash(payload);
        headers.insert("host".into(), host);
        headers.insert("x-amz-content-sha256".into(), payload_hash.clone());
        headers.insert("x-amz-date".into(), datetime.into());
        let signed_headers = headers.keys().cloned().collect::<Vec<_>>().join(";");
        let canonical_headers = headers
            .iter()
            .map(|(key, value)| {
                format!(
                    "{key}:{}\n",
                    value.split_whitespace().collect::<Vec<_>>().join(" ")
                )
            })
            .collect::<String>();
        let mut query = url
            .query_pairs()
            .map(|(k, v)| {
                (
                    encode_key(&k).replace('/', "%2F"),
                    encode_key(&v).replace('/', "%2F"),
                )
            })
            .collect::<Vec<_>>();
        query.sort();
        let query = query
            .into_iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        let canonical = format!(
            "{method}\n{}\n{query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}",
            url.path()
        );
        let date = &datetime[..8];
        let scope = format!("{date}/{}/s3/aws4_request", self.config.region);
        let to_sign = format!(
            "AWS4-HMAC-SHA256\n{datetime}\n{scope}\n{}",
            hash(canonical.as_bytes())
        );
        let initial = Zeroizing::new(format!("AWS4{}", self.secret.as_str()));
        let date_key = hmac(initial.as_bytes(), date.as_bytes());
        let region_key = hmac(&date_key, self.config.region.as_bytes());
        let service_key = hmac(&region_key, b"s3");
        let signing_key = hmac(&service_key, b"aws4_request");
        let signature = hex(hmac(&signing_key, to_sign.as_bytes()).as_slice());
        Ok(Signature {
            authorization: format!(
                "AWS4-HMAC-SHA256 Credential={}/{scope},SignedHeaders={signed_headers},Signature={signature}",
                self.access_key_id.as_str()
            ),
            headers,
        })
    }
    fn request(
        &self,
        method: Method,
        url: Url,
        payload: &[u8],
        create_only: bool,
    ) -> Result<Response> {
        let headers = if create_only {
            BTreeMap::from([("if-none-match".into(), "*".into())])
        } else {
            BTreeMap::new()
        };
        self.request_headers(method, url, payload, headers)
    }
    fn request_headers(
        &self,
        method: Method,
        url: Url,
        payload: &[u8],
        headers: BTreeMap<String, String>,
    ) -> Result<Response> {
        let date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let signature = self.sign(method.as_str(), &url, payload, &date, headers)?;
        let mut request = self
            .client
            .request(method.clone(), url)
            .header("Authorization", &signature.authorization);
        for (key, value) in signature.headers {
            request = request.header(key, value);
        }
        if method == Method::PUT {
            request = request.body(payload.to_vec());
        }
        request.send().map_err(|_| SyncError::Transport)
    }
}

impl ObjectStoreProvider {
    fn require_unversioned(&self) -> Result<()> {
        if !self.removal_supported {
            return Err(SyncError::UnsupportedRemoval(
                "endpoint conditional DELETE capability is unverified; only standard AWS S3 endpoints supported",
            ));
        }
        let unsupported = || {
            SyncError::UnsupportedRemoval(
                "bucket version retention is enabled, suspended or unverified",
            )
        };
        let mut url = self.bucket_url()?;
        url.set_query(Some("versioning="));
        let response = self.request(Method::GET, url, &[], false)?;
        if response.status().as_u16() != 200 {
            return Err(unsupported());
        }
        let xml = crate::bounded_read(response, crate::MAX_XML_BYTES)?;
        let mut reader = quick_xml::Reader::from_str(&xml);
        let mut seen = false;
        let mut depth = 0;
        // Only the documented empty VersioningConfiguration proves never-versioned.
        // Any status, unknown child, malformed XML or unsupported API fails closed.
        loop {
            use quick_xml::events::Event;
            match reader.read_event().map_err(|_| unsupported())? {
                Event::Start(e)
                    if !seen
                        && depth == 0
                        && e.local_name().as_ref() == b"VersioningConfiguration" =>
                {
                    seen = true;
                    depth = 1;
                }
                Event::Empty(e)
                    if !seen
                        && depth == 0
                        && e.local_name().as_ref() == b"VersioningConfiguration" =>
                {
                    seen = true;
                }
                Event::End(e)
                    if depth == 1 && e.local_name().as_ref() == b"VersioningConfiguration" =>
                {
                    depth = 0;
                }
                Event::Text(e) if e.as_ref().iter().all(u8::is_ascii_whitespace) => {}
                Event::Decl(_) | Event::Comment(_) => {}
                Event::Eof if seen && depth == 0 => return Ok(()),
                _ => return Err(unsupported()),
            }
        }
    }
    fn inspect_removal_object(&self, key: &str) -> Result<RemoteRemoval> {
        let response = self.request(Method::GET, self.asset_url(key)?, &[], false)?;
        match response.status().as_u16() {
            200 => {}
            404 => return Err(SyncError::Conflict),
            status => return Err(SyncError::Http(status)),
        }
        if response
            .headers()
            .get("x-amz-version-id")
            .is_some_and(|v| v != "null")
        {
            return Err(SyncError::UnsupportedRemoval(
                "object has a retained version",
            ));
        }
        let revision = response
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .filter(|v| {
                v.len() >= 2
                    && v.starts_with('"')
                    && v.ends_with('"')
                    && !v[1..v.len() - 1].contains('"')
            })
            .ok_or(SyncError::MissingRevision)?
            .to_owned();
        let bytes = crate::bounded_bytes(response)?;
        Ok(RemoteRemoval {
            confirmed_retained: Vec::new(),
            target: key.into(),
            revision,
            hash: hash(&bytes),
            bytes: bytes.len() as u64,
            provider_identity: self.identity().ok_or(SyncError::InvalidUrl)?,
            residual: RemovalResidual::ProviderBackupsUnknown,
        })
    }
}
impl AssetProvider for ObjectStoreProvider {
    fn inspect_asset(&self, key: &str) -> Result<RemoteRemoval> {
        validate_asset_key(key)?;
        self.require_unversioned()?;
        self.inspect_removal_object(key)
    }
    fn delete_asset(&self, plan: &RemoteRemoval) -> Result<RemovalOutcome> {
        validate_asset_key(&plan.target)?;
        if self.identity().as_deref() != Some(plan.provider_identity.as_str()) {
            return Err(SyncError::Conflict);
        }
        self.require_unversioned()?;
        if &self.inspect_removal_object(&plan.target)? != plan {
            return Err(SyncError::Conflict);
        }
        let response = self.request_headers(
            Method::DELETE,
            self.asset_url(&plan.target)?,
            &[],
            BTreeMap::from([("if-match".into(), plan.revision.clone())]),
        )?;
        match response.status().as_u16() {
            204 => {}
            404 | 412 => return Err(SyncError::Conflict),
            status => return Err(SyncError::Http(status)),
        }
        if response
            .headers()
            .get("x-amz-delete-marker")
            .is_some_and(|v| v == "true")
            || response
                .headers()
                .get("x-amz-version-id")
                .is_some_and(|v| v != "null")
        {
            return Ok(RemovalOutcome::remote(false));
        }
        let verified = self
            .request(Method::GET, self.asset_url(&plan.target)?, &[], false)
            .is_ok_and(|response| response.status().as_u16() == 404);
        Ok(RemovalOutcome::remote(verified))
    }

    fn get_asset(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let response = self.request(Method::GET, self.asset_url(key)?, &[], false)?;
        match response.status().as_u16() {
            200 => {}
            404 => return Ok(None),
            status => return Err(SyncError::Http(status)),
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_ASSET_BYTES as u64)
        {
            return Err(SyncError::LimitExceeded);
        }
        let mut bytes = Vec::new();
        response
            .take(MAX_ASSET_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_ASSET_BYTES {
            return Err(SyncError::LimitExceeded);
        }
        Ok(Some(bytes))
    }
    fn put_asset(&self, key: &str, bytes: &[u8]) -> Result<()> {
        if bytes.len() > MAX_ASSET_BYTES {
            return Err(SyncError::LimitExceeded);
        }
        let response = self.request(Method::PUT, self.asset_url(key)?, bytes, true)?;
        match response.status().as_u16() {
            200 | 201 | 204 => Ok(()),
            412 => {
                if self.get_asset(key)?.as_deref() == Some(bytes) {
                    Ok(())
                } else {
                    Err(SyncError::Conflict)
                }
            }
            status => Err(SyncError::Http(status)),
        }
    }
    fn check_connection(&self) -> Result<()> {
        let response = self.request(Method::HEAD, self.bucket_url()?, &[], false)?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(SyncError::Http(response.status().as_u16()))
        }
    }
    fn identity(&self) -> Option<String> {
        let public = format!(
            "{}|{}|{}|{}|{:?}|{}",
            self.base,
            self.config.bucket,
            self.config.region,
            self.config.prefix,
            self.config.addressing,
            self.access_key_id.as_str()
        );
        Some(format!("s3-{}", hash(public.as_bytes())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn config(endpoint: &str) -> ObjectStoreConfig {
        ObjectStoreConfig {
            endpoint: endpoint.into(),
            bucket: "study-images".into(),
            region: "us-east-1".into(),
            prefix: "study app/图片".into(),
            addressing: ObjectAddressing::Path,
        }
    }

    #[test]
    fn published_aws_sigv4_get_vector_matches() {
        let provider = ObjectStoreProvider::new(
            ObjectStoreConfig {
                endpoint: "https://s3.amazonaws.com".into(),
                bucket: "examplebucket".into(),
                region: "us-east-1".into(),
                prefix: String::new(),
                addressing: ObjectAddressing::VirtualHosted,
            },
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        )
        .unwrap();
        let url = reqwest::Url::parse("https://examplebucket.s3.amazonaws.com/test.txt").unwrap();
        let headers = std::collections::BTreeMap::from([("range".into(), "bytes=0-9".into())]);
        let signature = provider
            .sign("GET", &url, &[], "20130524T000000Z", headers)
            .unwrap();
        assert!(signature.authorization.ends_with(
            "Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        ));
    }

    #[test]
    fn address_styles_and_utf8_prefix_preserve_scoped_key() {
        let key = "assets/00000000-0000-4000-8000-000000000001/image.png";
        let provider =
            ObjectStoreProvider::new(config("https://objects.example.test"), "id", "secret")
                .unwrap();
        let url = provider.asset_url(key).unwrap();
        assert_eq!(
            url.path(),
            "/study-images/study%20app/%E5%9B%BE%E7%89%87/assets/00000000-0000-4000-8000-000000000001/image.png"
        );
        let mut target = config("https://objects.example.test");
        target.addressing = ObjectAddressing::VirtualHosted;
        let provider = ObjectStoreProvider::new(target, "id", "secret").unwrap();
        assert_eq!(
            provider.asset_url(key).unwrap().host_str(),
            Some("study-images.objects.example.test")
        );
        for key in [
            "../secret",
            "assets/../../secret",
            "https://other.example/secret",
        ] {
            assert!(provider.asset_url(key).is_err());
        }
    }

    #[test]
    fn configuration_rejects_unsafe_endpoints_and_ambiguous_prefixes() {
        for endpoint in [
            "http://objects.example.test",
            "https://id:secret@objects.example.test",
            "https://objects.example.test/?token=secret",
            "https://objects.example.test/subpath",
            "https://objects.example.test/#fragment",
        ] {
            assert!(ObjectStoreProvider::new(config(endpoint), "id", "secret").is_err());
        }
        for prefix in ["../private", "/absolute", "a//b", "a/./b", "a\\b", "a%2fb"] {
            let mut target = config("https://objects.example.test");
            target.prefix = prefix.into();
            assert!(ObjectStoreProvider::new(target, "id", "secret").is_err());
        }
    }

    fn fixture(
        responses: Vec<(&'static str, &'static [u8])>,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 1024];
                let end = loop {
                    let n = stream.read(&mut buffer).unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buffer[..n]);
                    if let Some(pos) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                        break pos + 4;
                    }
                };
                let header = String::from_utf8(bytes[..end].to_vec()).unwrap();
                let length: usize = header
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .map(|(_, value)| value.trim().parse().unwrap())
                    .unwrap_or(0);
                while bytes.len() < end + length {
                    let n = stream.read(&mut buffer).unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buffer[..n]);
                }
                requests.push(header.to_lowercase());
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(body).unwrap();
            }
            requests
        });
        (endpoint, worker)
    }

    #[test]
    fn mocked_put_is_signed_create_only_and_retry_verifies_existing_bytes() {
        let (endpoint, server) = fixture(vec![
            ("200 OK", b""),
            ("412 Precondition Failed", b""),
            ("200 OK", b"image"),
        ]);
        let provider =
            ObjectStoreProvider::for_loopback_test(config(&endpoint), "account", "secret").unwrap();
        let key = "assets/00000000-0000-4000-8000-000000000001/image.png";
        provider.put_asset(key, b"image").unwrap();
        provider.put_asset(key, b"image").unwrap();
        let requests = server.join().unwrap();
        assert!(requests[0].starts_with("put /study-images/study%20app/"));
        assert!(requests[0].contains("if-none-match: *"));
        assert!(requests[0].contains("authorization: aws4-hmac-sha256 credential=account/"));
        assert!(requests[0].contains("x-amz-content-sha256:"));
        assert!(requests[2].starts_with("get "));
    }

    #[test]
    fn mocked_read_missing_conflict_and_http_failure_are_bounded_and_redacted() {
        let (endpoint, server) = fixture(vec![
            ("404 Not Found", b""),
            ("412 Precondition Failed", b""),
            ("200 OK", b"other"),
            ("403 Forbidden", b"private server response"),
        ]);
        let provider = ObjectStoreProvider::for_loopback_test(
            config(&endpoint),
            "account",
            "do-not-print-secret",
        )
        .unwrap();
        let key = "assets/00000000-0000-4000-8000-000000000001/image.png";
        assert!(provider.get_asset(key).unwrap().is_none());
        assert!(matches!(
            provider.put_asset(key, b"image"),
            Err(crate::SyncError::Conflict)
        ));
        let error = provider.get_asset(key).unwrap_err().to_string();
        assert!(
            !error.contains("do-not-print-secret")
                && !error.contains(&endpoint)
                && !error.contains("private server response")
        );
        server.join().unwrap();
    }

    #[test]
    fn redirect_is_rejected_and_oversized_upload_never_contacts_server() {
        let (endpoint, server) = fixture(vec![(
            "307 Temporary Redirect\r\nLocation: https://outside.example.test/private",
            b"",
        )]);
        let provider =
            ObjectStoreProvider::for_loopback_test(config(&endpoint), "account", "private")
                .unwrap();
        let key = "assets/00000000-0000-4000-8000-000000000001/image.png";
        assert!(matches!(
            provider.put_asset(key, &vec![0; MAX_ASSET_BYTES + 1]),
            Err(crate::SyncError::LimitExceeded)
        ));
        assert!(matches!(
            provider.get_asset(key),
            Err(crate::SyncError::Http(307))
        ));
        assert_eq!(server.join().unwrap().len(), 1);
    }
}

#[cfg(test)]
mod removal_tests {
    use super::*;
    use crate::{RemovalResidual, removal_tests::removal_server};
    const KEY: &str = "assets/123e4567-e89b-42d3-a456-426614174000/a.png";
    fn provider(url: &str) -> ObjectStoreProvider {
        ObjectStoreProvider::for_loopback_test(
            ObjectStoreConfig {
                endpoint: url.into(),
                bucket: "bucket".into(),
                region: "us-east-1".into(),
                prefix: String::new(),
                addressing: ObjectAddressing::Path,
            },
            "testkey",
            "testsecret",
        )
        .unwrap()
    }
    const UNVERSIONED: &str =
        "<VersioningConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"/>";
    #[test]
    fn removal_object_signed_conditional_delete() {
        let (url, server) = removal_server(vec![
            (200, "", UNVERSIONED),
            (200, "ETag: \"v1\"\r\n", "image"),
            (200, "", UNVERSIONED),
            (200, "ETag: \"v1\"\r\n", "image"),
            (204, "", ""),
            (404, "", ""),
        ]);
        let p = provider(&url);
        let plan = p.inspect_asset(KEY).unwrap();
        assert_eq!(plan.hash, crate::content_hash("image"));
        assert_eq!(
            p.delete_asset(&plan).unwrap().residual,
            RemovalResidual::ProviderBackupsUnknown
        );
        let calls = server.join().unwrap();
        assert!(calls[0].starts_with("GET /bucket/?versioning="));
        let delete = calls[4].to_lowercase();
        assert!(delete.starts_with("delete /bucket/assets/"));
        assert!(delete.contains("if-match: \"v1\""));
        assert!(delete.contains("signedheaders=host;if-match;x-amz-content-sha256;x-amz-date"));
    }
    #[test]
    fn removal_object_versioned_unknown_denied_without_mutation() {
        for body in [
            "<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>",
            "<VersioningConfiguration><Status>Suspended</Status></VersioningConfiguration>",
            "<Unknown/>",
            "<VersioningConfiguration><Status>Unknown</Status></VersioningConfiguration>",
            "<VersioningConfiguration><Status/></VersioningConfiguration>",
        ] {
            let (url, server) = removal_server(vec![(200, "", body)]);
            let p = provider(&url);
            assert!(matches!(
                p.inspect_asset(KEY),
                Err(SyncError::UnsupportedRemoval(_))
            ));
            assert_eq!(server.join().unwrap().len(), 1);
        }
    }
}

#[cfg(test)]
mod removal_guards_tests {
    use super::*;
    use crate::removal_tests::removal_server;
    const KEY: &str = "assets/123e4567-e89b-42d3-a456-426614174000/a.png";
    const EMPTY: &str = "<VersioningConfiguration/>";
    fn provider(url: &str) -> ObjectStoreProvider {
        ObjectStoreProvider::for_loopback_test(
            ObjectStoreConfig {
                endpoint: url.into(),
                bucket: "bucket".into(),
                region: "us-east-1".into(),
                prefix: String::new(),
                addressing: ObjectAddressing::Path,
            },
            "testkey",
            "testsecret",
        )
        .unwrap()
    }
    #[test]
    fn removal_object_rechecks_version_state_before_delete() {
        let (url, server) = removal_server(vec![
            (200, "", EMPTY),
            (200, "ETag: \"v\"\r\n", "x"),
            (
                200,
                "",
                "<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>",
            ),
        ]);
        let p = provider(&url);
        let plan = p.inspect_asset(KEY).unwrap();
        assert!(matches!(
            p.delete_asset(&plan),
            Err(SyncError::UnsupportedRemoval(_))
        ));
        assert!(server.join().unwrap().iter().all(|r| r.starts_with("GET ")));
    }
    #[test]
    fn removal_object_missing_etag_auth_lock_conflict_and_redirect_fail_closed() {
        for headers in [
            "",
            "ETag: W/\"weak\"\r\n",
            "ETag: \"v\"\r\nx-amz-version-id: version1\r\n",
        ] {
            let (url, server) = removal_server(vec![(200, "", EMPTY), (200, headers, "x")]);
            assert!(provider(&url).inspect_asset(KEY).is_err());
            assert_eq!(server.join().unwrap().len(), 2);
        }
        for status in [403, 412, 302, 404, 500] {
            let (url, server) = removal_server(vec![
                (200, "", EMPTY),
                (200, "ETag: \"v\"\r\n", "x"),
                (200, "", EMPTY),
                (200, "ETag: \"v\"\r\n", "x"),
                (status, "", ""),
            ]);
            let p = provider(&url);
            let plan = p.inspect_asset(KEY).unwrap();
            assert!(p.delete_asset(&plan).is_err());
            assert_eq!(server.join().unwrap().len(), 5);
        }
    }
    #[test]
    fn removal_object_sigv4_canonical_query_is_sorted_encoded_and_signed() {
        let p = provider("http://127.0.0.1:1/");
        let sign = |query: &str| {
            p.sign(
                "GET",
                &Url::parse(&format!("http://127.0.0.1:1/bucket/?{query}")).unwrap(),
                &[],
                "20260101T000000Z",
                BTreeMap::new(),
            )
            .unwrap()
            .authorization
        };
        assert_eq!(sign("b=two%20words&a=%2F"), sign("a=%2F&b=two+words"));
        assert_ne!(sign("versioning="), sign(""));
        assert_ne!(sign("a=%2F"), sign("a=%252F"));
    }
}

#[cfg(test)]
mod removal_review_tests {
    use super::*;
    #[test]
    fn removal_unknown_s3_endpoint_is_unsupported_without_requests() {
        let p = ObjectStoreProvider::new(
            ObjectStoreConfig {
                endpoint: "https://127.0.0.1:1".into(),
                bucket: "bucket".into(),
                region: "us-east-1".into(),
                prefix: String::new(),
                addressing: ObjectAddressing::Path,
            },
            "testkey",
            "secret",
        )
        .unwrap();
        assert!(matches!(
            p.inspect_asset("assets/123e4567-e89b-42d3-a456-426614174000/a.png"),
            Err(SyncError::UnsupportedRemoval(_))
        ));
    }
    #[test]
    fn removal_s3_acknowledged_but_get_forbidden_is_unverified() {
        let (url, server) = crate::removal_tests::removal_server(vec![
            (200, "", "<VersioningConfiguration/>"),
            (200, "ETag: \"v\"\r\n", "x"),
            (200, "", "<VersioningConfiguration/>"),
            (200, "ETag: \"v\"\r\n", "x"),
            (204, "", ""),
            (403, "", ""),
        ]);
        let p = ObjectStoreProvider::for_loopback_test(
            ObjectStoreConfig {
                endpoint: url,
                bucket: "bucket".into(),
                region: "us-east-1".into(),
                prefix: String::new(),
                addressing: ObjectAddressing::Path,
            },
            "testkey",
            "secret",
        )
        .unwrap();
        let plan = p
            .inspect_asset("assets/123e4567-e89b-42d3-a456-426614174000/a.png")
            .unwrap();
        assert_eq!(
            p.delete_asset(&plan).unwrap().state,
            crate::RemovalState::AcknowledgedUnverified
        );
        server.join().unwrap();
    }
}
