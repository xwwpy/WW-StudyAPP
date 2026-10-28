//! Real Microsoft Pinyin acceptance. This module never supplies committed text.
use super::{key, tick};
use crate::{
    app::{Page, StudyApp},
    folders_ui::{FolderEvent, FolderFilter},
    markdown_editor::MarkdownEditor,
};
use anyhow::{Context as _, Result, anyhow, bail, ensure};
use gpui_kit::{
    component::{WindowExt, input::InputState},
    native_ime_observer::{self, ImeStringKind, TraceGuard},
    test::TestWindowExt,
    *,
};
use serde_json::{Value, json};
use std::{cell::RefCell, ffi::c_void, marker::PhantomData, path::Path, rc::Rc};
use studyapp_core::Command;
use windows::{
    Win32::{
        Foundation::{FreeLibrary, HMODULE, HWND, POINT, RECT, S_OK},
        Graphics::Gdi::*,
        System::{
            Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
            LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW},
            Threading::GetCurrentThreadId,
            Variant::{VT_I4, VariantClear},
        },
        UI::{
            Input::{
                Ime::{
                    IME_CONVERSION_MODE, IME_SENTENCE_MODE, ImmGetContext, ImmGetConversionStatus,
                    ImmGetOpenStatus, ImmReleaseContext,
                },
                KeyboardAndMouse::{
                    GetFocus, HKL, INPUT, INPUT_0, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
                    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEINPUT, SendInput, VIRTUAL_KEY,
                    VK_RETURN, VK_SHIFT, VK_SPACE,
                },
            },
            TextServices::*,
            WindowsAndMessaging::{
                GetClientRect, GetForegroundWindow, GetWindowThreadProcessId, IsChild,
                SetCursorPos, WM_IME_ENDCOMPOSITION, WM_IME_STARTCOMPOSITION, WindowFromPoint,
            },
        },
    },
    core::{GUID, HRESULT, Interface, s, w},
};

const ZH_CN: u16 = 0x0804;
const PINYIN_CLSID: GUID = GUID::from_u128(0x81d4e9c9_1d3b_41bc_9e6c_4b40bf79e35e);
const PINYIN_PROFILE: GUID = GUID::from_u128(0xfa550b04_5ad7_411f_a5ac_ca038ec515d7);

fn parent(native: &Value) -> Result<HWND> {
    let hwnd = native["parentHwnd"]
        .as_u64()
        .context("parent HWND missing")?;
    ensure!(hwnd != 0, "parent HWND is null");
    Ok(HWND(hwnd as usize as *mut _))
}

fn send(native: &Value, events: &[INPUT]) -> Result<()> {
    ensure!(
        unsafe { GetForegroundWindow() } == parent(native)?,
        "IME window lost foreground"
    );
    let sent = unsafe { SendInput(events, std::mem::size_of::<INPUT>() as i32) };
    ensure!(
        sent as usize == events.len(),
        "SendInput accepted {sent}/{} events",
        events.len()
    );
    Ok(())
}

fn press(native: &Value, code: VIRTUAL_KEY) -> Result<()> {
    ensure!(
        unsafe { GetFocus() } == parent(native)?,
        "IME key target is not the GPUI parent HWND"
    );
    send(
        native,
        &[key(code, KEYBD_EVENT_FLAGS(0)), key(code, KEYEVENTF_KEYUP)],
    )
}

fn click_input(
    window: AnyWindowHandle,
    input: &Entity<InputState>,
    native: &Value,
    cx: &mut AsyncApp,
) -> Result<()> {
    let center = window.update(cx, |_, window, cx| -> Result<_> {
        window.render_frame(cx);
        let target = window
            .try_find(("input", input.entity_id()))
            .context("folder input not rendered")?;
        ensure!(target.visible(), "folder input is not visible");
        Ok(target.bounds().center())
    })??;
    let parent = parent(native)?;
    let scale = native["logicalBounds"][4]
        .as_f64()
        .context("window DPI missing")?
        / 96.;
    ensure!(scale.is_finite() && scale > 0., "invalid window DPI");
    let mut position = POINT {
        x: (f64::from(f32::from(center.x)) * scale).round() as i32,
        y: (f64::from(f32::from(center.y)) * scale).round() as i32,
    };
    ensure!(
        unsafe { ClientToScreen(parent, &mut position) }.as_bool(),
        "input ClientToScreen failed"
    );
    ensure!(
        unsafe { GetForegroundWindow() } == parent,
        "folder window is not foreground"
    );
    ensure!(
        unsafe { WindowFromPoint(position) } == parent,
        "folder input is occluded"
    );
    unsafe { SetCursorPos(position.x, position.y)? };
    let events = [MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP].map(|flags| INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dwFlags: flags,
                ..Default::default()
            },
        },
    });
    send(native, &events)
}

/// Capture actual desktop pixels without changing focus or activating a window.
fn capture(parent: HWND, path: &Path) -> Result<()> {
    let mut bounds = RECT::default();
    unsafe { GetClientRect(parent, &mut bounds)? };
    let (width, height) = (bounds.right - bounds.left, bounds.bottom - bounds.top);
    ensure!(
        width > 0 && height > 0 && i64::from(width) * i64::from(height) <= 25_000_000,
        "invalid screenshot dimensions"
    );
    let mut origin = POINT {
        x: bounds.left,
        y: bounds.top,
    };
    ensure!(
        unsafe { ClientToScreen(parent, &mut origin) }.as_bool(),
        "screenshot ClientToScreen failed"
    );
    let target = unsafe {
        WindowFromPoint(POINT {
            x: origin.x + width / 2,
            y: origin.y + height / 2,
        })
    };
    ensure!(
        target == parent || unsafe { IsChild(parent, target) }.as_bool(),
        "screenshot target is obscured"
    );
    let length = width as usize * height as usize * 4;
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let pixels = unsafe {
        let screen = GetDC(None);
        ensure!(!screen.0.is_null(), "GetDC failed");
        let memory = CreateCompatibleDC(Some(screen));
        if memory.0.is_null() {
            ReleaseDC(None, screen);
            bail!("CreateCompatibleDC failed");
        }
        let mut bits = std::ptr::null_mut();
        let bitmap = match CreateDIBSection(Some(screen), &info, DIB_RGB_COLORS, &mut bits, None, 0)
        {
            Ok(bitmap) => bitmap,
            Err(error) => {
                let _ = DeleteDC(memory);
                ReleaseDC(None, screen);
                return Err(error.into());
            }
        };
        let previous = SelectObject(memory, HGDIOBJ(bitmap.0));
        if previous.0.is_null() || previous.0 as isize == -1 {
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            let _ = DeleteDC(memory);
            ReleaseDC(None, screen);
            bail!("screenshot SelectObject failed");
        }
        let copied = BitBlt(
            memory,
            0,
            0,
            width,
            height,
            Some(screen),
            origin.x,
            origin.y,
            SRCCOPY,
        );
        let pixels = if copied.is_ok() && !bits.is_null() {
            Some(std::slice::from_raw_parts(bits as *const u8, length).to_vec())
        } else {
            None
        };
        let _ = SelectObject(memory, previous);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
        copied?;
        pixels.context("screenshot bitmap had no storage")?
    };
    let mut bmp = Vec::with_capacity(54 + length);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&((54 + length) as u32).to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&54_u32.to_le_bytes());
    bmp.extend_from_slice(&40_u32.to_le_bytes());
    bmp.extend_from_slice(&width.to_le_bytes());
    bmp.extend_from_slice(&(-height).to_le_bytes());
    bmp.extend_from_slice(&1_u16.to_le_bytes());
    bmp.extend_from_slice(&32_u16.to_le_bytes());
    bmp.extend_from_slice(&[0; 24]);
    bmp.extend_from_slice(&pixels);
    std::fs::write(path, bmp)?;
    Ok(())
}

struct SystemModule(HMODULE);
impl Drop for SystemModule {
    fn drop(&mut self) {
        unsafe {
            let _ = FreeLibrary(self.0);
        }
    }
}

struct ImeSession {
    profiles: ITfInputProcessorProfiles,
    manager: ITfInputProcessorProfileMgr,
    thread_manager: ITfThreadMgr,
    original: TF_INPUTPROCESSORPROFILE,
    original_language: u16,
    thread: u32,
    owns_activation: bool,
    changed_language: bool,
    _module: SystemModule,
    _thread_bound: PhantomData<Rc<()>>,
}

fn profile_json(profile: &TF_INPUTPROCESSORPROFILE) -> Value {
    json!({"type":profile.dwProfileType,"langid":profile.langid,
        "clsid":format!("{:?}", profile.clsid),"profile":format!("{:?}",profile.guidProfile),
        "category":format!("{:?}",profile.catid),"hkl":profile.hkl.0 as usize,"flags":profile.dwFlags})
}

fn active_profile(manager: &ITfInputProcessorProfileMgr) -> Result<TF_INPUTPROCESSORPROFILE> {
    let mut profile = TF_INPUTPROCESSORPROFILE::default();
    let hr = unsafe {
        (Interface::vtable(manager).GetActiveProfile)(
            Interface::as_raw(manager),
            &GUID_TFCAT_TIP_KEYBOARD,
            &mut profile,
        )
    };
    ensure!(hr == S_OK, "GetActiveProfile returned {hr:?}");
    ensure!(
        profile.dwFlags & TF_IPP_FLAG_ACTIVE != 0,
        "keyboard profile is not active"
    );
    Ok(profile)
}

fn is_pinyin(profile: &TF_INPUTPROCESSORPROFILE) -> bool {
    profile.dwProfileType == TF_PROFILETYPE_INPUTPROCESSOR
        && profile.langid == ZH_CN
        && profile.clsid == PINYIN_CLSID
        && profile.guidProfile == PINYIN_PROFILE
        && profile.dwFlags & (TF_IPP_FLAG_ACTIVE | TF_IPP_FLAG_ENABLED)
            == (TF_IPP_FLAG_ACTIVE | TF_IPP_FLAG_ENABLED)
}

impl ImeSession {
    fn create(parent: HWND, evidence: &mut Evidence) -> Result<Self> {
        let thread = unsafe { GetCurrentThreadId() };
        ensure!(
            unsafe { GetWindowThreadProcessId(parent, None) } == thread,
            "IME setup is not on the window UI thread"
        );
        let module = SystemModule(unsafe {
            LoadLibraryExW(w!("msctf.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)?
        });
        let function = unsafe { GetProcAddress(module.0, s!("TF_GetThreadMgr")) }
            .context("TF_GetThreadMgr export missing")?;
        let get_thread_manager: unsafe extern "system" fn(*mut *mut c_void) -> HRESULT =
            unsafe { std::mem::transmute(function) };
        let mut raw = std::ptr::null_mut();
        let hr = unsafe { get_thread_manager(&mut raw) };
        evidence.stages.push(json!({"stage":"thread-manager","hresult":hr.0,"existing":!raw.is_null(),"thread":thread}));
        hr.ok()?;
        let thread_manager: ITfThreadMgr = if raw.is_null() {
            unsafe { CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER)? }
        } else {
            unsafe { ITfThreadMgr::from_raw(raw) }
        };
        let profiles: ITfInputProcessorProfiles = unsafe {
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?
        };
        let manager: ITfInputProcessorProfileMgr = profiles.cast()?;
        let original = active_profile(&manager)?;
        let original_language = unsafe { profiles.GetCurrentLanguage()? };
        evidence.stages.push(json!({"stage":"original-profile","profile":profile_json(&original),"language":original_language}));
        let entries = unsafe { manager.EnumProfiles(ZH_CN)? };
        let mut installed = false;
        let mut finished = false;
        for _ in 0..512 {
            let mut entry = [TF_INPUTPROCESSORPROFILE::default()];
            let mut fetched = 0;
            unsafe { entries.Next(&mut entry, &mut fetched)? };
            if fetched == 0 {
                finished = true;
                break;
            }
            ensure!(fetched == 1, "unexpected TSF profile enumeration count");
            let profile = entry[0];
            evidence
                .stages
                .push(json!({"stage":"installed-profile","profile":profile_json(&profile)}));
            installed |= profile.dwProfileType == TF_PROFILETYPE_INPUTPROCESSOR
                && profile.langid == ZH_CN
                && profile.clsid == PINYIN_CLSID
                && profile.guidProfile == PINYIN_PROFILE
                && profile.dwFlags & TF_IPP_FLAG_ENABLED != 0;
        }
        ensure!(finished, "TSF profile enumeration exceeded its bound");
        ensure!(
            installed,
            "Microsoft Pinyin is not installed and enabled for this runner user"
        );
        let mut session = Self {
            profiles,
            manager,
            thread_manager,
            original,
            original_language,
            thread,
            owns_activation: false,
            changed_language: false,
            _module: module,
            _thread_bound: PhantomData,
        };
        let mut client = 0;
        let hr = unsafe {
            (Interface::vtable(&session.thread_manager).Activate)(
                Interface::as_raw(&session.thread_manager),
                &mut client,
            )
        };
        session.owns_activation = hr == S_OK;
        evidence.stages.push(json!({"stage":"owned-thread-activation","hresult":hr.0,"clientId":client,"thread":thread}));
        if hr != S_OK {
            evidence.stages.push(
                json!({"stage":"activation-cleanup","result":format!("{:?}",session.restore())}),
            );
            bail!("Thread manager activation did not return S_OK: {hr:?}");
        }
        Ok(session)
    }

    fn activate(&mut self, evidence: &mut Evidence) -> Result<()> {
        ensure!(
            self.thread == unsafe { GetCurrentThreadId() },
            "IME activation changed threads"
        );
        // Mark before the call so a partial language change is also restored.
        self.changed_language = true;
        let language_result = unsafe { self.profiles.ChangeCurrentLanguage(ZH_CN) };
        evidence
            .stages
            .push(json!({"stage":"change-language","result":format!("{language_result:?}")}));
        language_result?;
        let hr = unsafe {
            (Interface::vtable(&self.manager).ActivateProfile)(
                Interface::as_raw(&self.manager),
                TF_PROFILETYPE_INPUTPROCESSOR,
                ZH_CN,
                &PINYIN_CLSID,
                &PINYIN_PROFILE,
                HKL::default(),
                TF_IPPMF_FORPROCESS,
            )
        };
        evidence
            .stages
            .push(json!({"stage":"activate-pinyin","hresult":hr.0,"scope":"current-process"}));
        ensure!(hr == S_OK, "Pinyin activation did not return S_OK: {hr:?}");
        Ok(())
    }

    fn restore(&mut self) -> Result<Value> {
        ensure!(
            self.thread == unsafe { GetCurrentThreadId() },
            "IME restoration changed threads"
        );
        let mut errors = Vec::new();
        let mut report = json!({"stage":"restore-profile","attempted":self.changed_language});
        if self.changed_language {
            let language = unsafe { self.profiles.ChangeCurrentLanguage(self.original_language) };
            report["languageResult"] = json!(format!("{language:?}"));
            if let Err(error) = language {
                errors.push(error.to_string());
            }
            let hr = unsafe {
                (Interface::vtable(&self.manager).ActivateProfile)(
                    Interface::as_raw(&self.manager),
                    self.original.dwProfileType,
                    self.original.langid,
                    &self.original.clsid,
                    &self.original.guidProfile,
                    if self.original.dwProfileType == TF_PROFILETYPE_KEYBOARDLAYOUT {
                        self.original.hkl
                    } else {
                        HKL::default()
                    },
                    TF_IPPMF_FORPROCESS,
                )
            };
            report["hresult"] = json!(hr.0);
            if hr != S_OK {
                errors.push(format!("restore profile: {hr:?}"));
            }
            match active_profile(&self.manager) {
                Ok(current) => {
                    let matches = current.dwProfileType == self.original.dwProfileType
                        && current.langid == self.original.langid
                        && current.clsid == self.original.clsid
                        && current.guidProfile == self.original.guidProfile
                        && (self.original.dwProfileType != TF_PROFILETYPE_KEYBOARDLAYOUT
                            || current.hkl == self.original.hkl);
                    report["actualProfile"] = profile_json(&current);
                    if !matches {
                        errors.push("restored profile differs from original".into());
                    }
                }
                Err(error) => errors.push(error.to_string()),
            }
            match unsafe { self.profiles.GetCurrentLanguage() } {
                Ok(language) => {
                    report["actualLanguage"] = json!(language);
                    if language != self.original_language {
                        errors.push("restored language differs from original".into());
                    }
                }
                Err(error) => errors.push(error.to_string()),
            }
            self.changed_language = false;
        }
        if self.owns_activation {
            let hr = unsafe {
                (Interface::vtable(&self.thread_manager).Deactivate)(Interface::as_raw(
                    &self.thread_manager,
                ))
            };
            report["deactivateHresult"] = json!(hr.0);
            if hr != S_OK {
                errors.push(format!("Deactivate: {hr:?}"));
            }
            self.owns_activation = false;
        }
        report["errors"] = json!(errors);
        if errors.is_empty() {
            Ok(report)
        } else {
            Err(anyhow!(report.to_string()))
        }
    }

    fn state(&self, parent: HWND) -> Result<Value> {
        ensure!(
            self.thread == unsafe { GetCurrentThreadId() },
            "IME state read changed threads"
        );
        let profile = active_profile(&self.manager)?;
        let compartments: ITfCompartmentMgr = self.thread_manager.cast()?;
        let open = read_i4(&compartments, &GUID_COMPARTMENT_KEYBOARD_OPENCLOSE);
        let conversion = read_i4(
            &compartments,
            &GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION,
        );
        let chinese = match (&open, &conversion) {
            (Ok(open), Ok(conversion)) => Some(
                *open != 0
                    && *conversion as u32 & TF_CONVERSIONMODE_NATIVE != 0
                    && *conversion as u32 & TF_CONVERSIONMODE_NOCONVERSION == 0,
            ),
            _ => None,
        };
        let context = unsafe { ImmGetContext(parent) };
        let mut imm = json!({"contextPresent":!context.0.is_null()});
        if !context.0.is_null() {
            let mut mode = IME_CONVERSION_MODE::default();
            let mut sentence = IME_SENTENCE_MODE::default();
            imm["open"] = json!(unsafe { ImmGetOpenStatus(context) }.as_bool());
            imm["conversionAvailable"] = json!(
                unsafe { ImmGetConversionStatus(context, Some(&mut mode), Some(&mut sentence)) }
                    .as_bool()
            );
            imm["conversion"] = json!(mode.0);
            imm["sentence"] = json!(sentence.0);
            imm["released"] = json!(unsafe { ImmReleaseContext(parent, context) }.as_bool());
        }
        Ok(
            json!({"profile":profile_json(&profile),"pinyinActive":is_pinyin(&profile),
            "open":open.as_ref().ok(),"openError":open.err().map(|error|error.to_string()),
            "conversion":conversion.as_ref().ok(),"conversionError":conversion.err().map(|error|error.to_string()),
            "chineseMode":chinese,"focusHwnd":unsafe {GetFocus()}.0 as usize,
            "foregroundHwnd":unsafe {GetForegroundWindow()}.0 as usize,"imm":imm}),
        )
    }
}

impl Drop for ImeSession {
    fn drop(&mut self) {
        if self.thread == unsafe { GetCurrentThreadId() } {
            let _ = self.restore();
        }
    }
}

fn read_i4(compartments: &ITfCompartmentMgr, key: &GUID) -> Result<i32> {
    let compartment = unsafe { compartments.GetCompartment(key)? };
    let mut value = unsafe { compartment.GetValue()? };
    let result = unsafe {
        if value.Anonymous.Anonymous.vt == VT_I4 {
            Ok(value.Anonymous.Anonymous.Anonymous.lVal)
        } else {
            Err(anyhow!(
                "TSF compartment has type {:?}, expected VT_I4",
                value.Anonymous.Anonymous.vt
            ))
        }
    };
    unsafe { VariantClear(&mut value)? };
    result
}

#[derive(Default)]
struct Evidence {
    stages: Vec<Value>,
    native_events: Vec<Value>,
    error: Option<String>,
    passed: bool,
}
impl Evidence {
    fn persist(&self, database: &Path) -> Result<()> {
        std::fs::write(
            database.with_file_name("native-ime-evidence.json"),
            serde_json::to_vec_pretty(&json!({
            "status":if self.passed {"passed"} else if self.error.is_some() {"failed"} else {"incomplete"}, "error":self.error,
            "stages":self.stages,"nativeEvents":self.native_events,
            "inputMethod":"ordinary Win32 SendInput virtual keys; no Unicode packet input"}))?,
        )?;
        Ok(())
    }
}

struct Case<'a> {
    window: AnyWindowHandle,
    view: &'a Entity<StudyApp>,
    native: &'a Value,
    database: &'a Path,
    session: &'a ImeSession,
    name: &'a str,
    input: Entity<InputState>,
    editor: Entity<MarkdownEditor>,
    original_editor: String,
    original_folders: Vec<String>,
    commands: Rc<RefCell<Vec<Command>>>,
    _subscription: Subscription,
    trace: TraceGuard,
    composing: bool,
    saw_composition: bool,
    saw_end: bool,
    results: Vec<String>,
}

impl Case<'_> {
    fn sample(&mut self, stage: &str, evidence: &mut Evidence, cx: &mut AsyncApp) -> Result<Value> {
        let (sample, batch) = self.window.update(cx, |_, window, cx| -> Result<_> {
            window.render_frame(cx);
            let focused_input_matches_target = window.focused_input(cx)
                .and_then(|owner| owner.as_input().cloned())
                .is_some_and(|owner| owner.entity_id() == self.input.entity_id());
            let app = self.view.read(cx);
            let mut folders: Vec<_> = app.snapshot.as_ref().context("snapshot disappeared")?.folders.iter().map(|folder|folder.id.clone()).collect();
            folders.sort();
            let ime = match self.session.state(parent(self.native)?) {
                Ok(state) => state,
                Err(error) => json!({"error":format!("{error:#}"),
                    "focusHwnd":unsafe { GetFocus() }.0 as usize,
                    "foregroundHwnd":unsafe { GetForegroundWindow() }.0 as usize}),
            };
            Ok((json!({"stage":format!("{}:{stage}",self.name),
                "ime":ime,
                "value":self.input.read(cx).value().as_str(),"targetFocused":self.input.focus_handle(cx).is_focused(window),
                "focusedInputMatchesTarget":focused_input_matches_target,
                "editable":self.input.read(cx).is_editable(),"modalOpen":app.folders.read(cx).modal_open(),
                "folderIds":folders,"editorUnchanged":self.editor.read(cx).value()==self.original_editor,
                "commands":format!("{:?}",self.commands.borrow().as_slice()),
                "commandCount":self.commands.borrow().len()}), self.trace.drain()))
        })??;
        for event in batch.events {
            if event.message == WM_IME_STARTCOMPOSITION {
                self.composing = true;
                self.saw_end = false;
            }
            if event.message == WM_IME_ENDCOMPOSITION {
                self.composing = false;
                self.saw_end = true;
            }
            let text = String::from_utf16(&event.utf16);
            let kind = match event.kind {
                None => "message",
                Some(ImeStringKind::Composition) => {
                    if !event.utf16.is_empty() {
                        self.saw_composition = true;
                        self.composing = true;
                    } else {
                        self.composing = false;
                    }
                    "composition"
                }
                Some(ImeStringKind::Result) => {
                    if let Ok(text) = &text {
                        self.results.push(text.clone());
                    }
                    "result"
                }
            };
            evidence.native_events.push(json!({"case":self.name,"sequence":event.sequence,"hwnd":event.hwnd,
                "message":event.message,"flags":event.flags,"kind":kind,"utf16":event.utf16,"text":text.as_ref().ok(),
                "decodeError":text.as_ref().err().map(|error|error.to_string())}));
            if text.is_err() {
                evidence.error = Some("native IME string was invalid UTF-16".into());
            }
        }
        evidence.stages.push(sample.clone());
        if let Some(error) = sample["ime"]["error"].as_str() {
            evidence.error = Some(error.to_owned());
        }
        if batch.overflow {
            evidence.error = Some("native IME observation overflowed".into());
        }
        evidence.persist(self.database)?;
        ensure!(
            evidence.error.is_none(),
            "{}",
            evidence.error.as_deref().unwrap_or("IME evidence error")
        );
        Ok(sample)
    }

    fn assert_unchanged(&self, sample: &Value) -> Result<()> {
        ensure!(
            sample["targetFocused"] == true
                && sample["focusedInputMatchesTarget"] == true
                && sample["editable"] == true
                && sample["ime"]["pinyinActive"] == true,
            "folder input lost logical focus, editability, or active Pinyin profile"
        );
        ensure!(
            sample["modalOpen"] == true,
            "IME candidate key prematurely closed the folder modal"
        );
        ensure!(
            sample["commandCount"] == 0,
            "IME candidate key prematurely submitted the folder"
        );
        ensure!(
            sample["folderIds"] == json!(self.original_folders),
            "IME candidate key mutated folders"
        );
        ensure!(
            sample["editorUnchanged"] == true,
            "IME text reached the Markdown editor"
        );
        Ok(())
    }

    async fn type_pinyin(&mut self, evidence: &mut Evidence, cx: &mut AsyncApp) -> Result<()> {
        for byte in b"XIATIAN" {
            press(self.native, VIRTUAL_KEY(u16::from(*byte)))?;
            tick(cx).await;
            let sample = self.sample("typing", evidence, cx)?;
            self.assert_unchanged(&sample)?;
        }
        for _ in 0..60 {
            tick(cx).await;
            let sample = self.sample("composition", evidence, cx)?;
            self.assert_unchanged(&sample)?;
            if self.saw_composition && self.composing {
                ensure!(
                    self.results.is_empty(),
                    "IME committed text before a candidate key"
                );
                return Ok(());
            }
        }
        bail!("ordinary Pinyin keys did not produce a real nonempty GCS_COMPSTR")
    }

    async fn finish_candidate(
        &mut self,
        key: VIRTUAL_KEY,
        evidence: &mut Evidence,
        cx: &mut AsyncApp,
    ) -> Result<String> {
        self.saw_end = false;
        press(self.native, key)?;
        for _ in 0..60 {
            tick(cx).await;
            let sample = self.sample("candidate-key", evidence, cx)?;
            self.assert_unchanged(&sample)?;
            if self.saw_end && !self.composing && !self.results.is_empty() {
                let result = self.results.concat();
                ensure!(!result.is_empty(), "IME committed an empty result");
                ensure!(
                    sample["value"].as_str() == Some(result.as_str()),
                    "field differs from actual GCS_RESULTSTR"
                );
                if key == VK_SPACE {
                    ensure!(
                        result == "夏天",
                        "Space selected unexpected Pinyin candidate: {result}"
                    );
                }
                // Include deferred command delivery in the candidate-key assertion.
                for _ in 0..8 {
                    tick(cx).await;
                    let settled = self.sample("candidate-settled", evidence, cx)?;
                    self.assert_unchanged(&settled)?;
                    ensure!(
                        settled["value"].as_str() == Some(result.as_str()) && !self.composing,
                        "candidate value or composition changed after commit"
                    );
                }
                return Ok(result);
            }
        }
        bail!("candidate confirmation did not produce GCS_RESULTSTR and end composition")
    }

    async fn submit_once(
        &mut self,
        expected: &str,
        evidence: &mut Evidence,
        cx: &mut AsyncApp,
    ) -> Result<()> {
        ensure!(
            !self.composing,
            "plain Enter was attempted during composition"
        );
        let before = self.sample("before-plain-enter", evidence, cx)?;
        self.assert_unchanged(&before)?;
        press(self.native, VK_RETURN)?;
        let mut completed = false;
        for _ in 0..100 {
            tick(cx).await;
            let sample = self.sample("plain-enter", evidence, cx)?;
            if sample["commandCount"] == 1 && sample["modalOpen"] == false {
                completed = self.window.update(cx, |_, _, cx| {
                    let app = self.view.read(cx);
                    let Some(snapshot) = &app.snapshot else {
                        return false;
                    };
                    let added: Vec<_> = snapshot
                        .folders
                        .iter()
                        .filter(|folder| !self.original_folders.contains(&folder.id))
                        .collect();
                    added.len() == 1
                        && added[0].name == expected
                        && snapshot.folders.len() == self.original_folders.len() + 1
                })?;
                if completed {
                    break;
                }
            }
        }
        ensure!(
            completed,
            "plain Enter did not create exactly one matching folder"
        );
        ensure!(
            matches!(self.commands.borrow().as_slice(), [Command::CreateFolder {name,parent_id:None}] if name==expected),
            "unexpected folder commands"
        );
        press(self.native, VK_RETURN)?;
        for _ in 0..8 {
            tick(cx).await;
        }
        let sample = self.sample("second-plain-enter", evidence, cx)?;
        ensure!(
            sample["commandCount"] == 1 && sample["modalOpen"] == false,
            "plain Enter submitted the same folder twice"
        );
        ensure!(
            sample["editorUnchanged"] == true,
            "folder submit changed the editor"
        );
        Ok(())
    }
}

fn open_case<'a>(
    window: AnyWindowHandle,
    view: &'a Entity<StudyApp>,
    native: &'a Value,
    database: &'a Path,
    session: &'a ImeSession,
    name: &'a str,
    cx: &mut AsyncApp,
) -> Result<Case<'a>> {
    let parent = native["parentHwnd"]
        .as_u64()
        .context("parent HWND missing")? as usize;
    let commands = Rc::new(RefCell::new(Vec::new()));
    let observed = commands.clone();
    let (input, editor, original_editor, original_folders, subscription, trace) =
        window.update(cx, |_, window, cx| -> Result<_> {
            view.update(cx, |app, cx| {
                app.page = Page::Notes;
                app.clear_notices(cx);
                cx.notify();
            });
            let folders = view.read(cx).folders.clone();
            ensure!(
                !folders.read(cx).modal_open(),
                "another folder modal is already open"
            );
            let subscription = cx.subscribe(&folders, move |_, event: &FolderEvent, _| {
                if let FolderEvent::Command(command) = event
                    && matches!(command.as_ref(), Command::CreateFolder { .. })
                {
                    observed.borrow_mut().push(command.as_ref().clone());
                }
            });
            folders.update(cx, |panel, cx| {
                panel.set_filter(FolderFilter::All, cx);
                panel.begin_create_folder(window, cx);
            });
            window.render_frame(cx);
            ensure!(folders.read(cx).modal_open(), "folder modal did not open");
            let input = window
                .focused_input(cx)
                .and_then(|input| input.as_input().cloned())
                .context("folder input absent")?;
            ensure!(
                input.read(cx).value().is_empty(),
                "new folder input was not empty"
            );
            let editor = view.read(cx).note_content.clone();
            let body = editor.read(cx).value().to_owned();
            let mut ids: Vec<_> = view
                .read(cx)
                .snapshot
                .as_ref()
                .context("snapshot missing")?
                .folders
                .iter()
                .map(|folder| folder.id.clone())
                .collect();
            ids.sort();
            let trace = native_ime_observer::begin(parent).map_err(|error| anyhow!(error))?;
            Ok((input, editor, body, ids, subscription, trace))
        })??;
    click_input(window, &input, native, cx)?;
    Ok(Case {
        window,
        view,
        native,
        database,
        session,
        name,
        input,
        editor,
        original_editor,
        original_folders,
        commands,
        _subscription: subscription,
        trace,
        composing: false,
        saw_composition: false,
        saw_end: false,
        results: Vec::new(),
    })
}

async fn cases(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    native: &Value,
    database: &Path,
    session: &mut ImeSession,
    evidence: &mut Evidence,
    cx: &mut AsyncApp,
) -> Result<()> {
    window.update(cx, |_, _, _| session.activate(evidence))??;
    let parent = HWND(
        native["parentHwnd"]
            .as_u64()
            .context("parent HWND missing")? as usize as *mut _,
    );
    for _ in 0..60 {
        tick(cx).await;
        let state = window.update(cx, |_, _, _| session.state(parent))??;
        let active = state["pinyinActive"] == true;
        evidence
            .stages
            .push(json!({"stage":"activation-wait","ime":state}));
        evidence.persist(database)?;
        if active {
            break;
        }
    }
    for (name, key) in [
        ("space-candidate", VK_SPACE),
        ("enter-candidate", VK_RETURN),
    ] {
        let mut case = open_case(window, view, native, database, session, name, cx)?;
        for _ in 0..4 {
            tick(cx).await;
        }
        let state = case.sample("focused", evidence, cx)?;
        ensure!(
            state["targetFocused"] == true
                && state["editable"] == true
                && state["ime"]["focusHwnd"].as_u64() == Some(parent.0 as usize as u64),
            "folder input has inconsistent native/logical focus"
        );
        ensure!(
            state["ime"]["pinyinActive"] == true,
            "Microsoft Pinyin did not become the active keyboard profile"
        );
        ensure!(
            state["ime"]["chineseMode"].is_boolean(),
            "Pinyin mode compartments are unavailable or not VT_I4"
        );
        if state["ime"]["chineseMode"] == false {
            press(native, VK_SHIFT)?;
            for _ in 0..20 {
                tick(cx).await;
                if case.sample("chinese-mode-wait", evidence, cx)?["ime"]["chineseMode"] == true {
                    break;
                }
            }
        }
        let state = case.sample("ready", evidence, cx)?;
        ensure!(
            state["ime"]["chineseMode"] == true,
            "Pinyin Chinese mode could not be verified"
        );
        case.type_pinyin(evidence, cx).await?;
        let committed = case.finish_candidate(key, evidence, cx).await?;
        capture(
            parent,
            &database.with_file_name(format!("native-ime-{name}-candidate.bmp")),
        )?;
        case.submit_once(&committed, evidence, cx).await?;
        capture(
            parent,
            &database.with_file_name(format!("native-ime-{name}-submitted.bmp")),
        )?;
    }
    Ok(())
}

pub(super) async fn exercise(
    window: AnyWindowHandle,
    view: &Entity<StudyApp>,
    native: &Value,
    database: &Path,
    cx: &mut AsyncApp,
) -> Result<()> {
    ensure!(
        std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true"),
        "native IME acceptance is CI-only"
    );
    let runner =
        std::fs::canonicalize(std::env::var_os("RUNNER_TEMP").context("RUNNER_TEMP missing")?)?;
    let data = std::fs::canonicalize(database.parent().context("database parent missing")?)?;
    ensure!(
        data.starts_with(&runner) && data != runner,
        "IME fixture must be a child of runner temporary storage"
    );
    let parent = HWND(
        native["parentHwnd"]
            .as_u64()
            .context("parent HWND missing")? as usize as *mut _,
    );
    let mut evidence = Evidence::default();
    evidence
        .stages
        .push(json!({"stage":"begin","parentHwnd":parent.0 as usize,"runnerData":data}));
    evidence.persist(database)?;
    let created = window
        .update(cx, |_, _, _| ImeSession::create(parent, &mut evidence))
        .and_then(|result| result);
    let mut session = match created {
        Ok(session) => session,
        Err(error) => {
            evidence.error = Some(format!("{error:#}"));
            evidence.persist(database)?;
            let captured = capture(parent, &database.with_file_name("native-ime-failure.bmp"));
            evidence
                .stages
                .push(json!({"stage":"failure-screenshot","result":format!("{captured:?}")}));
            evidence.persist(database)?;
            return Err(error);
        }
    };
    let result = cases(
        window,
        view,
        native,
        database,
        &mut session,
        &mut evidence,
        cx,
    )
    .await;
    if let Err(error) = &result {
        evidence.error = Some(format!("{error:#}"));
    }
    evidence.persist(database)?;
    // The non-Send session and its thread identity check also permit restoration
    // if the window has disappeared while this foreground task was running.
    let restore = session.restore();
    match &restore {
        Ok(report) => evidence.stages.push(report.clone()),
        Err(error) => evidence
            .stages
            .push(json!({"stage":"restore-failed","error":format!("{error:#}")})),
    }
    if result.is_err() || restore.is_err() {
        evidence.error = Some(format!("acceptance={result:?}; restore={restore:?}"));
        evidence.persist(database)?;
        let captured = capture(parent, &database.with_file_name("native-ime-failure.bmp"));
        evidence
            .stages
            .push(json!({"stage":"failure-screenshot","result":format!("{captured:?}")}));
        evidence.persist(database)?;
    }
    result?;
    restore?;
    evidence.passed = true;
    evidence.persist(database)?;
    std::fs::write(
        database.with_file_name("native-ime-result.txt"),
        "PASS: real Microsoft Pinyin composition, Space selection, candidate Enter suppression, and exactly-once plain Enter submission.\n",
    )?;
    Ok(())
}
