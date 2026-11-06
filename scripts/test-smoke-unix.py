"""Check Unix smoke evidence validation without opening native windows."""
import contextlib
import io
import subprocess
from unittest import mock
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location("smoke_unix", Path(__file__).with_name("smoke-unix.py"))
smoke = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(smoke)


class SmokeTests(unittest.TestCase):
    def test_missing_report_and_false_success_are_rejected(self):
        root = Path(__file__).resolve().parents[1] / ".scratch/cross-platform-012"
        root.mkdir(parents=True, exist_ok=True)
        data = Path(tempfile.mkdtemp(dir=root, prefix="smoke-proof-"))
        with self.assertRaises((FileNotFoundError, ValueError)):
            smoke.verify(data, interactive=False)
        (data / "smoke-result.txt").write_text("FAILED")
        with self.assertRaises(ValueError):
            smoke.verify(data, interactive=False)
        (data / "smoke-result.txt").write_text("PASS: WebKit ready")
        smoke.verify(data, interactive=False)
        with self.assertRaises((FileNotFoundError, ValueError)):
            smoke.verify(data, interactive=True)
        for index in range(5):
            (data / f"page-{index}.png").write_bytes(b"page-image" * 200)
        (data / "native-markdown-editor.png").write_bytes(b"initialized-editor" * 200)
        (data / "native-markdown-editor-root.png").write_bytes(b"native-screen" * 200)
        draft_id = "00000000-0000-4000-8000-000000000001"
        (data / "native-close-draft-id.txt").write_text(draft_id)
        (data / "drafts").mkdir()
        draft = {"schema": 2, "id": draft_id, "status": {"kind": "active"}, "revision": 1,
                 "draft": {"content": "关闭窗口前的末次输入 🧠"}}
        path = data / "drafts" / f"{draft_id}.json"
        path.write_text(json.dumps(draft), encoding="utf-8")
        with self.assertRaises((FileNotFoundError, ValueError)):
            smoke.verify(data, interactive=True)
        (data / "native-app-migration.txt").write_text("FAILED: profile relocation")
        with self.assertRaises(ValueError):
            smoke.verify(data, interactive=True)
        (data / "native-app-migration.txt").write_text("PASS: profile relocation")
        (data / "native-app-migration-contexts.json").write_text(json.dumps({
            "reader": {"profileContext": "0x1234", "profileOwners": 2},
            "preview": {"profileContext": "0x1234", "profileOwners": 2},
        }), encoding="utf-8")
        with self.assertRaises(FileNotFoundError):
            smoke.verify(data, interactive=True)
        (data / "native-timer-pin.json").write_text(json.dumps(self.pin_fixture()), encoding="utf-8")
        for target in ("true", "false"):
            (data / f"native-timer-pin-{target}.png").write_bytes(b"pin-fixture-image" * 200)
        smoke.verify(data, interactive=True)
        draft["draft"]["content"] = "stale content"
        path.write_text(json.dumps(draft), encoding="utf-8")
        with self.assertRaises(ValueError):
            smoke.verify(data, interactive=True)

    @staticmethod
    def pin_fixture():
        # Fabricated evidence belongs only to these verifier unit-test fixtures.
        return {"schema": 1, "desktopClosed": True, "states": [
            {"checked": target, "label": "取消置顶" if target else "置顶", "native": {"pinned": target, "managed": True,
             "focus": 123, "bounds": [80, 80, 380, 300]}}
            for target in (False, True, False)]}

    def test_native_pin_verifier_rejects_missing_ack_focus_move_and_incomplete_close(self):
        root = Path(__file__).resolve().parents[1] / ".scratch/cross-platform-012"
        root.mkdir(parents=True, exist_ok=True)
        data = Path(tempfile.mkdtemp(dir=root, prefix="pin-proof-"))
        cases = [("pinned", False), ("focus", 456), ("bounds", [0, 0, 380, 300]), ("managed", False)]
        for key, value in cases:
            evidence = self.pin_fixture()
            evidence["states"][1]["native"][key] = value
            (data / "native-timer-pin.json").write_text(json.dumps(evidence), encoding="utf-8")
            with self.subTest(key=key), self.assertRaises(ValueError):
                smoke.verify_pin(data)
        for bad in ({"schema": 1}, {**self.pin_fixture(), "desktopClosed": False}):
            (data / "native-timer-pin.json").write_text(json.dumps(bad), encoding="utf-8")
            with self.assertRaises(ValueError):
                smoke.verify_pin(data)

    def test_profile_context_verifier_requires_actual_shared_concurrent_owners(self):
        root = Path(__file__).resolve().parents[1] / ".scratch/cross-platform-012"
        root.mkdir(parents=True, exist_ok=True)
        data = Path(tempfile.mkdtemp(dir=root, prefix="context-proof-"))
        # Fabricated evidence belongs only to this verifier unit-test fixture.
        good = {"reader": {"profileContext": "0x1234", "profileOwners": 2},
                "preview": {"profileContext": "0x1234", "profileOwners": 2}}
        path = data / "native-app-migration-contexts.json"
        path.write_text(json.dumps(good), encoding="utf-8")
        smoke.verify_profile_contexts(data)
        for side, key, value in (("preview", "profileContext", "0x5678"),
                                 ("reader", "profileContext", None),
                                 ("reader", "profileContext", "0x0"),
                                 ("reader", "profileContext", "0x!"),
                                 ("reader", "profileContext", "0x"),
                                 ("reader", "profileContext", "0x" + "1" * 40),
                                 ("reader", "profileContext", "1234"),
                                 ("reader", "profileOwners", 1),
                                 ("preview", "profileOwners", True),
                                 ("preview", "profileOwners", 2.0)):
            bad = json.loads(json.dumps(good))
            bad[side][key] = value
            path.write_text(json.dumps(bad), encoding="utf-8")
            with self.subTest(side=side, key=key, value=value), self.assertRaises(ValueError):
                smoke.verify_profile_contexts(data)

    def test_linux_profile_contexts_are_shared_only_while_hosts_own_them(self):
        root = Path(__file__).resolve().parents[1]
        source = (root / "crates/studyapp-desktop/src/web_editor_host_unix.rs").read_text(encoding="utf-8")
        for check in ("std::fs::canonicalize(profile)", "PROFILE_CONTEXTS.with(",
                      "contexts.borrow_mut().acquire(", "is_custom_protocol_registered(\"studyapp\")",
                      "if register_protocol", "drop(context_borrow)", "Rc::strong_count(&self._context)",
                      "LOAD_TRACES.with(|traces| traces.borrow().get(_id).cloned())"):
            self.assertIn("".join(check.split()), "".join(source.split()))
        pool = (root / "crates/studyapp-desktop/src/editor_profile_context.rs").read_text(encoding="utf-8")
        for check in ("Weak<RefCell<T>>", "Weak::upgrade", "Rc::downgrade(&owner)",
                      "owner.strong_count() != 0"):
            self.assertIn(check, pool)
        smoke_source = (root / "crates/studyapp-desktop/src/smoke_library.rs").read_text(encoding="utf-8")
        self.assertIn('"native-app-migration-contexts.json"', smoke_source)
        self.assertIn('"live editor views must share the persistent profile owner"', smoke_source)
        workflow = (root / ".github/workflows/platforms.yml").read_text(encoding="utf-8")
        self.assertIn("inputs.profile_repetitions || '3'", workflow)
        self.assertIn('exit "$result"', workflow)
        self.assertIn('result=1', workflow)

    def test_isolated_wm_waits_for_ewmh_and_stops_its_own_child_on_failure(self):
        root = Path(__file__).resolve().parents[1] / ".scratch/cross-platform-012"
        root.mkdir(parents=True, exist_ok=True)
        data = Path(tempfile.mkdtemp(dir=root, prefix="wm-proof-")) / "native"
        process = mock.Mock()
        process.poll.return_value = None
        reply = mock.Mock(stdout="_NET_SUPPORTED(ATOM) = _NET_WM_STATE_ABOVE")
        with (
            mock.patch.object(smoke.sys, "platform", "linux"),
            mock.patch.object(smoke.subprocess, "Popen", return_value=process) as start,
            mock.patch.object(smoke.subprocess, "run", return_value=reply),
        ):
            with self.assertRaisesRegex(RuntimeError, "fixture failure"):
                with smoke.window_manager(data, True):
                    raise RuntimeError("fixture failure")
        self.assertEqual(start.call_args.args[0], ["openbox", "--sm-disable"])
        process.terminate.assert_called_once()
        process.wait.assert_called_once_with(timeout=5)

    def test_wm_exit_is_not_treated_as_readiness_and_noninteractive_skips_it(self):
        root = Path(__file__).resolve().parents[1] / ".scratch/cross-platform-012"
        root.mkdir(parents=True, exist_ok=True)
        data = Path(tempfile.mkdtemp(dir=root, prefix="wm-exit-proof-")) / "native"
        process = mock.Mock()
        process.poll.return_value = 1
        with (
            mock.patch.object(smoke.sys, "platform", "linux"),
            mock.patch.object(smoke.subprocess, "Popen", return_value=process) as start,
        ):
            with self.assertRaisesRegex(RuntimeError, "exited before readiness"):
                with smoke.window_manager(data, True):
                    self.fail("must not launch app without WM readiness")
            with smoke.window_manager(data, False):
                pass
            self.assertEqual(start.call_count, 1)
        process.terminate.assert_not_called()
        process.wait.assert_called_once_with(timeout=5)

    def test_native_pointer_launch_does_not_hide_the_window_or_create_a_console(self):
        root = Path(__file__).resolve().parents[1]
        for name in ("smoke-window-drag.ps1", "smoke-timer-widget.ps1", "smoke-note-rename.ps1"):
            source = (root / "scripts" / name).read_text(encoding="utf-8")
            launch = next(line for line in source.splitlines() if "Start-Process" in line)
            self.assertIn("-NoNewWindow", launch)
            self.assertNotIn("-WindowStyle", launch)
            self.assertIn("$env:GITHUB_ACTIONS -ne 'true'", source)
        pointer = (root / "crates/studyapp-desktop/src/update_acceptance_pointer.rs").read_text(encoding="utf-8")
        for check in ("WindowFromPoint(point)", "assert_eq!(hit_root, parent", "IsWindowVisible(parent)",
                      'fixture must own its target window'):
            self.assertIn(check, pointer)
        self.assertNotIn("HWND_TOPMOST", pointer)

    def test_rename_pass_requires_native_close_and_the_exact_preserved_draft(self):
        root = Path(__file__).resolve().parents[1]
        source = (root / "crates/studyapp-desktop/src/smoke_note_rename.rs").read_text(encoding="utf-8")
        for check in ("cx.on_window_closed(", "click_window_control(", '"window-close"',
                      "WindowsAndMessaging::HTCLOSE", "app.close_allowed", "app.note_editor.is_none()",
                      "app.draft_close_drain.is_none()", "app.draft_persist.is_none()",
                      'assert_eq!(stored["draft"], expected)', '"normalCloseDurable": true'):
            self.assertIn(check, source)
        self.assertLess(source.index("cx.on_window_closed("), source.index("std::fs::write(&report_path"))
        dispatch = (root / "crates/studyapp-desktop/src/smoke.rs").read_text(encoding="utf-8")
        branch = dispatch.split('== Ok("note-rename") {', 1)[1].split("return;", 1)[0]
        self.assertNotIn("cx.quit()", branch)
        harness = (root / "scripts/smoke-note-rename.ps1").read_text(encoding="utf-8")
        self.assertIn("'normalCloseDurable'", harness)
        self.assertIn("WaitForExit(120000)", harness)

    def test_widget_resize_waits_for_actual_pointer_delivery_before_hit_testing(self):
        root = Path(__file__).resolve().parents[1]
        source = (root / "crates/studyapp-desktop/src/smoke_timer_widget.rs").read_text(encoding="utf-8")
        corner = source.split("let corner = POINT", 1)[1].split("let resized = rect(handle)", 1)[0]
        self.assertLess(corner.index("pointer::hover(widget, corner, cx).await"),
                        corner.index("gesture(handle, corner, 55, 45, HTBOTTOMRIGHT)"))
        edges = source.split("for (name, point, expected) in points {", 1)[1].split("#[cfg(not(test))]", 1)[0]
        self.assertLess(edges.index("pointer::hover(widget, point, cx).await"),
                        edges.index("let actual = hit(handle, point)"))
        self.assertIn("actual, expected as isize", edges)
        pointer = (root / "crates/studyapp-desktop/src/update_acceptance_pointer.rs").read_text(encoding="utf-8")
        for check in ("assert_target(parent, target)", "window.mouse_position()", "ScreenToClient(parent",
                      "native.x == target.x", "native.y == target.y", "Duration::from_secs(2)"):
            self.assertIn(check, pointer)

    def test_native_settings_keep_notification_and_logging_receipts(self):
        root = Path(__file__).resolve().parents[1]
        workflow = (root / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        for directory in ("SMOKE_DIR", "SETTINGS_DIR", "OPTIMIZED_SETTINGS_DIR"):
            for receipt in ("notification-native-result.json", "logging-native-result.json"):
                self.assertIn("${{ env." + directory + " }}/" + receipt, workflow)

    def test_notification_cards_share_frame_policy_and_preserve_fit_focus_and_cache_evidence(self):
        root = Path(__file__).resolve().parents[1]
        desktop = (root / "crates/studyapp-desktop/src/notification_desktop.rs").read_text(encoding="utf-8")
        native = (root / "crates/studyapp-desktop/src/card_window.rs").read_text(encoding="utf-8")
        timer = (root / "crates/studyapp-desktop/src/timer_widget_window.rs").read_text(encoding="utf-8")
        smoke_source = (root / "crates/studyapp-desktop/src/smoke_notifications.rs").read_text(encoding="utf-8")
        workflow = (root / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        for source in (desktop, timer):
            self.assertIn("crate::card_window::disable_native_frame(window)?", source)
        self.assertIn("DWMNCRP_DISABLED", native)
        self.assertIn("SWP_NOACTIVATE | SWP_NOZORDER", native)
        self.assertIn("window.defer(cx", desktop)
        self.assertIn("bounds.size.height", desktop)
        for check in ('native["nonClientRendering"], false', "WS_EX_NOACTIVATE", "relevant notification edit must rebuild visible controls only",
                      "fitting G must not activate it", "stacking G must not activate it", "shrinking G must not activate it"):
            self.assertIn(check, smoke_source)
        for directory in ("SMOKE_DIR", "SETTINGS_DIR", "OPTIMIZED_SETTINGS_DIR"):
            self.assertIn("${{ env." + directory + " }}/notification-*.png", workflow)
        self.assertIn("settings-notification-cache-timing.json", smoke_source)

    def test_mac_editor_has_no_persistent_browser_state_and_keeps_origin_checks(self):
        root = Path(__file__).resolve().parents[1]
        source = (root / "crates/studyapp-desktop/src/web_editor_host_unix.rs").read_text(encoding="utf-8")
        self.assertIn(".with_incognito(true)", source)
        self.assertIn("BackgroundThrottlingPolicy::Disabled", source)
        for boundary in (".with_url(EDITOR_PAGE)", ".with_visible(false)", ".with_focused(false)",
                         "PermissionResponse::Deny", "NewWindowResponse::Deny",
                         "editor_page_allowed(&url)", "ipc_allowed(&request.uri().to_string()"):
            self.assertIn(boundary, source)

    def test_application_migration_preserves_native_data_and_platform_browser_contract(self):
        root = Path(__file__).resolve().parents[1]
        source = (root / "crates/studyapp-desktop/src/smoke_library.rs").read_text(encoding="utf-8")
        body = source.split("pub(crate) async fn application_migration(", 1)[1].split("#[cfg(windows)]", 1)[0]
        for check in ('"migrate-application"', '"confirm-application-migration"',
                      'marker(true, window, view, cx)', 'marker(false, window, view, cx)',
                      '== Some(&json!("persisted"))', 'let expected_marker = if cfg!(target_os = "macos")',
                      'Value::Null', 'json!("persisted")', 'assert_eq!(observed_marker, expected_marker',
                      '"native-app-migration-browser.json"', '"outline preference durably saved before relocation"',
                      'after_editor["outline"], json!(outline)', 'before_editor["text"], expected_view_text',
                      'after_editor["text"], expected_view_text', 'after_editor["readonly"], true', 'after_canonical, expected_text',
                      '"versions": snapshot.versions', '"settings": snapshot.settings',
                      '"tableViews": snapshot.table_views', '"folders": snapshot.folders',
                      'assert!(old.exists())', 'Work::SwitchLibrary(old.parent().unwrap().into())'):
            self.assertIn("".join(check.split()), "".join(body.split()))
        self.assertIn('const TIMEOUT: Duration = Duration::from_secs(5)', source)
        self.assertNotIn('with_incognito(false)', body)
        self.assertLess(body.index('"return to original library"'), body.index('"restore prior outline after migration fixture"'))
        for check in ('"name":original_outline.position', '"shown":original_outline.shown',
                      'restored["outline"], json!(original_outline)', 'Some(original_outline)'):
            self.assertIn("".join(check.split()), "".join(body.split()))
        dispatch = (root / "crates/studyapp-desktop/src/smoke_editor.rs").read_text(encoding="utf-8")
        close = dispatch.split("pub async fn close_with_draft(", 1)[1]
        self.assertIn('crate::smoke_visibility::type_into(window, &editor, "关闭窗口前的末次输入', close)
        self.assertNotIn('"type":"outlinePreference"', close)

    def test_migration_runner_keeps_bounded_original_error_without_changing_the_result(self):
        root = Path(__file__).resolve().parents[1]
        source = (root / "crates/studyapp-desktop/src/worker.rs").read_text(encoding="utf-8")
        body = source.split("Ok(Work::MigrateApplication(directory)) =>", 1)[1].split("Ok(Work::MigrateLibrary(directory)) =>", 1)[0]
        self.assertIn('let diagnostic = format!("{error:#}")', body)
        self.assertRegex(body, r'#\[cfg\(feature = "ui-test"\)\]\s*eprintln!')
        self.assertIn('diagnostic.chars().take(4096).collect::<String>()', body)
        self.assertIn('Err(diagnostic)', body)
        self.assertIn('storage_event = Some(StorageEvent::Finished(result))', body)
        self.assertNotIn('retry', body.lower())

    def test_closed_profile_recovery_is_production_bounded_and_not_a_smoke_retry(self):
        root = Path(__file__).resolve().parents[1]
        source = (root / "crates/studyapp-desktop/src/app_migration.rs").read_text(encoding="utf-8")
        body = source.split("pub fn prepare_relocating_profile(",1)[1].split("fn prepare_checked(",1)[0]
        self.assertLess(body.index("let mut settling = 80"), body.index("loop {"))
        for check in ("settling -= 1", "Duration::from_millis(125)", "pass < 4", "settling > 0", "cause.is::<TransientProfile>()"):
            self.assertIn(check, body)
        self.assertIn("prepare_with_profile_wait(",body)
        self.assertNotIn("publish(",body)
        worker = (root / "crates/studyapp-desktop/src/worker.rs").read_text(encoding="utf-8")
        self.assertIn("crate::app_migration::prepare_relocating_profile(engine, paths, target, now",worker)
        smoke = (root / "crates/studyapp-desktop/src/smoke_library.rs").read_text(encoding="utf-8")
        scenario = smoke.split("pub(crate) async fn application_migration(",1)[1]
        self.assertEqual(scenario.count('"confirm-application-migration"'),1)
        self.assertNotIn("prepare_relocating_profile",scenario)
        self.assertIn("Duration::from_secs(90)",scenario)

    def test_profile_evidence_is_after_the_tested_close_and_preserves_browser_assertion(self):
        root = Path(__file__).resolve().parents[1]
        source = (root / "crates/studyapp-desktop/src/smoke_library.rs").read_text(encoding="utf-8")
        body = source.split("pub(crate) async fn application_migration(",1)[1].split("#[cfg(windows)]",1)[0]
        self.assertLess(body.index('"application migrated"'),body.index('profile_evidence("before-host")'))
        self.assertLess(body.index('profile_evidence("before-host")'),body.index('"native data changed during application relocation"'))
        self.assertLess(body.index('profile_evidence("after-host")'),body.index('"relocated browser storage contract"'))
        self.assertIn("".join('assert_eq!(observed_marker, expected_marker'.split()), "".join(body.split()))
        evidence = source.split("fn migration_profile_evidence(",1)[1].split("pub(crate) async fn application_migration(",1)[0]
        for check in ('files.len() < 64', '8 * 1024 * 1024', '"sha256"', '"hasMarker"', '"hasPersisted"'):
            self.assertIn(check,evidence)
        for write in ('fs::write', 'remove_file', 'create_dir', 'localStorage.setItem'):
            self.assertNotIn(write,evidence)

    def test_mac_bundled_document_keeps_exact_request_origin_and_compositor_capture(self):
        root = Path(__file__).resolve().parents[1]
        source = (root / "crates/studyapp-desktop/src/web_editor_host_unix.rs").read_text(encoding="utf-8")
        for boundary in ("NSURL::URLWithString(&NSString::from_str(EDITOR_PAGE))",
                         "NSURLRequest::requestWithURL(&url)",
                         "super::single_document_editor().1",
                         "loadSimulatedRequest_responseHTMLString("):
            self.assertIn(boundary, source)
        self.assertNotIn(".with_html(", source)
        self.assertNotIn("loadHTMLString_baseURL", source)
        captures = (root / "crates/studyapp-desktop/src/smoke.rs").read_text(encoding="utf-8")
        self.assertIn('args(["-x", "-o", "-l", &id.to_string()])', captures)
        self.assertIn("windowNumber()", captures)
        self.assertIn("Native macOS captures are restricted to GitHub runners", captures)

    def test_first_readonly_baseline_closes_real_startup_notices_without_forcing_the_host(self):
        root = Path(__file__).resolve().parents[1]
        source = (root / "crates/studyapp-desktop/src/smoke_editor.rs").read_text(encoding="utf-8")
        setup = source.split("pub async fn default_preview(", 1)[1].split("async fn first_readonly_ready(", 1)[0]
        self.assertLess(setup.index("dismiss_notice_before_editor(window, view, cx).await"),
                        setup.index("app.page = Page::Notes"))
        close = source.split("async fn dismiss_notice_before_editor(", 1)[1].split("/// First readonly paint", 1)[0]
        for check in ('format!("dismiss-notice-{id}")', "window.click(keyed, cx)",
                      "test_is_closing(id)", "occlusions().is_empty()", "Duration::from_secs(5)"):
            self.assertIn(check, close)
        for bypass in ("clear_notices", ".clear(", ".set_visible(", ".set_theme(", ".evaluate(", ".focus("):
            self.assertNotIn(bypass, close)

    def test_first_readiness_deadline_captures_evidence_without_silently_repairing(self):
        root = Path(__file__).resolve().parents[1]
        source = (root / "crates/studyapp-desktop/src/smoke_editor.rs").read_text(encoding="utf-8")
        body = source.split("async fn first_readonly_ready(", 1)[1].split("async fn capture_default_preview(", 1)[0]
        self.assertIn("Duration::from_secs(5)", body)
        self.assertIn("native-markdown-editor-default-ready-deadline.json", body)
        self.assertIn("panic!(", body)
        for call in (".set_visible(", ".set_theme(", ".evaluate(", ".focus(", ".requestMeasure("):
            self.assertNotIn(call, body)
        self.assertIn("const TIMEOUT: Duration = Duration::from_secs(5)",
                      (root / "crates/studyapp-desktop/src/smoke_library.rs").read_text(encoding="utf-8"))

    def test_native_failure_prints_captured_stderr_and_still_fails(self):
        root = Path(__file__).resolve().parents[1] / ".scratch/cross-platform-012"
        root.mkdir(parents=True, exist_ok=True)
        data = Path(tempfile.mkdtemp(dir=root, prefix="smoke-failure-")) / "native"
        process = mock.MagicMock()
        process.__enter__.return_value = process
        process.wait.return_value = 101
        def fail(command, **kwargs):
            kwargs["stderr"].write("panic: localstorage-wal vanished" + chr(10))
            return process
        captured = io.StringIO()
        with mock.patch.object(smoke.subprocess, "Popen", side_effect=fail) as start:
            with contextlib.redirect_stderr(captured):
                with self.assertRaises(subprocess.CalledProcessError):
                    smoke.run_native(Path(__file__), data, True)
        self.assertIn("panic: localstorage-wal vanished", captured.getvalue())
        self.assertIn("--smoke-app-dir", start.call_args.args[0])
        process.wait.assert_called_once_with(timeout=480)
        process.kill.assert_not_called()
        self.assertTrue(data.with_suffix(".stderr.log").is_file())

    def test_timeout_samples_only_its_child_then_kills_it_and_still_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            data = Path(directory) / "native"
            process = mock.MagicMock(pid=1234)
            process.__enter__.return_value = process
            process.wait.side_effect = [subprocess.TimeoutExpired("fixture", 480), -9]
            order = mock.Mock()
            order.attach_mock(process.kill, "kill")
            with (mock.patch.object(smoke.subprocess, "Popen", return_value=process),
                  mock.patch.object(smoke, "sample_timed_out_child") as sample,
                  contextlib.redirect_stderr(io.StringIO())):
                order.attach_mock(sample, "sample")
                with self.assertRaises(subprocess.TimeoutExpired):
                    smoke.run_native(Path(__file__), data, True)
            self.assertEqual(order.mock_calls, [mock.call.sample(process, data), mock.call.kill()])
            self.assertEqual(process.wait.call_args_list, [mock.call(timeout=480), mock.call(timeout=5)])

    def test_sample_is_passive_bounded_and_restricted_to_live_macos_runner_child(self):
        with tempfile.TemporaryDirectory() as directory:
            data = Path(directory) / "native"
            process = mock.Mock(pid=1234)
            process.poll.return_value = None
            with (mock.patch.object(smoke.sys, "platform", "darwin"),
                  mock.patch.dict(smoke.os.environ, {"GITHUB_ACTIONS": "true"}),
                  mock.patch.object(smoke.subprocess, "run") as run):
                smoke.sample_timed_out_child(process, data)
                self.assertEqual(run.call_args.args[0],
                                 ["/usr/bin/sample", "1234", "2", "-file", str(data.with_suffix(".sample.txt"))])
                self.assertEqual(run.call_args.kwargs["timeout"], 10)
                process.poll.return_value = 0
                smoke.sample_timed_out_child(process, data)
                process.poll.return_value = None
                with mock.patch.dict(smoke.os.environ, {"GITHUB_ACTIONS": "false"}):
                    smoke.sample_timed_out_child(process, data)
                with mock.patch.object(smoke.sys, "platform", "linux"):
                    smoke.sample_timed_out_child(process, data)
                self.assertEqual(run.call_count, 1)
            process.kill.assert_not_called()
            process.terminate.assert_not_called()

    def test_unavailable_sample_does_not_replace_the_original_timeout(self):
        with tempfile.TemporaryDirectory() as directory:
            process = mock.Mock(pid=1234)
            process.poll.return_value = None
            with (mock.patch.object(smoke.sys, "platform", "darwin"),
                  mock.patch.dict(smoke.os.environ, {"GITHUB_ACTIONS": "true"}),
                  mock.patch.object(smoke.subprocess, "run", side_effect=OSError("fixture")),
                  contextlib.redirect_stderr(io.StringIO()) as captured):
                smoke.sample_timed_out_child(process, Path(directory) / "native")
            self.assertIn("sample unavailable: OSError", captured.getvalue())


if __name__ == "__main__":
    unittest.main()
