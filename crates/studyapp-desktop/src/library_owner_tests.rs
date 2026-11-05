use super::*;
#[test]
fn hints_and_protocol_reject_pid_path_and_unknown_authorities() {
    assert!(
        serde_json::from_str::<Hint>(
            r#"{"schema":1,"session":"00000000000000000000000000000000","pid":1}"#
        )
        .is_err()
    );
    assert!(!valid_id("../other"));
    assert!(!valid_id("f"));
    assert!(read_packet::<Hint>(&mut std::io::Cursor::new(vec![b'x'; LIMIT + 1])).is_err());
    assert!(read_packet::<Hint>(&mut std::io::Cursor::new(b"{}\n{}\n")).is_err());
}
#[test]
fn close_tickets_are_idempotent_and_cancelled_requests_can_retry() {
    let control = Control {
        database: PathBuf::from("library"),
        state: Mutex::new((Phase::Idle, None)),
    };
    let mut request = Request {
        schema: 1,
        session: "f".repeat(32),
        lock: "held".into(),
        op: Op::Close,
        ticket: Some("a".repeat(32)),
    };
    assert!(control.apply(&request));
    assert!(!control.apply(&request));
    control.mark(Phase::Cancelled);
    assert_eq!(control.phase(), Phase::Cancelled);
    assert!(!control.apply(&request));
    request.ticket = Some("b".repeat(32));
    assert!(control.apply(&request));
    control.mark(Phase::Closing);
    request.ticket = Some("c".repeat(32));
    assert!(!control.apply(&request));
}

fn lock_library(database: &Path) -> File {
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(database.with_extension("lock"))
        .unwrap();
    file.try_lock().unwrap();
    file
}
fn hint(database: &Path) -> Hint {
    serde_json::from_slice(&std::fs::read(hint_path(database)).unwrap()).unwrap()
}
fn local_offer(database: &Path, file: &File) -> Offer {
    Offer {
        pid: std::process::id(),
        session: hint(database).session,
        lock: identity(file).unwrap(),
        database: database.to_owned(),
    }
}
#[test]
fn server_rejects_wrong_lock_session_and_ticket_without_notifying_ui() {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("studyapp.sqlite3");
    let file = lock_library(&database);
    let (tx, rx) = std::sync::mpsc::channel();
    let server = Server::start(&database, &file, move |c| {
        tx.send(c).unwrap();
    })
    .unwrap();
    let offer = local_offer(&database, &file);
    let ticket = "a".repeat(32);
    let mut other = offer.clone();
    other.lock = "different lock".into();
    assert!(exchange(&other, Op::Close, Some(&ticket)).is_err());
    assert!(exchange(&offer, Op::Close, Some("../not-a-ticket")).is_err());
    let mut connection = native::Connection::connect(&offer.session).unwrap();
    packet(
        &mut connection,
        &Request {
            schema: 1,
            session: "b".repeat(32),
            lock: offer.lock.clone(),
            op: Op::Close,
            ticket: Some(ticket.clone()),
        },
    )
    .unwrap();
    assert!(read_packet::<Reply>(&mut connection).is_err());
    drop(connection);
    assert!(rx.try_recv().is_err());
    assert!(matches!(
        offer.request_close(&ticket).unwrap(),
        Progress::Phase(_)
    ));
    let control = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(control.database, database);
    control.mark(Phase::Cancelled);
    assert_eq!(
        exchange(&offer, Op::Close, Some(&ticket)).unwrap().phase,
        Phase::Cancelled
    );
    assert!(
        rx.try_recv().is_err(),
        "a repeated cancelled ticket must not request close again"
    );
    let second = "c".repeat(32);
    assert!(offer.request_close(&second).is_ok());
    assert!(rx.recv_timeout(Duration::from_secs(2)).is_ok());
    assert!(is_held(&locked_file(&database).unwrap()).unwrap());
    drop(server);
    assert!(matches!(
        offer.progress(&second).unwrap(),
        Progress::WaitingForRelease
    ));
    drop(file);
    assert!(matches!(
        offer.progress(&second).unwrap(),
        Progress::Released
    ));
    assert!(database.with_extension("lock").is_file());
}
#[test]
fn discovery_never_closes_self_or_follows_a_hint_to_another_library() {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("first.sqlite3");
    let other = root.path().join("second.sqlite3");
    let file = lock_library(&database);
    let second = lock_library(&other);
    let (tx, rx) = std::sync::mpsc::channel();
    let server = Server::start(&database, &file, move |c| {
        tx.send(c).unwrap();
    })
    .unwrap();
    assert!(inspect(&database, &locked_file(&database).unwrap()).is_err());
    std::fs::copy(hint_path(&database), hint_path(&other)).unwrap();
    assert!(inspect(&other, &locked_file(&other).unwrap()).is_err());
    let mut stale = local_offer(&database, &file);
    stale.pid = stale.pid.wrapping_add(1);
    assert!(stale.request_close(&"a".repeat(32)).is_err());
    assert!(rx.try_recv().is_err());
    drop(server);
    assert!(inspect(&database, &locked_file(&database).unwrap()).is_err());
    drop((file, second));
}
#[test]
fn unresponsive_connections_cannot_hold_server_shutdown_indefinitely() {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("studyapp.sqlite3");
    let file = lock_library(&database);
    let server = Server::start(&database, &file, |_| panic!("no valid request sent")).unwrap();
    let connection = native::Connection::connect(&hint(&database).session).unwrap();
    thread::sleep(Duration::from_millis(40));
    let started = Instant::now();
    drop(server);
    assert!(started.elapsed() < Duration::from_secs(4));
    drop(connection);
    assert!(is_held(&locked_file(&database).unwrap()).unwrap());
}

// This exact test executable is the child; no native window or app entrypoint.
#[test]
fn owner_process_probe() {
    let Some(root) = std::env::var_os("WW_STUDYAPP_OWNER_PROBE") else {
        return;
    };
    let root = PathBuf::from(root);
    let database = root.join("studyapp.sqlite3");
    let worker = crate::worker::Worker::start(database.clone(), None);
    let initial = worker
        .receiver
        .recv_timeout(Duration::from_secs(60))
        .unwrap();
    assert!(initial.snapshot.is_some(), "{:?}", initial.startup_failure);
    // This reply orders readiness after owner service attachment.
    worker
        .sender
        .send(crate::worker::Work::Apply(
            studyapp_core::Command::SetShowFilePaths(true),
        ))
        .unwrap();
    loop {
        if worker
            .receiver
            .recv_timeout(Duration::from_secs(60))
            .unwrap()
            .preferences
            .is_some()
        {
            break;
        }
    }
    std::fs::write(root.join("ready"), b"ready").unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    thread::spawn(move || {
        let _ = std::io::stdin().read(&mut [0u8; 1]);
        stopped.store(true, Ordering::Release);
    });
    let started = Instant::now();
    let mut first = true;
    while !stop.load(Ordering::Acquire) && started.elapsed() < Duration::from_secs(60) {
        if let Ok(update) = worker.receiver.recv_timeout(Duration::from_millis(50))
            && let Some(control) = update.owner_close
        {
            if first {
                control.mark(Phase::Cancelled);
                first = false;
            } else {
                control.mark(Phase::Waiting);
            }
        }
    }
    drop(worker);
}
struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        drop(self.0.stdin.take());
        let _ = self.0.wait();
    }
}
#[test]
fn real_owner_process_cancels_then_releases_lock_and_worker_reopens_original_data() {
    use crate::worker::{Work, Worker};
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("studyapp.sqlite3");
    let seed = Worker::start(database.clone(), None);
    assert!(
        seed.receiver
            .recv_timeout(Duration::from_secs(60))
            .unwrap()
            .snapshot
            .is_some()
    );
    seed.sender
        .send(Work::Apply(studyapp_core::Command::CreateNote {
            title: "Retained owner data".into(),
            content: "Unchanged across normal close".into(),
        }))
        .unwrap();
    let saved = seed
        .receiver
        .recv_timeout(Duration::from_secs(60))
        .unwrap()
        .snapshot
        .unwrap();
    let note_id = saved.notes[0].id.clone();
    drop(seed);
    let lock_content = b"retained lock file; never delete or truncate";
    std::fs::write(database.with_extension("lock"), lock_content).unwrap();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "library_owner::tests::owner_process_probe",
            "--nocapture",
        ])
        .env("WW_STUDYAPP_OWNER_PROBE", root.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut child = Child(child);
    let started = Instant::now();
    while !root.path().join("ready").is_file() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "owner exited before readiness"
        );
        assert!(started.elapsed() < Duration::from_secs(60));
        thread::sleep(Duration::from_millis(10));
    }
    let contender = Worker::start(database.clone(), None);
    let failed = contender
        .receiver
        .recv_timeout(Duration::from_secs(60))
        .unwrap();
    assert!(failed.snapshot.is_none());
    let failure = failed.startup_failure.unwrap();
    assert_eq!(
        failure.kind,
        crate::worker::startup::StartupFailureKind::InUse
    );
    let offer = failure
        .owner
        .expect("same-executable child must have a verified offer");
    assert_eq!(offer.pid, child.0.id());
    let ticket = "1".repeat(32);
    assert!(offer.request_close(&ticket).is_ok());
    let started = Instant::now();
    loop {
        if matches!(
            offer.progress(&ticket).unwrap(),
            Progress::Phase(Phase::Cancelled)
        ) {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(5));
        thread::sleep(Duration::from_millis(10));
    }
    assert!(is_held(&locked_file(&database).unwrap()).unwrap());
    let ticket = "2".repeat(32);
    assert!(offer.request_close(&ticket).is_ok());
    let started = Instant::now();
    loop {
        if matches!(
            offer.progress(&ticket).unwrap(),
            Progress::Phase(Phase::Waiting)
        ) {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(5));
        thread::sleep(Duration::from_millis(10));
    }
    assert!(child.0.try_wait().unwrap().is_none());
    contender.sender.send(Work::RetryOpenLibrary).unwrap();
    assert!(
        contender
            .receiver
            .recv_timeout(Duration::from_secs(60))
            .unwrap()
            .snapshot
            .is_none()
    );
    drop(child.0.stdin.take());
    assert!(child.0.wait().unwrap().success());
    assert!(matches!(
        offer.progress(&ticket).unwrap(),
        Progress::Released
    ));
    contender.sender.send(Work::RetryOpenLibrary).unwrap();
    let reopened = contender
        .receiver
        .recv_timeout(Duration::from_secs(60))
        .unwrap();
    assert!(reopened.startup_failure.is_none());
    let snapshot = reopened.snapshot.unwrap();
    assert_eq!(snapshot.notes[0].id, note_id);
    assert_eq!(
        snapshot.versions[0].content,
        "Unchanged across normal close"
    );
    drop(contender);
    assert_eq!(
        std::fs::read(database.with_extension("lock")).unwrap(),
        lock_content
    );
}

#[cfg(feature = "ui-test")]
#[path = "library_owner_ui_tests.rs"]
mod ui;
