use super::*;
const ID: &str = "123e4567-e89b-42d3-a456-426614174000";
fn folder() -> std::path::PathBuf {
    tempfile::tempdir().unwrap().keep()
}
#[test]
#[cfg(windows)]
fn removal_local_conditional_and_retained_status() {
    let root = folder();
    let p = LocalFolderProvider::new(&root).unwrap();
    p.write(ID, "hello", None).unwrap();
    let plan = p.inspect_note(ID).unwrap();
    assert_eq!(plan.hash, content_hash("hello"));
    assert_eq!(plan.bytes, 5);
    let outcome = p.delete_note(&plan).unwrap();
    assert_eq!(outcome.residual, RemovalResidual::RetainedCopies);
    assert!(p.read(ID).unwrap().is_none());
    assert!(matches!(p.delete_note(&plan), Err(SyncError::Conflict)));
    assert!(
        std::fs::read_dir(root.join("StudyApp/notes/.retained"))
            .unwrap()
            .next()
            .is_none()
    );
}
#[test]
#[cfg(windows)]
fn removal_local_stale_foreign_lock_and_unrelated() {
    let root = folder();
    let p = LocalFolderProvider::new(&root).unwrap();
    p.write(ID, "hello", None).unwrap();
    let plan = p.inspect_note(ID).unwrap();
    let other = LocalFolderProvider::new(folder()).unwrap();
    other.write(ID, "hello", None).unwrap();
    assert!(matches!(other.delete_note(&plan), Err(SyncError::Conflict)));
    let target = root.join(format!("StudyApp/notes/{ID}.md"));
    std::fs::write(&target, "changed").unwrap();
    assert!(matches!(p.delete_note(&plan), Err(SyncError::Conflict)));
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join("StudyApp/.sync.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    assert!(matches!(p.delete_note(&plan), Err(SyncError::LockBusy)));
    drop(lock);
    assert_eq!(std::fs::read_to_string(target).unwrap(), "changed");
    assert!(p.inspect_note("../bad").is_err());
}
use std::{
    io::{Read, Write},
    net::TcpListener,
};
pub(crate) fn removal_server(
    replies: Vec<(u16, &'static str, &'static str)>,
) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let thread = std::thread::spawn(move || {
        let mut calls = Vec::new();
        for (status, headers, body) in replies {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(v) => break v,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if std::time::Instant::now() > deadline {
                            return calls;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            loop {
                let mut byte = [0];
                if stream.read(&mut byte).unwrap() == 0 {
                    break;
                }
                request.push(byte[0]);
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8(request).unwrap();
            let length = request
                .lines()
                .find_map(|line| {
                    line.to_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|n| n.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            let mut body_bytes = vec![0; length];
            stream.read_exact(&mut body_bytes).unwrap();
            calls.push(request);
            write!(stream,"HTTP/1.1 {status} Result\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",body.len()).unwrap();
        }
        calls
    });
    (format!("http://{addr}/"), thread)
}
fn dav_fixture(
    replies: Vec<(u16, &'static str, &'static str)>,
    target: &str,
) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let mut expanded = Vec::new();
    for (i, reply) in replies.into_iter().enumerate() {
        expanded.push(reply);
        if i < 2 && reply.0 == 200 && reply.1.starts_with("ETag: \"") {
            let etag = reply.1.trim().strip_prefix("ETag: ").unwrap();
            let xml = format!(
                "<d:multistatus xmlns:d=\"DAV:\"><d:response><d:href>{target}</d:href><d:propstat><d:prop><d:resourcetype/><d:getetag>{etag}</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"
            );
            expanded.push((207, "", Box::leak(xml.into_boxed_str())));
        }
    }
    removal_server(expanded)
}
fn webdav_server(
    replies: Vec<(u16, &'static str, &'static str)>,
) -> (String, std::thread::JoinHandle<Vec<String>>) {
    dav_fixture(replies, &format!("/StudyApp/notes/{ID}.md"))
}
#[test]
fn removal_webdav_condition_and_verified_absence() {
    let (url, server) = webdav_server(vec![
        (200, "ETag: \"v1\"\r\n", "note"),
        (200, "ETag: \"v1\"\r\n", "note"),
        (204, "", ""),
        (404, "", ""),
    ]);
    let p = WebDavProvider::for_loopback_test(&url, "user", "secret").unwrap();
    let plan = p.inspect_note(ID).unwrap();
    assert_eq!(plan.hash, content_hash("note"));
    assert_eq!(
        p.delete_note(&plan).unwrap().residual,
        RemovalResidual::ProviderBackupsUnknown
    );
    let calls = server.join().unwrap();
    assert!(calls[4].starts_with("DELETE /StudyApp/notes/"));
    assert!(calls[4].to_lowercase().contains("if-match: \"v1\""));
    assert!(!format!("{plan:?}").contains("secret"));
}
#[test]
fn removal_webdav_missing_weak_and_failed_reads_never_delete() {
    for (status, etag) in [
        (200, ""),
        (200, "ETag: W/\"weak\"\r\n"),
        (404, ""),
        (302, "Location: https://example.com\r\n"),
        (401, ""),
    ] {
        let (url, server) = webdav_server(vec![(status, etag, "")]);
        let p = WebDavProvider::for_loopback_test(&url, "u", "p").unwrap();
        assert!(p.inspect_note(ID).is_err());
        assert_eq!(server.join().unwrap().len(), 1);
    }
}
#[test]
fn removal_webdav_conflict_refusal_and_false_success() {
    for status in [412, 403, 302, 404, 500] {
        let (url, server) = webdav_server(vec![
            (200, "ETag: \"v\"\r\n", "x"),
            (200, "ETag: \"v\"\r\n", "x"),
            (status, "", ""),
        ]);
        let p = WebDavProvider::for_loopback_test(&url, "u", "p").unwrap();
        let plan = p.inspect_note(ID).unwrap();
        assert!(p.delete_note(&plan).is_err());
        server.join().unwrap();
    }
    let (url, server) = webdav_server(vec![
        (200, "ETag: \"v\"\r\n", "x"),
        (200, "ETag: \"v\"\r\n", "x"),
        (204, "", ""),
        (200, "ETag: \"v\"\r\n", "x"),
    ]);
    let p = WebDavProvider::for_loopback_test(&url, "u", "p").unwrap();
    let plan = p.inspect_note(ID).unwrap();
    assert_eq!(
        p.delete_note(&plan).unwrap().state,
        RemovalState::AcknowledgedUnverified
    );
    server.join().unwrap();
}
#[test]
#[cfg(windows)]
fn removal_local_same_bytes_new_identity_and_shared_alias_are_preserved() {
    let root = folder();
    let p = LocalFolderProvider::new(&root).unwrap();
    p.write(ID, "same", None).unwrap();
    let plan = p.inspect_note(ID).unwrap();
    let path = root.join(format!("StudyApp/notes/{ID}.md"));
    let unrelated = root.join("unrelated.md");
    std::fs::rename(&path, &unrelated).unwrap();
    std::fs::write(&path, "same").unwrap();
    assert!(matches!(p.delete_note(&plan), Err(SyncError::Conflict)));
    let alias = root.join("shared.md");
    std::fs::hard_link(&path, &alias).unwrap();
    let plan = p.inspect_note(ID).unwrap();
    assert_eq!(
        p.delete_note(&plan).unwrap().residual,
        RemovalResidual::RetainedCopies
    );
    assert_eq!(std::fs::read_to_string(alias).unwrap(), "same");
    assert_eq!(std::fs::read_to_string(unrelated).unwrap(), "same");
}
#[test]
#[cfg(windows)]
fn removal_local_asset_is_exact_and_missing_parent_never_selects_foreign_file() {
    let root = folder();
    let p = LocalFolderProvider::new(&root).unwrap();
    let key = format!("assets/{ID}/image.png");
    p.write_asset(&key, b"image").unwrap();
    let plan = Provider::inspect_asset(&p, &key).unwrap();
    Provider::delete_asset(&p, &plan).unwrap();
    assert!(p.read_asset(&key).unwrap().is_none());
    let foreign = root.join("StudyApp/assets/image.png");
    std::fs::write(&foreign, b"foreign").unwrap();
    let missing = "assets/00000000-0000-4000-8000-000000000001/image.png";
    assert!(Provider::inspect_asset(&p, missing).is_err());
    assert_eq!(std::fs::read(foreign).unwrap(), b"foreign");
}
#[test]
fn removal_webdav_assets_stale_bytes_never_delete() {
    let key = format!("assets/{ID}/image.png");
    let (url, server) = dav_fixture(
        vec![
            (200, "ETag: \"v\"\r\n", "x"),
            (200, "ETag: \"v\"\r\n", "changed"),
        ],
        &format!("/StudyApp/{key}"),
    );
    let p = WebDavProvider::for_loopback_test(&url, "u", "p").unwrap();
    let plan = Provider::inspect_asset(&p, &key).unwrap();
    assert!(matches!(
        Provider::delete_asset(&p, &plan),
        Err(SyncError::Conflict)
    ));
    let calls = server.join().unwrap();
    assert!(calls[0].contains(&key));
}

#[test]
fn removal_webdav_collection_ambiguous_and_unmatched_properties_never_delete() {
    for (href, kind, etag, status) in [
        (
            format!("/StudyApp/notes/{ID}.md"),
            "<d:collection/>",
            "\"v\"",
            "200",
        ),
        ("/other.md".into(), "", "\"v\"", "200"),
        (format!("/StudyApp/notes/{ID}.md"), "", "\"stale\"", "200"),
        (format!("/StudyApp/notes/{ID}.md"), "", "\"v\"", "404"),
        (
            format!("/StudyApp/notes/{ID}.md"),
            "<d:unknown/>",
            "\"v\"",
            "200",
        ),
    ] {
        let xml = format!(
            "<d:multistatus xmlns:d=\"DAV:\"><d:response><d:href>{href}</d:href><d:propstat><d:prop><d:resourcetype>{kind}</d:resourcetype><d:getetag>{etag}</d:getetag></d:prop><d:status>HTTP/1.1 {status} Status</d:status></d:propstat></d:response></d:multistatus>"
        );
        let (url, server) = removal_server(vec![
            (200, "ETag: \"v\"\r\n", "index"),
            (207, "", Box::leak(xml.into_boxed_str())),
        ]);
        let p = WebDavProvider::for_loopback_test(&url, "u", "p").unwrap();
        assert!(p.inspect_note(ID).is_err());
        let calls = server.join().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls[1].starts_with("PROPFIND "));
        assert!(calls[1].to_lowercase().contains("depth: 0"));
        assert!(!calls.iter().any(|r| r.starts_with("DELETE ")));
    }
}
#[test]
fn removal_webdav_acknowledged_but_unverified_is_not_refusal() {
    let (url, server) = webdav_server(vec![
        (200, "ETag: \"v\"\r\n", "x"),
        (200, "ETag: \"v\"\r\n", "x"),
        (204, "", ""),
        (403, "", ""),
    ]);
    let p = WebDavProvider::for_loopback_test(&url, "u", "p").unwrap();
    let plan = p.inspect_note(ID).unwrap();
    assert_eq!(
        p.delete_note(&plan).unwrap().state,
        RemovalState::AcknowledgedUnverified
    );
    server.join().unwrap();
}
#[test]
#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
fn removal_local_selected_retained_and_partial_completion() {
    let root = folder();
    let p = LocalFolderProvider::new(&root).unwrap();
    p.write(ID, "note", None).unwrap();
    let mut plan = p.inspect_note(ID).unwrap();
    assert_eq!(plan.confirmed_retained.len(), 1);
    let retained = root
        .join("StudyApp/notes/.retained")
        .join(&plan.confirmed_retained[0].name);
    plan.confirmed_retained.clear();
    assert_eq!(p.delete_note(&plan).unwrap().removed_retained, 0);
    assert_eq!(std::fs::read_to_string(retained).unwrap(), "note");
    let root = folder();
    let p = LocalFolderProvider::new(&root).unwrap();
    p.write(ID, "note", None).unwrap();
    let target = root.join(format!("StudyApp/notes/{ID}.md"));
    let second = root.join("StudyApp/notes/.retained/ffffffff-ffff-4fff-8fff-ffffffffffff.pending");
    std::fs::hard_link(&target, &second).unwrap();
    let plan = p.inspect_note(ID).unwrap();
    assert_eq!(plan.confirmed_retained.len(), 2);
    let changed = root
        .join("StudyApp/notes/.retained")
        .join(&plan.confirmed_retained[1].name);
    local::BEFORE_REMOVE.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |_| {
            std::fs::rename(&changed, changed.with_extension("foreign")).unwrap();
            std::fs::write(changed, "replacement").unwrap();
        }))
    });
    let outcome = p.delete_note(&plan).unwrap();
    assert_eq!(outcome.state, RemovalState::Partial);
    assert_eq!(outcome.removed_retained, 1);
    assert_eq!(outcome.remaining_confirmed.len(), 1);
    assert!(target.exists());
}
#[test]
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn removal_local_unix_is_unsupported_before_any_mutation() {
    let root = folder();
    let p = LocalFolderProvider::new(&root).unwrap();
    p.write(ID, "note", None).unwrap();
    assert!(matches!(
        p.inspect_note(ID),
        Err(SyncError::UnsupportedRemoval(_))
    ));
    assert_eq!(p.read(ID).unwrap().unwrap().content, "note");
}
