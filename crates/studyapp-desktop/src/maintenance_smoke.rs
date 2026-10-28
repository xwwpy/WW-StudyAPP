//! Runner-only native maintenance UI regression. The executor records intent.
use super::*;
use gpui_kit::test::TestWindowExt;
use std::{cell::Cell, path::PathBuf, rc::Rc, time::Duration};

fn parse_scenario(args: &[String], github_actions: bool) -> Result<Option<(String, PathBuf)>> {
    if !args.iter().any(|a| a == "--maintenance-smoke") {
        return Ok(None);
    }
    ensure!(github_actions, "维护界面回归仅能在 GitHub runner 运行");
    ensure!(
        args.len() == 4 && args[0] == "--maintenance-smoke" && args[2] == "--data-dir",
        "维护回归参数无效"
    );
    ensure!(
        matches!(args[1].as_str(), "missing" | "corrupt" | "confirmations"),
        "未知维护回归场景"
    );
    let root = PathBuf::from(&args[3]);
    ensure!(
        absolute_components(&root)?.len() >= 3,
        "回归目录必须是独立的绝对路径"
    );
    Ok(Some((args[1].clone(), root)))
}

pub(crate) fn run_if_requested(args: &[String]) -> Result<bool> {
    let Some((scenario, root)) = parse_scenario(
        args,
        std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true"),
    )?
    else {
        return Ok(false);
    };
    ensure!(!root.try_exists()?, "维护回归必须使用尚不存在的目录");
    let _guards = crate::storage_backend::guard_directories(
        root.parent().context("回归目录父路径无效")?,
        false,
    )?;
    std::fs::create_dir(&root)?;
    let library = root.join("library");
    let database = library.join("studyapp.sqlite3");
    let before = if scenario == "missing" {
        None
    } else {
        std::fs::create_dir(&library)?;
        let bytes = b"deliberately corrupt fixture; never open as database".to_vec();
        std::fs::write(&database, &bytes)?;
        Some(bytes)
    };
    let settings = library.join("settings");
    let corrupt_settings = if scenario == "corrupt" {
        std::fs::create_dir(&settings)?;
        let bytes = b"deliberately corrupt settings manifest".to_vec();
        std::fs::write(settings.join("manifest.json"), &bytes)?;
        Some(bytes)
    } else {
        None
    };
    let context = DiscoveryContext {
        default_library: library.clone(),
        location_file: None,
        explicit_library: Some(library.clone()),
    };
    let library_discovery = maintenance_runtime::discover_library(&context);
    // Synthetic ownership is injected only into the fake executor window. It
    // does not enter installed/portable discovery or any production executor.
    let receipt = InstallReceipt {
        schema: 1,
        install_id: "ui-fixture".into(),
        owner: InstallOwner::Portable,
        install_root: root.join("application"),
        root_identity: FileIdentity {
            volume: 1,
            file_id: 1,
            size: 0,
            sha256: String::new(),
        },
        version: env!("CARGO_PKG_VERSION").into(),
        source_package_sha256: "a".repeat(64),
        files: vec![OwnedFile {
            relative_path: "WW-StudyApp.exe".into(),
            role: FileRole::Application,
            size: 4,
            sha256: "a".repeat(64),
        }],
    };
    let discovery = InstallDiscovery {
        receipt: Some(receipt),
        library: library_discovery,
        issues: vec![],
    };
    let calls = Rc::new(Cell::new(0));
    let recorded = calls.clone();
    let executor: MaintenanceExecutor = Box::new(move |token| {
        assert_eq!(
            token.preview().selection,
            DataSelection {
                configuration: false,
                database: true
            }
        );
        recorded.set(recorded.get() + 1);
        Ok("测试已记录确认意图，未删除任何文件。".into())
    });
    run_window(
        discovery,
        executor,
        Some(Box::new(move |handle, view, cx| {
            cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_millis(500)).await;
            let mut stages = vec!["selection"];
            let selection = handle.update(cx, |_, window, cx| {
                assert_eq!(cx.windows().len(), 1, "standalone maintenance window only");
                assert_eq!(view.read(cx).controller.selection, DataSelection::default());
                window.render_frame(cx);
                window.find("maintenance-config"); window.find("maintenance-database");
                window.render_to_image().unwrap().save(root.join("maintenance-selection.png")).unwrap();
                if scenario == "confirmations" {
                    window.click("maintenance-database", cx);
                    window.render_frame(cx);
                    window.click("maintenance-next", cx);
                    assert_eq!(view.read(cx).controller.stage(), ConfirmationStage::DatabaseReview);
                    assert_eq!(calls.get(),0);
                    window.render_frame(cx);
                    window.click("maintenance-back",cx);
                    assert_eq!(view.read(cx).controller.stage(), ConfirmationStage::Selection);
                    window.render_frame(cx);
                    window.click("maintenance-config",cx);
                    window.render_frame(cx);
                    window.click("maintenance-config",cx);
                    window.render_frame(cx);
                    window.click("maintenance-next",cx);
                    assert_eq!(view.read(cx).controller.stage(), ConfirmationStage::DatabaseReview);
                    stages.push("database-review");
                    window.render_frame(cx);
                    window.render_to_image().unwrap().save(root.join("maintenance-database-review.png")).unwrap();
                    window.click("maintenance-next",cx);
                    assert_eq!(view.read(cx).controller.stage(), ConfirmationStage::DatabaseFinal);
                    assert_eq!(calls.get(),0);
                    stages.push("database-final");
                    window.render_frame(cx);
                    window.render_to_image().unwrap().save(root.join("maintenance-database-final.png")).unwrap();
                    window.click("maintenance-next",cx);
                    assert_eq!(view.read(cx).controller.stage(), ConfirmationStage::DatabaseFinal);
                    view.update(cx,|v,cx|v.phrase.update(cx,|input,cx|input.set_value("删除数据库",window,cx)));
                    window.render_frame(cx);
                    window.click("maintenance-next",cx);
                    assert_eq!(view.read(cx).controller.stage(), ConfirmationStage::Ready);
                    assert_eq!(calls.get(),0);
                    stages.push("ready");
                    window.render_frame(cx);
                    window.click("maintenance-next",cx);
                    assert_eq!(calls.get(),1);
                    stages.push("fake-execution");
                } else {
                    window.click("maintenance-next",cx);
                    assert_eq!(view.read(cx).controller.stage(),ConfirmationStage::Ready);
                    assert_eq!(calls.get(),0);
                    stages.push("ready"); stages.push("cancel");
                }
                let selection = view.read(cx).controller.selection;
                window.render_frame(cx);
                window.click("maintenance-cancel",cx);
                selection
            }).unwrap();
            let after = std::fs::read(&database).ok();
            assert_eq!(after,before);
            let settings_unchanged = if let Some(before) = &corrupt_settings {
                std::fs::read(settings.join("manifest.json")).unwrap() == *before
                    && std::fs::read_dir(&settings).unwrap().count() == 1
            } else { !settings.exists() };
            assert!(settings_unchanged);
            assert!(!library.join("studyapp.lock").exists());
            std::fs::write(root.join("maintenance-smoke.json"),serde_json::to_vec_pretty(&serde_json::json!({
                "scenario":scenario,"passed":true,"stages":stages,"executor_calls":calls.get(),
                "selection":{"configuration":selection.configuration,"database":selection.database},
                "database_unchanged":after==before,"settings_absent":!settings.exists(),"settings_unchanged":settings_unchanged
            })).unwrap()).unwrap();
            cx.update(|cx|cx.quit());
        }).detach();
        })),
    )?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn maintenance_smoke_requires_runner_and_exact_bounded_arguments() {
        let args = vec![
            "--maintenance-smoke".into(),
            "missing".into(),
            "--data-dir".into(),
            r"C:\fixture\new".into(),
        ];
        assert!(parse_scenario(&args, false).is_err());
        assert!(parse_scenario(&args, true).unwrap().is_some());
        for bad in [
            vec!["--maintenance-smoke".into()],
            vec![
                "--maintenance-smoke".into(),
                "delete".into(),
                "--data-dir".into(),
                r"C:\fixture\new".into(),
            ],
            vec![
                "--maintenance-smoke".into(),
                "missing".into(),
                "--data-dir".into(),
                "../x".into(),
            ],
        ] {
            assert!(parse_scenario(&bad, true).is_err());
        }
        assert!(parse_scenario(&[], false).unwrap().is_none());
    }
}
