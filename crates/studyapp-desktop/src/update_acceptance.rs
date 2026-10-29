//! Explicitly non-production real package transition acceptance driver.
use crate::app::{Page, StudyApp};
use gpui_kit::{test::TestWindowExt, *};
use serde_json::{Value, json};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
static HOLD_AUTOSAVE: AtomicBool = AtomicBool::new(false);
pub fn hold_autosave() -> bool {
    HOLD_AUTOSAVE.load(Ordering::Relaxed)
}
static CHECKS_READY: AtomicBool = AtomicBool::new(false);
static LAUNCH_DIRECTORY: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
#[cfg(target_os = "macos")]
pub(crate) fn accept_launch_directory(path: PathBuf) -> Result<(), String> {
    if !path.is_absolute() || !path.join("fixture.json").is_file() {
        return Err("Invalid acceptance launch context".into());
    }
    LAUNCH_DIRECTORY
        .set(path)
        .map_err(|_| "Acceptance context already set".into())
}
pub(crate) fn launch_directory() -> Option<PathBuf> {
    resolve_directory(
        LAUNCH_DIRECTORY.get().cloned(),
        std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true"),
        std::env::var_os("STUDYAPP_UPDATE_ACCEPTANCE_DIR").map(PathBuf::from),
    )
}
fn resolve_directory(
    handoff: Option<PathBuf>,
    runner: bool,
    environment: Option<PathBuf>,
) -> Option<PathBuf> {
    handoff.or_else(|| runner.then_some(environment).flatten())
}
pub fn active() -> bool {
    launch_directory().is_some()
}
pub fn checks_allowed() -> bool {
    CHECKS_READY.load(Ordering::Relaxed)
}
fn directory() -> Result<PathBuf, String> {
    if !active() {
        return Err("Acceptance fixture is not active".into());
    }
    launch_directory().ok_or("Missing fixture directory".into())
}
fn transaction_directory() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    return crate::update_install::macos::acceptance_health_directory();
    #[cfg(not(target_os = "macos"))]
    std::env::var_os("STUDYAPP_UPDATE_TRANSACTION").map(PathBuf::from)
}
fn configuration() -> Result<Value, String> {
    serde_json::from_slice(
        &std::fs::read(directory()?.join("fixture.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}
pub fn fixture_bytes(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let config = configuration()?;
    let filename = if url == crate::update_backend::LATEST_MANIFEST_URL {
        "latest.json"
    } else if config["package_url"].as_str() == Some(url) {
        config["package_name"]
            .as_str()
            .ok_or("Missing fixture package")?
    } else {
        return Err("Unmapped fixture URL".into());
    };
    if Path::new(filename).file_name().and_then(|v| v.to_str()) != Some(filename) {
        return Err("Invalid fixture filename".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(directory()?.join(filename))
        .map_err(|e| e.to_string())?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("Fixture exceeds transport limit".into());
    }
    Ok(bytes)
}
fn save(name: &str, value: Value) {
    let dir = directory().unwrap();
    let temporary = dir.join(format!("{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    std::fs::rename(temporary, dir.join(name)).unwrap();
}
async fn wait(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    cx: &mut AsyncApp,
    description: &str,
    condition: impl Fn(&StudyApp, &App) -> bool,
) {
    let started = Instant::now();
    loop {
        if window
            .update(cx, |_, _, cx| condition(view.read(cx), cx))
            .unwrap()
        {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "Acceptance timeout: {description}"
        );
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
    }
}
async fn presented(window: AnyWindowHandle, cx: &mut AsyncApp) {
    // Submit outside the capture/click transaction, then allow compositor presentation.
    for _ in 0..2 {
        window
            .update(cx, |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.background_executor()
            .timer(Duration::from_millis(200))
            .await;
    }
}
async fn click_visible(window: AnyWindowHandle, id: SharedString, cx: &mut AsyncApp) {
    let started = Instant::now();
    loop {
        presented(window, cx).await;
        if window
            .update(cx, |_, window, _| window.try_find(id.clone()).is_some())
            .unwrap()
        {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "Visible control missing: {id}"
        );
    }
    #[cfg(not(target_os = "linux"))]
    window
        .update(cx, |_, window, cx| window.click(id, cx))
        .unwrap();
    #[cfg(target_os = "linux")]
    {
        use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        assert!(
            active(),
            "native input is restricted to the remote acceptance fixture"
        );
        let (parent, x, y) = window
            .update(cx, |_, window, _| {
                let parent = match HasWindowHandle::window_handle(window).unwrap().as_raw() {
                    RawWindowHandle::Xlib(raw) => raw.window,
                    RawWindowHandle::Xcb(raw) => u64::from(raw.window.get()),
                    _ => panic!("X11 acceptance parent required"),
                };
                let center = window.find(id).bounds().center();
                let scale = f64::from(window.scale_factor());
                (
                    parent,
                    (f64::from(center.x) * scale).round() as i32,
                    (f64::from(center.y) * scale).round() as i32,
                )
            })
            .unwrap();
        let result = std::process::Command::new("xdotool")
            .args([
                "mousemove",
                "--window",
                &parent.to_string(),
                &x.to_string(),
                &y.to_string(),
                "click",
                "1",
            ])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "native card input failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        cx.background_executor()
            .timer(Duration::from_millis(200))
            .await;
    }
}

#[cfg(target_os = "linux")]
async fn native_visibility(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    visible: bool,
    name: &str,
    cx: &mut AsyncApp,
) {
    let started = Instant::now();
    loop {
        presented(window, cx).await;
        let state = window
            .update(cx, |_, _, cx| {
                view.read(cx).note_content.read(cx).native_state().unwrap()
            })
            .unwrap();
        if state["viewable"] == visible
            && state["mapped"] == visible
            && state["appliedVisible"] == visible
        {
            save(name, state);
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "native visibility did not follow modal: {state}"
        );
    }
}
fn committed(app: &StudyApp) -> Value {
    json!(
        app.snapshot
            .as_ref()
            .unwrap()
            .versions
            .iter()
            .map(|v| (&v.id, &v.content))
            .collect::<Vec<_>>()
    )
}
pub fn exercise(window: AnyWindowHandle, view: Entity<StudyApp>, database: PathBuf, cx: &mut App) {
    cx.spawn(async move|cx|{
        cx.background_executor().timer(Duration::from_millis(500)).await;
        wait(window,&view,cx,"library and drafts",|a,_|a.snapshot.is_some()&&!a.startup_busy&&!a.draft_loading).await;
        let config=configuration().unwrap();let marker=config["draft_marker"].as_str().unwrap();
        if config["target_version"].as_str()==Some(env!("CARGO_PKG_VERSION")){
            let previous:Value=serde_json::from_slice(&std::fs::read(directory().unwrap().join("old-observation.json")).unwrap()).unwrap();
            let id=previous["draft_id"].as_str().unwrap();
            window.update(cx,|_,_,cx|{
                assert_eq!(committed(view.read(cx)),previous["committed"]);
                view.update(cx,|a,cx|{a.page=Page::Notes;a.clear_notices(cx);cx.notify();});
            }).unwrap();
            click_visible(window, "folders-drafts".into(), cx).await;
            click_visible(window, SharedString::from(format!("draft-row-{id}")), cx).await;
            wait(window,&view,cx,"restored editor",|a,cx|a.note_editor.is_some()&&a.note_content.read(cx).ready()).await;
            let editor=window.update(cx,|_,_,cx|view.read(cx).note_content.clone()).unwrap();
            let live=crate::smoke_editor::bridge(window,&editor,json!({"type":"snapshot"}),cx).await;
            assert_eq!(live["text"],marker);
            presented(window, cx).await;
            window.update(cx,|_,window,_|crate::smoke::capture(window,directory().unwrap().join("restored.png")).unwrap()).unwrap();
            save("new-observation.json",json!({"version":env!("CARGO_PKG_VERSION"),"pid":std::process::id(),"executable":std::env::current_exe().unwrap(),"draft_id":id,"live_text":live["text"],"database":database,"source_sha":config["source_sha"],"transaction":transaction_directory()}));
            return;
        }
        if directory().unwrap().join("old-observation.json").exists(){
            let notice=window.update(cx,|_,_,cx|format!("{:?}",view.read(cx).notice)).unwrap();
            save("restart-failure.json",json!({"pid":std::process::id(),"version":env!("CARGO_PKG_VERSION"),"notice":notice}));return;
        }
        HOLD_AUTOSAVE.store(true,Ordering::Relaxed);
        let baseline=window.update(cx,|_,_,cx|committed(view.read(cx))).unwrap();
        window.update(cx,|_,window,cx|{view.update(cx,|a,cx|{a.page=Page::Notes;a.clear_notices(cx);cx.notify();});window.render_frame(cx);crate::smoke_library::library_add(window,false,&view,cx);}).unwrap();
        wait(window,&view,cx,"editable native draft",|a,cx|a.note_editor.is_some()&&a.note_content.read(cx).ready()).await;
        let editor=window.update(cx,|_,_,cx|view.read(cx).note_content.clone()).unwrap();
        crate::smoke_editor::bridge(window,&editor,json!({"type":"replace","from":0,"to":0,"text":marker}),cx).await;
        let live=crate::smoke_editor::bridge(window,&editor,json!({"type":"snapshot"}),cx).await;
        assert_eq!(live["text"],marker);
        let before=window.update(cx,|_,_,cx|{let app=view.read(cx);json!({"version":env!("CARGO_PKG_VERSION"),"pid":std::process::id(),"executable":std::env::current_exe().unwrap(),"draft_id":app.active_draft_id,"committed":baseline,"database":database,"source_sha":config["source_sha"],"transaction":transaction_directory()})}).unwrap();
        save("old-observation.json",before);
        CHECKS_READY.store(true,Ordering::Relaxed);
        window.update(cx,|_,_,cx|{let panel=view.read(cx).update_panel.clone();panel.update(cx,|p,cx|p.acceptance_check(cx));}).unwrap();
        wait(window,&view,cx,"signed update card",|a,cx|a.update_panel.read(cx).dialog_visible()).await;
        #[cfg(target_os = "linux")]
        native_visibility(window, &view, false, "card-first-native-state.json", cx).await;
        click_visible(window, "update-dialog-later".into(), cx).await;
        wait(window,&view,cx,"native dismissal",|a,cx|!a.update_panel.read(cx).dialog_visible()).await;
        #[cfg(target_os = "linux")]
        native_visibility(window, &view, true, "editor-restored-native-state.json", cx).await;
        click_visible(window, "sidebar-update".into(), cx).await;
        wait(window,&view,cx,"reopened card",|a,cx|a.update_panel.read(cx).dialog_visible()).await;
        #[cfg(target_os = "linux")]
        native_visibility(window, &view, false, "card-native-state.json", cx).await;
        presented(window, cx).await;
        window.update(cx,|_,window,_|crate::smoke::capture(window,directory().unwrap().join("update-card.png")).unwrap()).unwrap();
        click_visible(window, "update-dialog-install".into(), cx).await;
        #[cfg(target_os = "macos")]
        {
            // Normal system approval may leave the first attempt on its retry card.
            // This signal only clicks that card; production prepare still verifies receipt.
            let started = Instant::now();
            while !directory().unwrap().join("system-approved.json").exists() {
                assert!(started.elapsed() < Duration::from_secs(180), "System approval did not complete");
                cx.background_executor().timer(Duration::from_millis(250)).await;
            }
            presented(window, cx).await;
            window.update(cx,|_,window,_|crate::smoke::capture(window,directory().unwrap().join("approved-retry-card.png")).unwrap()).unwrap();
            click_visible(window, "update-dialog-install".into(), cx).await;
        }
        // The application and its supervisor own all remaining install/restart work.
    }).detach();
}

pub fn before_close(app: &StudyApp, prepared: &crate::update_install::PreparedPackage, cx: &App) {
    if !active() {
        return;
    }
    let dir = match prepared {
        crate::update_install::PreparedPackage::Windows(p)
        | crate::update_install::PreparedPackage::Linux(p)
        | crate::update_install::PreparedPackage::Macos(p) => p,
    };
    save(
        "attempt.json",
        json!({"directory":dir,"pid":std::process::id()}),
    );
    let marker = configuration().unwrap()["draft_marker"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(app.note_content.read(cx).value().to_string(), marker);
    let id = app.active_draft_id.as_ref().expect("active pending draft");
    let record = &app.draft_catalog[id];
    let disk = crate::drafts::DraftStore::record_path(&app.last_library, id).unwrap();
    let stored = std::fs::read(&disk)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    assert!(
        stored
            .as_ref()
            .is_none_or(|value| value["draft"]["content"] != marker),
        "ordinary autosave unexpectedly persisted final marker"
    );
    assert!(app.draft_durable.get(id).copied().unwrap_or(0) < record.revision);
    save(
        "close-pending.json",
        json!({"id":id,"revision":record.revision,"marker":marker,"disk":disk,"pid":std::process::id(),"timestamp":chrono::Utc::now().timestamp_millis()}),
    );
}
pub fn close_ack(app: &StudyApp) {
    if !active() || app.pending_nsis_update.is_none() {
        return;
    }
    let pending: Value = serde_json::from_slice(
        &std::fs::read(directory().unwrap().join("close-pending.json")).unwrap(),
    )
    .unwrap();
    let disk: Value =
        serde_json::from_slice(&std::fs::read(pending["disk"].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(disk["draft"]["content"], pending["marker"]);
    assert!(disk["revision"].as_u64().unwrap() >= pending["revision"].as_u64().unwrap());
    save(
        "close-ack.json",
        json!({"id":pending["id"],"revision":disk["revision"],"pid":std::process::id(),"timestamp":chrono::Utc::now().timestamp_millis()}),
    );
}
pub fn before_helper() {
    if !active() {
        return;
    }
    let ack: Value = serde_json::from_slice(
        &std::fs::read(directory().unwrap().join("close-ack.json")).unwrap(),
    )
    .unwrap();
    save(
        "helper-handoff.json",
        json!({"ack":ack,"pid":std::process::id(),"timestamp":chrono::Utc::now().timestamp_millis()}),
    );
}
pub fn install_failure_hook() {
    if !active() {
        return;
    }
    std::panic::set_hook(Box::new(|info| {
        save(
            &format!("panic-{}.json", std::process::id()),
            json!({"pid":std::process::id(),"version":env!("CARGO_PKG_VERSION"),"error":info.to_string()}),
        );
    }));
}

pub fn startup_failure(error: &str) {
    if active() {
        save(
            &format!("startup-error-{}.json", std::process::id()),
            json!({"pid":std::process::id(),"error":error,"version":env!("CARGO_PKG_VERSION")}),
        );
    }
}

#[cfg(test)]
mod launch_context_tests {
    use super::resolve_directory;
    use std::path::PathBuf;
    #[test]
    fn bound_launch_context_survives_missing_environment() {
        let fixture = PathBuf::from("fixture");
        assert_eq!(
            resolve_directory(Some(fixture.clone()), false, None),
            Some(fixture.clone())
        );
        assert_eq!(resolve_directory(None, false, Some(fixture.clone())), None);
        assert_eq!(
            resolve_directory(None, true, Some(fixture.clone())),
            Some(fixture.clone())
        );
        assert_eq!(
            resolve_directory(Some(fixture.clone()), true, Some(PathBuf::from("stale"))),
            Some(fixture)
        );
    }
}
