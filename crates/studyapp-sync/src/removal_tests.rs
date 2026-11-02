use super::*;
const ID: &str = "123e4567-e89b-42d3-a456-426614174000";
fn folder() -> std::path::PathBuf {
    tempfile::tempdir().unwrap().keep()
}
#[test]
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
            .is_some()
    );
}
#[test]
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
            calls.push(String::from_utf8(request).unwrap());
            write!(stream,"HTTP/1.1 {status} Result\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",body.len()).unwrap();
        }
        calls
    });
    (format!("http://{addr}/"), thread)
}
#[test]
fn removal_webdav_condition_and_verified_absence() {
    let (url, server) = removal_server(vec![
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
    assert!(calls[2].starts_with("DELETE /StudyApp/notes/"));
    assert!(calls[2].to_lowercase().contains("if-match: \"v1\""));
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
        let (url, server) = removal_server(vec![(status, etag, "")]);
        let p = WebDavProvider::for_loopback_test(&url, "u", "p").unwrap();
        assert!(p.inspect_note(ID).is_err());
        assert_eq!(server.join().unwrap().len(), 1);
    }
}
#[test]
fn removal_webdav_conflict_refusal_and_false_success() {
    for status in [412, 403, 302, 404, 500] {
        let (url, server) = removal_server(vec![
            (200, "ETag: \"v\"\r\n", "x"),
            (200, "ETag: \"v\"\r\n", "x"),
            (status, "", ""),
        ]);
        let p = WebDavProvider::for_loopback_test(&url, "u", "p").unwrap();
        let plan = p.inspect_note(ID).unwrap();
        assert!(p.delete_note(&plan).is_err());
        server.join().unwrap();
    }
    let (url, server) = removal_server(vec![
        (200, "ETag: \"v\"\r\n", "x"),
        (200, "ETag: \"v\"\r\n", "x"),
        (204, "", ""),
        (200, "ETag: \"v\"\r\n", "x"),
    ]);
    let p = WebDavProvider::for_loopback_test(&url, "u", "p").unwrap();
    let plan = p.inspect_note(ID).unwrap();
    assert!(p.delete_note(&plan).is_err());
    server.join().unwrap();
}
#[test]
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
    let (url, server) = removal_server(vec![
        (200, "ETag: \"v\"\r\n", "x"),
        (200, "ETag: \"v\"\r\n", "changed"),
    ]);
    let p = WebDavProvider::for_loopback_test(&url, "u", "p").unwrap();
    let plan = Provider::inspect_asset(&p, &key).unwrap();
    assert!(matches!(
        Provider::delete_asset(&p, &plan),
        Err(SyncError::Conflict)
    ));
    let calls = server.join().unwrap();
    assert!(calls[0].contains(&key));
}
