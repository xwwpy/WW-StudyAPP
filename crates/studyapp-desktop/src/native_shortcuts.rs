//! Configured editing keys for the focused GPUI input. WebView commands have a
//! separate dispatcher; this module never sends document-editor IPC.
use gpui_kit::{Action, App, Window, base::input, component::WindowExt};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NativeEdit {
    Undo,
    Redo,
    SelectAll,
    DeletePreviousWord,
    DeleteNextWord,
    Indent,
    Outdent,
    AddCursorAbove,
    AddCursorBelow,
}

impl NativeEdit {
    fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "editor.undo" => Self::Undo,
            "editor.redo" => Self::Redo,
            "editor.select-all" => Self::SelectAll,
            "editor.delete-group-backward" => Self::DeletePreviousWord,
            "editor.delete-group-forward" => Self::DeleteNextWord,
            "editor.indent-more" => Self::Indent,
            "editor.indent-less" => Self::Outdent,
            "editor.add-cursor-above" => Self::AddCursorAbove,
            "editor.add-cursor-below" => Self::AddCursorBelow,
            _ => return None,
        })
    }

    fn action(self) -> Box<dyn Action> {
        match self {
            Self::Undo => Box::new(input::Undo),
            Self::Redo => Box::new(input::Redo),
            Self::SelectAll => Box::new(input::SelectAll),
            Self::DeletePreviousWord => Box::new(input::DeleteToPreviousWordStart),
            Self::DeleteNextWord => Box::new(input::DeleteToNextWordEnd),
            Self::Indent => Box::new(input::Indent),
            Self::Outdent => Box::new(input::Outdent),
            Self::AddCursorAbove => Box::new(input::AddCursorAbove),
            Self::AddCursorBelow => Box::new(input::AddCursorBelow),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Resolution {
    Dispatch(NativeEdit),
    SuppressDefault,
    PassThrough,
}

fn matches_binding(binding: &str, combination: &str) -> bool {
    binding
        .split(';')
        .filter(|alias| !alias.is_empty())
        .any(|alias| {
            crate::shortcuts::normalize_binding(alias).is_ok_and(|alias| alias == combination)
        })
}

fn resolve(combination: &str, bindings: &BTreeMap<String, String>) -> Resolution {
    let Ok(combination) = crate::shortcuts::normalize_binding(combination) else {
        return Resolution::PassThrough;
    };
    // Resolve current assignments before suppressing old defaults, so reusing
    // Ctrl+Z for an application command does not get swallowed as retired Undo.
    for action in crate::shortcuts::SHORTCUT_ACTIONS {
        let binding = bindings
            .get(action.id)
            .map(String::as_str)
            .unwrap_or(action.default);
        if matches_binding(binding, &combination) {
            return NativeEdit::from_id(action.id)
                .map(Resolution::Dispatch)
                .unwrap_or(Resolution::PassThrough);
        }
    }
    for action in crate::shortcuts::SHORTCUT_ACTIONS {
        if NativeEdit::from_id(action.id).is_some() && matches_binding(action.default, &combination)
        {
            return Resolution::SuppressDefault;
        }
    }
    Resolution::PassThrough
}

/// Returns true when a focused native input owns this key, including a retired
/// default binding that must be suppressed. The caller must stop propagation
/// from its `intercept_keystrokes` callback, before GPUI resolves default keys.
///
/// The caller also preserves CharacterInput/AltGr and platform-modifier events;
/// their intent cannot be reconstructed from this normalized key string alone.
pub fn handle_native_edit(
    combination: &str,
    bindings: &BTreeMap<String, String>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let resolution = resolve(combination, bindings);
    if resolution == Resolution::PassThrough {
        return false;
    }
    // WindowExt validates the registry against actual keyboard focus, and drops
    // stale input registrations. In a modal, this is the modal's own input.
    let Some(input) = window.focused_input(cx) else {
        return false;
    };
    if !input.focus_handle(cx).is_focused(window) {
        return false;
    }
    if let Resolution::Dispatch(command) = resolution {
        // GPUI captures the current focus ID before deferring this dispatch.
        // This avoids reentering StudyApp during its keystroke interception.
        window.dispatch_action(command.action(), cx);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebinding_and_unbinding_retire_native_default_aliases() {
        let mut bindings = crate::shortcuts::default_bindings();
        bindings.insert("editor.undo".into(), "Ctrl+Shift+U".into());
        assert_eq!(resolve("Ctrl+Z", &bindings), Resolution::SuppressDefault);
        assert_eq!(
            resolve("Ctrl+Shift+U", &bindings),
            Resolution::Dispatch(NativeEdit::Undo)
        );
        bindings.insert("editor.undo".into(), String::new());
        assert_eq!(resolve("Ctrl+Z", &bindings), Resolution::SuppressDefault);
        assert_eq!(resolve("Ctrl+Shift+U", &bindings), Resolution::PassThrough);
        bindings.insert("editor.redo".into(), String::new());
        assert_eq!(resolve("Ctrl+Y", &bindings), Resolution::SuppressDefault);
        assert_eq!(
            resolve("Ctrl+Shift+Z", &bindings),
            Resolution::SuppressDefault
        );
    }

    #[test]
    fn current_assignment_wins_over_a_retired_default() {
        let mut bindings = crate::shortcuts::default_bindings();
        bindings.insert("editor.undo".into(), String::new());
        bindings.insert("editor.redo".into(), "Ctrl+Z".into());
        assert_eq!(
            resolve("Ctrl+Z", &bindings),
            Resolution::Dispatch(NativeEdit::Redo)
        );
        bindings.insert("editor.redo".into(), String::new());
        bindings.insert("app.new".into(), "Ctrl+Z".into());
        assert_eq!(resolve("Ctrl+Z", &bindings), Resolution::PassThrough);
    }

    #[test]
    fn unconfigured_native_actions_use_defaults_and_navigation_passes_through() {
        let bindings = BTreeMap::new();
        assert_eq!(
            resolve("Ctrl+A", &bindings),
            Resolution::Dispatch(NativeEdit::SelectAll)
        );
        assert_eq!(
            resolve("Ctrl+Backspace", &bindings),
            Resolution::Dispatch(NativeEdit::DeletePreviousWord)
        );
        assert_eq!(
            resolve("Ctrl+BracketLeft", &bindings),
            Resolution::Dispatch(NativeEdit::Outdent)
        );
        assert_eq!(
            resolve("Ctrl+Alt+ArrowUp", &bindings),
            Resolution::Dispatch(NativeEdit::AddCursorAbove)
        );
        for key in [
            "a",
            "ArrowLeft",
            "Shift+ArrowLeft",
            "Ctrl+ArrowLeft",
            "Home",
            "Ctrl+Home",
            "Ctrl+C",
            "Ctrl+V",
            "Alt+F4",
            "Ctrl+Enter",
            "Ctrl+B",
        ] {
            assert_eq!(resolve(key, &bindings), Resolution::PassThrough, "{key}");
        }
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod ui_tests {
    use super::*;
    use gpui_kit::{
        AppContext, Context, Entity, Focusable, InputPreference, IntoElement, ParentElement,
        Render, Styled, TestAppContext, VisualTestContext,
        component::{
            Root,
            input::{Input, InputState},
        },
    };
    use std::{cell::RefCell, rc::Rc};

    struct Inputs {
        first: Entity<InputState>,
        second: Entity<InputState>,
    }

    impl Render for Inputs {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            gpui_kit::div()
                .flex()
                .flex_col()
                .child(Input::new(&self.first))
                .child(Input::new(&self.second))
        }
    }

    fn fixture(
        cx: &mut TestAppContext,
        bindings: Rc<RefCell<BTreeMap<String, String>>>,
    ) -> (Entity<InputState>, Entity<InputState>, VisualTestContext) {
        cx.update(gpui_kit::init);
        cx.update(|cx| {
            cx.intercept_keystrokes(move |event, window, cx| {
                if event.input_preference == InputPreference::CharacterInput
                    || event.keystroke.modifiers.platform
                {
                    return;
                }
                let modifiers = event.keystroke.modifiers;
                let combination = format!(
                    "{}{}{}{}",
                    if modifiers.control { "Ctrl+" } else { "" },
                    if modifiers.alt { "Alt+" } else { "" },
                    if modifiers.shift { "Shift+" } else { "" },
                    event.keystroke.key
                );
                if handle_native_edit(&combination, &bindings.borrow(), window, cx) {
                    cx.stop_propagation();
                }
            })
            .detach();
        });
        let mut first = None;
        let mut second = None;
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |window, cx| {
                let first_state = cx.new(|cx| InputState::new(window, cx).default_value("first"));
                let second_state = cx.new(|cx| InputState::new(window, cx).default_value("second"));
                first = Some(first_state.clone());
                second = Some(second_state.clone());
                let content = cx.new(|_| Inputs {
                    first: first_state,
                    second: second_state,
                });
                cx.new(|cx| Root::new(content, window, cx))
            })
            .unwrap()
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        draw(&mut visual);
        (first.unwrap(), second.unwrap(), visual)
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn focus(input: &Entity<InputState>, cx: &mut VisualTestContext) {
        cx.update(|window, cx| input.focus_handle(cx).focus(window, cx));
        draw(cx);
    }

    #[gpui_kit::test]
    fn native_input_undo_respects_remap_and_empty_binding(cx: &mut TestAppContext) {
        let mut configured = crate::shortcuts::default_bindings();
        configured.insert("editor.undo".into(), "Ctrl+Shift+U".into());
        let bindings = Rc::new(RefCell::new(configured));
        let (first, _, mut cx) = fixture(cx, bindings.clone());
        focus(&first, &mut cx);
        cx.update(|window, cx| {
            first.update(cx, |input, cx| {
                let end = input.value().len();
                input.set_selected_range(end..end, cx);
                input.insert("!", window, cx);
            })
        });
        draw(&mut cx);
        assert_eq!(first.read_with(&cx, |input, _| input.value()), "first!");
        cx.simulate_keystrokes("ctrl-z");
        draw(&mut cx);
        assert_eq!(
            first.read_with(&cx, |input, _| input.value()),
            "first!",
            "Old Ctrl+Z must be suppressed before GPUI keybinding dispatch"
        );
        cx.simulate_keystrokes("ctrl-shift-u");
        draw(&mut cx);
        assert_eq!(first.read_with(&cx, |input, _| input.value()), "first");
        cx.update(|window, cx| {
            first.update(cx, |input, cx| {
                let end = input.value().len();
                input.set_selected_range(end..end, cx);
                input.insert("?", window, cx);
            })
        });
        draw(&mut cx);
        bindings
            .borrow_mut()
            .insert("editor.undo".into(), String::new());
        cx.simulate_keystrokes("ctrl-z ctrl-shift-u");
        draw(&mut cx);
        assert_eq!(first.read_with(&cx, |input, _| input.value()), "first?");
    }

    #[gpui_kit::test]
    fn select_all_targets_the_focused_input_and_explicit_unbind_blocks_it(cx: &mut TestAppContext) {
        let bindings = Rc::new(RefCell::new(crate::shortcuts::default_bindings()));
        let (first, second, mut cx) = fixture(cx, bindings.clone());
        focus(&first, &mut cx);
        let first_range = first.read_with(&cx, |input, _| input.selected_range());
        focus(&second, &mut cx);
        cx.simulate_keystrokes("ctrl-a");
        draw(&mut cx);
        assert_eq!(
            second.read_with(&cx, |input, _| input.selected_range()),
            0..6
        );
        assert_eq!(
            first.read_with(&cx, |input, _| input.selected_range()),
            first_range
        );
        bindings
            .borrow_mut()
            .insert("editor.select-all".into(), String::new());
        cx.simulate_keystrokes("right ctrl-a");
        draw(&mut cx);
        assert_eq!(
            second.read_with(&cx, |input, _| input.selected_range()),
            6..6
        );
        cx.update(|window, cx| {
            window.blur(cx);
            assert!(!handle_native_edit(
                "Ctrl+Z",
                &bindings.borrow(),
                window,
                cx
            ));
        });
    }
}
