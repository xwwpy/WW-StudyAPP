//! Runner-only native repair acceptance through the real settings panel and repair worker.
use super::*;
use anyhow::{Context as _, Result, ensure};
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::test::TestWindowExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use studyapp_desktop::maintenance::{FileRole, InstallOwner, PackageManifest, RECEIPT_PATH};

struct Counts {
    total: AtomicUsize,
    metadata: AtomicUsize,
    checksum: AtomicUsize,
    package: AtomicUsize,
}

impl Counts {
    fn snapshot(&self) -> [usize; 4] {
        [
            self.total.load(Ordering::SeqCst),
            self.metadata.load(Ordering::SeqCst),
            self.checksum.load(Ordering::SeqCst),
            self.package.load(Ordering::SeqCst),
        ]
    }
}

struct Loopback {
    url: reqwest::Url,
    counts: Arc<Counts>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<Result<()>>>,
}

impl Loopback {
    fn start(package: &Path, hash: &str) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let url = reqwest::Url::parse(&format!("http://{}/", listener.local_addr()?))?;
        let counts = Arc::new(Counts {
            total: AtomicUsize::new(0),
            metadata: AtomicUsize::new(0),
            checksum: AtomicUsize::new(0),
            package: AtomicUsize::new(0),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let worker_counts = counts.clone();
        let worker_stop = stop.clone();
        let package = package.to_owned();
        let version = env!("CARGO_PKG_VERSION");
        let name = format!("WW-StudyApp-{version}-windows-x86_64-portable.zip");
        let size = fs::metadata(&package)?.len();
        let checksum = format!("{hash}  {name}\n").into_bytes();
        let checksum_hash = format!("{:x}", Sha256::digest(&checksum));
        let canonical =
            format!("https://github.com/xwwpy/WW-StudyAPP/releases/download/v{version}/{name}");
        let metadata = serde_json::to_vec(&json!({
            "tag_name":format!("v{version}"), "draft":false, "prerelease":false,
            "body":"Current-version repair fixture",
            "html_url":format!("https://github.com/xwwpy/WW-StudyAPP/releases/tag/v{version}"),
            "assets":[
                {"name":name,"browser_download_url":canonical,"state":"uploaded","size":size,"digest":format!("sha256:{hash}")},
                {"name":format!("{name}.sha256"),"browser_download_url":format!("{canonical}.sha256"),"state":"uploaded","size":checksum.len(),"digest":format!("sha256:{checksum_hash}")}
            ]
        }))?;
        let thread = std::thread::spawn(move || -> Result<()> {
            let deadline = Instant::now() + Duration::from_secs(120);
            while !worker_stop.load(Ordering::Relaxed) && Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        ensure!(
                            worker_counts.total.fetch_add(1, Ordering::SeqCst) < 8,
                            "Too many repair fixture requests"
                        );
                        // Windows accepts inherit the listener's nonblocking
                        // mode; bounded response I/O must wait for backpressure.
                        stream.set_nonblocking(false)?;
                        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
                        stream.set_write_timeout(Some(Duration::from_secs(30)))?;
                        let mut line = String::new();
                        BufReader::new(&stream).read_line(&mut line)?;
                        ensure!(line.len() <= 2048, "Loopback request line too long");
                        match line.trim_end() {
                            "GET /metadata HTTP/1.1" => {
                                let attempt = worker_counts.metadata.fetch_add(1, Ordering::SeqCst);
                                if attempt == 0 {
                                    reply(&mut stream, 503, b"offline")?;
                                } else {
                                    reply(&mut stream, 200, &metadata)?;
                                }
                            }
                            "GET /checksum HTTP/1.1" => {
                                worker_counts.checksum.fetch_add(1, Ordering::SeqCst);
                                reply(&mut stream, 200, &checksum)?;
                            }
                            "GET /package HTTP/1.1" => {
                                worker_counts.package.fetch_add(1, Ordering::SeqCst);
                                write!(
                                    stream,
                                    "HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n"
                                )?;
                                std::io::copy(&mut File::open(&package)?, &mut stream)?;
                            }
                            _ => reply(&mut stream, 404, b"unknown route")?,
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(20))
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            ensure!(
                worker_stop.load(Ordering::Relaxed),
                "Loopback repair server timed out"
            );
            Ok(())
        });
        Ok(Self {
            url,
            counts,
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take()
            && let Ok(Err(error)) = thread.join()
        {
            eprintln!("repair fixture transport: {error:#}");
        }
    }
}

fn reply(stream: &mut TcpStream, status: u16, body: &[u8]) -> Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status} fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    Ok(())
}

fn create_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn unchanged(paths: &[PathBuf], before: &[Vec<u8>]) -> Result<bool> {
    for (path, expected) in paths.iter().zip(before) {
        if fs::read(path)? != *expected {
            return Ok(false);
        }
    }
    Ok(true)
}

fn children(directory: &Path) -> Result<Vec<String>> {
    let mut names = fs::read_dir(directory)?
        .map(|entry| {
            entry?
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("Non-Unicode fixture entry"))
        })
        .collect::<Result<Vec<_>>>()?;
    names.sort();
    Ok(names)
}

fn fixture_members(package: &Path) -> Result<Vec<(String, Vec<u8>)>> {
    let mut zip = zip::ZipArchive::new(File::open(package)?)?;
    let prefix = format!(
        "WW-StudyApp-{}-windows-x86_64-portable/",
        env!("CARGO_PKG_VERSION")
    );
    let mut manifest = None;
    let mut selected = Vec::new();
    for index in 0..zip.len() {
        let mut member = zip.by_index(index)?;
        let name = member.name()?.into_owned();
        if name == format!("{prefix}support/manifest.json") {
            let mut bytes = Vec::new();
            member.read_to_end(&mut bytes)?;
            manifest = Some(serde_json::from_slice::<PackageManifest>(&bytes)?);
        } else if ["WW-StudyApp.exe", "vcruntime140.dll", "vcruntime140_1.dll"]
            .iter()
            .any(|relative| name == format!("{prefix}{relative}"))
        {
            let mut bytes = Vec::new();
            member.read_to_end(&mut bytes)?;
            selected.push((name.strip_prefix(&prefix).unwrap().to_owned(), bytes));
        }
    }
    let manifest = manifest.context("Production package manifest missing")?;
    ensure!(
        manifest.version == env!("CARGO_PKG_VERSION") && selected.len() == 3,
        "Fixture package is not the exact current version"
    );
    for (name, bytes) in &selected {
        let owned = manifest
            .files
            .iter()
            .find(|entry| &entry.relative_path == name)
            .context("Fixture component missing from manifest")?;
        ensure!(
            matches!(owned.role, FileRole::Application | FileRole::Runtime)
                && owned.size == bytes.len() as u64
                && owned.sha256 == format!("{:x}", Sha256::digest(bytes)),
            "Fixture component differs from manifest"
        );
    }
    Ok(selected)
}

async fn wait_state(
    panel: &Entity<UpdatePanel>,
    expected_repair: bool,
    expected_error: bool,
    counts: &Counts,
    cx: &mut AsyncApp,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let (state, error) = cx.update(|app| {
            let panel = panel.read(app);
            (
                (!panel.busy, panel.repair_needed, panel.error.is_some()),
                panel.error.clone(),
            )
        });
        if state == (true, expected_repair, expected_error) {
            return Ok(());
        }
        ensure!(
            Instant::now() < deadline,
            "Repair UI worker did not reach expected state: state={state:?}, error={error:?}, counts(total,metadata,checksum,package)={:?}",
            counts.snapshot()
        );
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
    }
}

fn local_fixture_path(path: &Path) -> Result<PathBuf> {
    use std::path::{Component, Prefix};
    let path = match path.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(_) => path.to_owned(),
            Prefix::VerbatimDisk(_) => PathBuf::from(
                path.to_str()
                    .context("Non-Unicode fixture path")?
                    .strip_prefix(r"\\?\")
                    .context("Unsupported verbatim fixture prefix")?,
            ),
            _ => anyhow::bail!("Fixture must use a local drive path"),
        },
        _ => anyhow::bail!("Fixture must use an absolute local drive path"),
    };
    studyapp_desktop::maintenance::absolute_components(&path)?;
    Ok(path)
}

pub(crate) fn run_repair_acceptance(fixture_root: &Path, package: &Path) -> Result<Value> {
    ensure!(
        std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
            && std::env::var("RUNNER_OS").as_deref() == Ok("Windows"),
        "Repair acceptance requires isolated Windows CI"
    );
    let runner =
        fs::canonicalize(std::env::var_os("RUNNER_TEMP").context("RUNNER_TEMP required")?)?;
    let root = fs::canonicalize(fixture_root)?;
    ensure!(
        root != runner && root.starts_with(&runner),
        "Repair fixture must be under RUNNER_TEMP"
    );
    // Keep canonical paths for the boundary check above. Maintenance accepts
    // local drive paths, so normalize only the canonical VerbatimDisk prefix.
    let root = local_fixture_path(&root)?;
    let package = fs::canonicalize(package)?;
    let package_hash = crate::maintenance_runtime::observe_file(&package, "package")?
        .identity
        .sha256;
    let install = root.join("offline-repair-install");
    let library = root.join("offline-repair-library");
    fs::create_dir(&install)?;
    fs::create_dir(&library)?;
    let target = install.join("WW-StudyApp.exe");
    let members = fixture_members(&package)?;
    for (name, bytes) in &members {
        create_new(&install.join(name), bytes)?;
    }
    create_new(&install.join("unknown.txt"), b"unknown install neighbor")?;
    create_new(
        &library.join("studyapp.sqlite3"),
        b"preserved private library",
    )?;
    create_new(&library.join("notes.txt"), b"preserved notes")?;
    create_new(&library.join("image.png"), b"preserved image")?;
    create_new(&library.join("backup.zip"), b"preserved backup")?;
    let protected = [
        install.join("unknown.txt"),
        library.join("studyapp.sqlite3"),
        library.join("notes.txt"),
        library.join("image.png"),
        library.join("backup.zip"),
    ];
    let before = protected
        .iter()
        .map(fs::read)
        .collect::<std::io::Result<Vec<_>>>()?;
    let app_before = members
        .iter()
        .map(|(name, _)| fs::read(install.join(name)))
        .collect::<std::io::Result<Vec<_>>>()?;
    let install_entries_before = children(&install)?;
    let library_entries_before = children(&library)?;
    ensure!(
        !install.join(RECEIPT_PATH).try_exists()? && !install.join("Uninstall.exe").try_exists()?,
        "Fixture is already repaired"
    );
    let server = Loopback::start(&package, &package_hash)?;
    let transport = RepairTransport::fixture_for_runner(&root, &target, server.url.clone())?;
    let counters = server.counts.clone();
    let automatic_network_downloads = counters.package.load(Ordering::SeqCst);
    ensure!(
        counters.metadata.load(Ordering::SeqCst) == 0 && automatic_network_downloads == 0,
        "Network activity before repair click"
    );
    let failed = root.join("offline-repair-failed.png");
    let succeeded = root.join("offline-repair-succeeded.png");
    let result = Rc::new(RefCell::new(None));
    let result_for_app = result.clone();
    let library_path = library
        .join("studyapp.sqlite3")
        .to_string_lossy()
        .into_owned();
    application()
        .with_assets(crate::assets::StudyAssets)
        .run(move |cx| {
            init(cx);
            Theme::change(ThemeMode::Light, None, cx);
            let theme = Theme::global_mut(cx);
            theme.font_family = studyapp_ui::design::UI_FONT.into();
            theme.font_size = px(14.);
            theme.colors.primary = rgb(studyapp_ui::components::GREEN).into();
            theme.colors.primary_foreground = rgb(0xFFFFFF).into();
            let options = WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("WW-StudyApp 软件更新".into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(700.), px(500.)),
                    cx,
                ))),
                ..Default::default()
            };
            let (handle, panel) = open_window(options, cx, move |_, cx| {
                cx.new(|cx| {
                    let mut panel = UpdatePanel::new(cx);
                    panel.fixture = Some(CheckFixture {
                        result: Ok(UpdateCheck::UpToDate {
                            current_version: env!("CARGO_PKG_VERSION").into(),
                        }),
                        checks: Default::default(),
                        downloads: Default::default(),
                    });
                    panel.library = library_path;
                    panel.isolated_launch = true;
                    panel.repair_needed = true;
                    panel.repair_fixture = Some((target, transport));
                    panel
                })
            })
            .expect("创建修复验收窗口失败");
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.spawn(async move |cx| {
                let outcome: Result<Value> = async {
                    let mut repair_clicks = 0usize;
                    cx.background_executor()
                        .timer(Duration::from_millis(500))
                        .await;
                    handle.update(cx, |_, window, cx| {
                        window.render_frame(cx);
                        window.find("repair-installation");
                        assert!(window.try_find("download-update").is_none());
                        window.click("repair-installation", cx);
                    })?;
                    repair_clicks += 1;
                    wait_state(&panel, true, true, &counters, cx).await?;
                    ensure!(
                        counters.metadata.load(Ordering::SeqCst) == 1
                            && counters.checksum.load(Ordering::SeqCst) == 0
                            && counters.package.load(Ordering::SeqCst) == 0,
                        "First click did not fail before download: counts(total,metadata,checksum,package)={:?}, panel_error={:?}",
                        counters.snapshot(),
                        cx.update(|app| panel.read(app).error.clone())
                    );
                    let app_paths = members
                        .iter()
                        .map(|(name, _)| install.join(name))
                        .collect::<Vec<_>>();
                    ensure!(
                        unchanged(&protected, &before)?
                            && unchanged(&app_paths, &app_before)?
                            && children(&install)? == install_entries_before
                            && children(&library)? == library_entries_before
                            && !install.join(RECEIPT_PATH).try_exists()?,
                        "Failed repair changed installation or data"
                    );
                    handle.update(cx, |_, window, cx| {
                        window.render_frame(cx);
                        window.find("repair-installation");
                        window.render_to_image().unwrap().save(&failed).unwrap();
                        window.click("repair-installation", cx);
                    })?;
                    repair_clicks += 1;
                    wait_state(&panel, false, false, &counters, cx).await?;
                    ensure!(
                        counters.metadata.load(Ordering::SeqCst) == 2
                            && counters.checksum.load(Ordering::SeqCst) == 1
                            && counters.package.load(Ordering::SeqCst) == 1
                            && counters.total.load(Ordering::SeqCst) == 4,
                        "Retry request counts differ from actual transport: counts(total,metadata,checksum,package)={:?}, panel_error={:?}",
                        counters.snapshot(),
                        cx.update(|app| panel.read(app).error.clone())
                    );
                    ensure!(
                        unchanged(&protected, &before)?,
                        "Repair changed unknown or library files"
                    );
                    let receipt = crate::maintenance_runtime::read_portable_receipt(
                        &install.join("WW-StudyApp.exe"),
                    )?;
                    ensure!(
                        receipt.owner == InstallOwner::Portable
                            && receipt.version == env!("CARGO_PKG_VERSION")
                            && receipt.source_package_sha256 == package_hash,
                        "Repair receipt is not current verified package"
                    );
                    for owned in &receipt.files {
                        let actual = crate::maintenance_runtime::observe_file(
                            &install.join(&owned.relative_path),
                            "repaired",
                        )?;
                        ensure!(
                            actual.identity.size == owned.size
                                && actual.identity.sha256 == owned.sha256,
                            "Repaired file differs from receipt"
                        );
                    }
                    let ui_state = cx.update(|app| {
                        let panel_state = panel.read(app);
                        (
                            panel_state.available_version().is_none(),
                            panel_state.prepared.is_none(),
                            panel_state.error.is_none(),
                            !panel_state.repair_needed,
                            panel_state
                                .fixture
                                .as_ref()
                                .unwrap()
                                .downloads
                                .load(Ordering::SeqCst),
                        )
                    });
                    ensure!(
                        ui_state == (true, true, true, true, 0),
                        "Current-version repair appeared as update or used CheckFixture download"
                    );
                    handle.update(cx, |_, window, cx| {
                        window.render_frame(cx);
                        assert!(window.try_find("download-update").is_none());
                        window.render_to_image().unwrap().save(&succeeded).unwrap();
                        window.remove_window();
                    })?;
                    ensure!(
                        failed.is_file() && succeeded.is_file(),
                        "Repair screenshots missing"
                    );
                    Ok(json!({
                        "requested_version":env!("CARGO_PKG_VERSION"),
                        "metadata_requests":counters.metadata.load(Ordering::SeqCst),
                        "checksum_requests":counters.checksum.load(Ordering::SeqCst),
                        "package_requests":counters.package.load(Ordering::SeqCst),
                        "total_requests":counters.total.load(Ordering::SeqCst),
                        "automatic_network_downloads":automatic_network_downloads,
                        "repair_clicks":repair_clicks,
                        "failed_screenshot":failed, "success_screenshot":succeeded,
                        "verified_receipt":install.join(RECEIPT_PATH),
                        "preserved_sentinels":protected.len(), "available_new_version":false
                    }))
                }
                .await;
                let _ = handle.update(cx, |_, window, _| window.remove_window());
                *result_for_app.borrow_mut() = Some(outcome);
                cx.update(|cx| cx.quit());
            })
            .detach();
            cx.activate(true);
        });
    drop(server);
    result
        .borrow_mut()
        .take()
        .context("Repair window closed without a result")?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn maintenance_repair_loopback_large_response_survives_backpressure() {
        let payload = (0..4 * 1024 * 1024)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        let expected_hash = format!("{:x}", Sha256::digest(&payload));
        let scratch = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.scratch");
        fs::create_dir_all(&scratch).unwrap();
        let package = scratch.join(format!("repair-loopback-{}.bin", uuid::Uuid::new_v4()));
        create_new(&package, &payload).unwrap();
        let outcome = (|| -> Result<()> {
            let mut server = Loopback::start(&package, &expected_hash)?;
            let mut response = Vec::new();
            let exchange = (|| -> Result<()> {
                let mut client = TcpStream::connect((
                    server.url.host_str().unwrap(),
                    server.url.port().unwrap(),
                ))?;
                client.set_read_timeout(Some(Duration::from_secs(10)))?;
                client.set_write_timeout(Some(Duration::from_secs(5)))?;
                client.write_all(b"GET /package HTTP/1.1\r\n\r\n")?;
                // Let the real server fill the receive window before draining
                // it, so an inherited nonblocking stream cannot hide behind a
                // fast reader or a response that fits in the socket buffer.
                std::thread::sleep(Duration::from_millis(250));
                client.read_to_end(&mut response)?;
                Ok(())
            })();
            server.stop.store(true, Ordering::Relaxed);
            let transport = server
                .thread
                .take()
                .unwrap()
                .join()
                .map_err(|_| anyhow::anyhow!("Loopback transport panicked"))?;
            eprintln!(
                "large loopback response: received_bytes={}, counts={:?}, transport={transport:?}",
                response.len(),
                server.counts.snapshot()
            );
            transport.context("Large loopback response failed")?;
            exchange?;
            ensure!(
                server.counts.snapshot() == [1, 0, 0, 1],
                "Request count changed"
            );
            let separator = response
                .windows(4)
                .position(|bytes| bytes == b"\r\n\r\n")
                .context("HTTP response header missing")?;
            let header = std::str::from_utf8(&response[..separator])?;
            ensure!(
                header.starts_with("HTTP/1.1 200 OK\r\n"),
                "Wrong HTTP status"
            );
            ensure!(
                header
                    .lines()
                    .any(|line| line == format!("Content-Length: {}", payload.len())),
                "Incorrect declared response length"
            );
            let received = &response[separator + 4..];
            ensure!(
                received == payload,
                "Large response was truncated or changed"
            );
            let received_hash = format!("{:x}", Sha256::digest(received));
            ensure!(
                received_hash == expected_hash,
                "Large response digest differs"
            );
            eprintln!(
                "complete loopback body: bytes={}, sha256={received_hash}",
                received.len()
            );
            Ok(())
        })();
        // One specifically created ordinary fixture file; never a directory
        // sweep or application/runtime cleanup.
        fs::remove_file(&package).unwrap();
        outcome.unwrap();
    }

    #[::core::prelude::v1::test]
    fn maintenance_repair_fixture_path_accepts_local_drive_and_canonical_verbatim_disk() {
        let expected = Path::new(r"D:\a\_temp\修复");
        for path in [expected, Path::new(r"\\?\D:\a\_temp\修复")] {
            let normalized = local_fixture_path(path).unwrap();
            assert_eq!(normalized, expected);
            studyapp_desktop::maintenance::absolute_components(&normalized).unwrap();
        }
    }

    #[::core::prelude::v1::test]
    fn maintenance_repair_fixture_path_rejects_unc_devices_relative_paths_and_aliases() {
        for text in [
            r"\\server\share\repair",
            r"\\?\UNC\server\share\repair",
            r"\\.\C:\repair",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1\repair",
            r"\\?\Volume{12345678-1234-1234-1234-123456789012}\repair",
            r"\Device\HarddiskVolume1\repair",
            r"C:repair",
            r"relative\repair",
            r"C:\a\..\repair",
            r"\\?\C:\a\..\repair",
            r"\\?\C:\a\.\repair",
            r"\\?\1:\repair",
        ] {
            assert!(
                local_fixture_path(Path::new(text)).is_err(),
                "accepted unsupported fixture path: {text}"
            );
        }
    }
}
