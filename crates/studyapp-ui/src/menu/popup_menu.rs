// Adapted from gpui-component 0.7.3, Copyright 2024 - 2026 Longbridge.
// Licensed under Apache-2.0; see LICENSE-APACHE in this directory.
// WW-StudyApp changes: independent appearance/density, public input adapters,
// actionable keyboard selection and native regression coverage.
use crate::menu::menu_item::MenuItemElement;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::base::actions::{Cancel, Confirm, SelectDown, SelectUp};
use gpui_kit::base::actions::{SelectLeft, SelectRight};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{ElementExt, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::component::{Side, kbd::Kbd};
use gpui_kit::{
    Action, Anchor, AnyElement, App, AppContext, Bounds, Context, DismissEvent, Edges, Entity,
    EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyBinding,
    ParentElement, Pixels, Render, Role, ScrollHandle, SharedString, StatefulInteractiveElement,
    Styled, WeakEntity, Window, anchored, deferred, div, prelude::FluentBuilder, px,
};
use gpui_kit::{ClickEvent, MouseDownEvent, Point, Subscription};
use studyapp_core::{NoticeStyle, NotificationAppearance};

use std::rc::Rc;

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type ItemRenderer = Box<dyn Fn(&mut Window, &mut App) -> AnyElement>;

const CONTEXT: &str = "StudyAppPopupMenu";

struct MenuBindings;
impl gpui_kit::Global for MenuBindings {}

pub(super) fn init(cx: &mut App) {
    if cx.has_global::<MenuBindings>() {
        return;
    }
    cx.set_global(MenuBindings);
    cx.bind_keys([
        KeyBinding::new("enter", Confirm { secondary: false }, Some(CONTEXT)),
        KeyBinding::new("escape", Cancel, Some(CONTEXT)),
        KeyBinding::new("up", SelectUp, Some(CONTEXT)),
        KeyBinding::new("down", SelectDown, Some(CONTEXT)),
        KeyBinding::new("left", SelectLeft, Some(CONTEXT)),
        KeyBinding::new("right", SelectRight, Some(CONTEXT)),
    ]);
}

/// An menu item in a popup menu.
pub enum PopupMenuItem {
    /// A menu separator item.
    Separator,
    /// A non-interactive label item.
    Label(SharedString),
    /// A standard menu item.
    Item {
        icon: Option<Icon>,
        label: SharedString,
        disabled: bool,
        checked: bool,
        is_link: bool,
        action: Option<Box<dyn Action>>,
        // For link item
        handler: Option<ClickHandler>,
    },
    /// A menu item with custom element render.
    ElementItem {
        icon: Option<Icon>,
        disabled: bool,
        checked: bool,
        action: Option<Box<dyn Action>>,
        render: ItemRenderer,
        handler: Option<ClickHandler>,
    },
    /// A submenu item that opens another popup menu.
    Submenu {
        icon: Option<Icon>,
        label: SharedString,
        disabled: bool,
        menu: Entity<PopupMenu>,
    },
}

impl FluentBuilder for PopupMenuItem {}
impl PopupMenuItem {
    /// Create a new menu item with the given label.
    #[inline]
    pub fn new(label: impl Into<SharedString>) -> Self {
        PopupMenuItem::Item {
            icon: None,
            label: label.into(),
            disabled: false,
            checked: false,
            action: None,
            is_link: false,
            handler: None,
        }
    }

    /// Create a new menu item with custom element render.
    #[inline]
    pub fn element<F, E>(builder: F) -> Self
    where
        F: Fn(&mut Window, &mut App) -> E + 'static,
        E: IntoElement,
    {
        PopupMenuItem::ElementItem {
            icon: None,
            disabled: false,
            checked: false,
            action: None,
            render: Box::new(move |window, cx| builder(window, cx).into_any_element()),
            handler: None,
        }
    }

    /// Create a new submenu item that opens another popup menu.
    #[inline]
    pub fn submenu(label: impl Into<SharedString>, menu: Entity<PopupMenu>) -> Self {
        PopupMenuItem::Submenu {
            icon: None,
            label: label.into(),
            disabled: false,
            menu,
        }
    }

    /// Create a separator menu item.
    #[inline]
    pub fn separator() -> Self {
        PopupMenuItem::Separator
    }

    /// Creates a label menu item.
    #[inline]
    pub fn label(label: impl Into<SharedString>) -> Self {
        PopupMenuItem::Label(label.into())
    }

    /// Set the icon for the menu item.
    ///
    /// Only works for [`PopupMenuItem::Item`], [`PopupMenuItem::ElementItem`] and [`PopupMenuItem::Submenu`].
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        match &mut self {
            PopupMenuItem::Item { icon: i, .. } => {
                *i = Some(icon.into());
            }
            PopupMenuItem::ElementItem { icon: i, .. } => {
                *i = Some(icon.into());
            }
            PopupMenuItem::Submenu { icon: i, .. } => {
                *i = Some(icon.into());
            }
            _ => {}
        }
        self
    }

    /// Set the action for the menu item.
    ///
    /// Only works for [`PopupMenuItem::Item`] and [`PopupMenuItem::ElementItem`].
    pub fn action(mut self, action: Box<dyn Action>) -> Self {
        match &mut self {
            PopupMenuItem::Item { action: a, .. } => {
                *a = Some(action);
            }
            PopupMenuItem::ElementItem { action: a, .. } => {
                *a = Some(action);
            }
            _ => {}
        }
        self
    }

    /// Set the disabled state for the menu item.
    ///
    /// Only works for [`PopupMenuItem::Item`], [`PopupMenuItem::ElementItem`] and [`PopupMenuItem::Submenu`].
    pub fn disabled(mut self, disabled: bool) -> Self {
        match &mut self {
            PopupMenuItem::Item { disabled: d, .. } => {
                *d = disabled;
            }
            PopupMenuItem::ElementItem { disabled: d, .. } => {
                *d = disabled;
            }
            PopupMenuItem::Submenu { disabled: d, .. } => {
                *d = disabled;
            }
            _ => {}
        }
        self
    }

    /// Set checked state for the menu item.
    ///
    /// NOTE: If `check_side` is [`Side::Left`], the icon will replace with a check icon.
    pub fn checked(mut self, checked: bool) -> Self {
        match &mut self {
            PopupMenuItem::Item { checked: c, .. } => {
                *c = checked;
            }
            PopupMenuItem::ElementItem { checked: c, .. } => {
                *c = checked;
            }
            _ => {}
        }
        self
    }

    /// Add a click handler for the menu item.
    ///
    /// Only works for [`PopupMenuItem::Item`] and [`PopupMenuItem::ElementItem`].
    pub fn on_click<F>(mut self, handler: F) -> Self
    where
        F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    {
        match &mut self {
            PopupMenuItem::Item { handler: h, .. } => {
                *h = Some(Rc::new(handler));
            }
            PopupMenuItem::ElementItem { handler: h, .. } => {
                *h = Some(Rc::new(handler));
            }
            _ => {}
        }
        self
    }

    /// Create a link menu item.
    #[inline]
    pub fn link(label: impl Into<SharedString>, href: impl Into<String>) -> Self {
        let href = href.into();
        PopupMenuItem::Item {
            icon: None,
            label: label.into(),
            disabled: false,
            checked: false,
            action: None,
            is_link: true,
            handler: Some(Rc::new(move |_, _, cx| cx.open_url(&href))),
        }
    }

    #[inline]
    fn is_clickable(&self) -> bool {
        !matches!(self, PopupMenuItem::Separator)
            && matches!(
                self,
                PopupMenuItem::Item {
                    disabled: false,
                    ..
                } | PopupMenuItem::ElementItem {
                    disabled: false,
                    ..
                } | PopupMenuItem::Submenu {
                    disabled: false,
                    ..
                }
            )
    }

    #[inline]
    fn is_separator(&self) -> bool {
        matches!(self, PopupMenuItem::Separator)
    }

    fn has_left_icon(&self, check_side: Side) -> bool {
        match self {
            PopupMenuItem::Item { icon, checked, .. } => {
                icon.is_some() || (check_side.is_left() && *checked)
            }
            PopupMenuItem::ElementItem { icon, checked, .. } => {
                icon.is_some() || (check_side.is_left() && *checked)
            }
            PopupMenuItem::Submenu { icon, .. } => icon.is_some(),
            _ => false,
        }
    }

    #[inline]
    fn is_checked(&self) -> bool {
        match self {
            PopupMenuItem::Item { checked, .. } => *checked,
            PopupMenuItem::ElementItem { checked, .. } => *checked,
            _ => false,
        }
    }

    fn a11y_label(&self) -> Option<SharedString> {
        match self {
            PopupMenuItem::Item { label, .. }
            | PopupMenuItem::Label(label)
            | PopupMenuItem::Submenu { label, .. } => Some(label.clone()),
            PopupMenuItem::Separator | PopupMenuItem::ElementItem { .. } => None,
        }
    }
}

/// Actual popup rectangles, including only the currently visible submenu chain.
/// Native child-window hosts use this to keep their document visible behind menus.
#[derive(Clone, Debug)]
pub struct PopupMenuLayout(pub Vec<Bounds<Pixels>>);

pub struct PopupMenu {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) menu_items: Vec<PopupMenuItem>,
    /// The focus handle of Entity to handle actions.
    pub(crate) action_context: Option<FocusHandle>,
    /// The focus to restore on dismiss. Unlike `action_context`, this does not
    /// change where actions are dispatched: they still bubble from the menu's
    /// own focus path (through the trigger element's ancestors).
    pub(crate) previous_focus_handle: Option<FocusHandle>,
    /// A focus handle on the trigger's dispatch path, so shortcut hints can
    /// resolve against the key contexts the menu's actions bubble through on
    /// the very frame the menu opens. GPUI looks a handle up in the previously
    /// rendered frame, and the trigger was in it when the menu was not yet.
    pub(crate) trigger_focus_handle: Option<FocusHandle>,
    selected_index: Option<usize>,
    min_width: Option<Pixels>,
    max_width: Option<Pixels>,
    max_height: Option<Pixels>,
    bounds: Bounds<Pixels>,
    child_bounds: Vec<Bounds<Pixels>>,
    child_id: Option<gpui_kit::EntityId>,
    reported_bounds: Vec<Bounds<Pixels>>,
    appearance: Option<NotificationAppearance>,
    check_side: Side,

    /// The parent menu of this menu, if this is a submenu
    parent_menu: Option<WeakEntity<Self>>,
    external_link_icon: bool,
    scroll_handle: ScrollHandle,
    // This will update on render
    submenu_anchor: (Anchor, Pixels),

    /// Paint priority for this menu layer. The top-level menu starts at 1 and
    /// each nested submenu increments it, so deeper levels are always drawn on
    /// top of shallower ones. This fixes background content (e.g. the
    /// underlying list) bleeding through multi-level submenus, which happens
    /// when nested `anchored` popovers share the same paint order.
    ///
    /// The top-level menu relies on its container (e.g. `Popover`,
    /// `ContextMenu`) to `deferred`-draw it, and each submenu is deferred once
    /// in `render_item` with `priority + 1`. Keeping a single deferred layer
    /// per level matters because GPUI caps nested deferred depth (see
    /// `prepaint_deferred_draws`).
    ///
    /// Deferring is also what lets a submenu open from a `scrollable` menu:
    /// GPUI paints a deferred draw at the window level with no inherited
    /// content mask, so the items container's `overflow_y_scroll` clip never
    /// reaches it, while the scroll offset is still baked into its anchor.
    priority: usize,

    _subscriptions: Vec<Subscription>,
}

impl PopupMenu {
    pub(crate) fn new(cx: &mut App) -> Self {
        init(cx);
        Self {
            focus_handle: cx.focus_handle(),
            action_context: None,
            previous_focus_handle: None,
            trigger_focus_handle: None,
            parent_menu: None,
            menu_items: Vec::new(),
            selected_index: None,
            min_width: None,
            max_width: None,
            max_height: None,
            check_side: Side::Left,
            bounds: Bounds::default(),
            child_bounds: Vec::new(),
            child_id: None,
            reported_bounds: Vec::new(),
            scroll_handle: ScrollHandle::default(),
            external_link_icon: true,
            appearance: None,
            submenu_anchor: (Anchor::TopLeft, Pixels::ZERO),
            priority: gpui_kit::base::POPUP_PRIORITY,
            _subscriptions: vec![],
        }
    }

    pub fn build(
        window: &mut Window,
        cx: &mut App,
        f: impl FnOnce(Self, &mut Window, &mut Context<PopupMenu>) -> Self,
    ) -> Entity<Self> {
        cx.new(|cx| f(Self::new(cx), window, cx))
    }

    /// Overrides only this menu's presentation, for inert component previews.
    pub fn set_appearance(&mut self, appearance: NotificationAppearance, cx: &mut Context<Self>) {
        if self.appearance.as_ref() != Some(&appearance) {
            self.appearance = Some(appearance);
            cx.notify();
        }
    }

    fn effective_appearance(&self) -> NotificationAppearance {
        self.appearance
            .clone()
            .unwrap_or_else(crate::theme::notification_appearance)
    }

    /// Set the focus handle of Entity to handle actions.
    ///
    /// When the menu is dismissed or before an action is triggered, the focus will be returned to this handle.
    ///
    /// Then the action will be dispatched to this handle.
    /// Submenus without their own action context use the nearest parent's
    /// explicit context for actions and shortcut hints.
    pub fn action_context(mut self, handle: FocusHandle) -> Self {
        self.action_context = Some(handle);
        self
    }

    pub fn set_action_context(
        &mut self,
        action_context: Option<FocusHandle>,
        cx: &mut Context<Self>,
    ) {
        self.action_context = action_context.clone();

        for item in &self.menu_items {
            if let PopupMenuItem::Submenu { menu, .. } = item {
                menu.update(cx, |menu, cx| {
                    menu.set_action_context(action_context.clone(), cx);
                });
            }
        }
    }

    /// Set the focus to restore when the menu is dismissed, without changing
    /// where actions are dispatched.
    pub(crate) fn set_previous_focus(
        &mut self,
        handle: Option<FocusHandle>,
        cx: &mut Context<Self>,
    ) {
        self.previous_focus_handle = handle.clone();

        for item in &self.menu_items {
            if let PopupMenuItem::Submenu { menu, .. } = item {
                menu.update(cx, |menu, cx| {
                    menu.set_previous_focus(handle.clone(), cx);
                });
            }
        }
    }

    /// Set the focus handle on the trigger's dispatch path that shortcut hints
    /// resolve against, without changing focus or where actions are dispatched.
    pub(crate) fn set_trigger_focus(
        &mut self,
        handle: Option<FocusHandle>,
        cx: &mut Context<Self>,
    ) {
        self.trigger_focus_handle = handle.clone();

        for item in &self.menu_items {
            if let PopupMenuItem::Submenu { menu, .. } = item {
                menu.update(cx, |menu, cx| {
                    menu.set_trigger_focus(handle.clone(), cx);
                });
            }
        }
    }

    /// Set min width of the popup menu, default is 120px
    pub fn min_w(mut self, width: impl Into<Pixels>) -> Self {
        self.min_width = Some(width.into());
        self
    }

    /// Set max width of the popup menu, default is 500px
    pub fn max_w(mut self, width: impl Into<Pixels>) -> Self {
        self.max_width = Some(width.into());
        self
    }

    /// Set max height of the popup menu, default is half of the window height
    pub fn max_h(mut self, height: impl Into<Pixels>) -> Self {
        self.max_height = Some(height.into());
        self
    }

    /// Compatibility with existing builders: every project menu now scrolls
    /// when it exceeds its configured or viewport-limited height.
    pub fn scrollable(self, _scrollable: bool) -> Self {
        self
    }

    /// Set the side to show check icon, default is `Side::Left`.
    pub fn check_side(mut self, side: Side) -> Self {
        self.check_side = side;
        self
    }

    /// Set the menu to show external link icon, default is true.
    pub fn external_link_icon(mut self, visible: bool) -> Self {
        self.external_link_icon = visible;
        self
    }

    /// Add Menu Item
    pub fn menu(self, label: impl Into<SharedString>, action: Box<dyn Action>) -> Self {
        self.menu_with_disabled(label, action, false)
    }

    /// Add Menu Item with enable state
    pub fn menu_with_enable(
        mut self,
        label: impl Into<SharedString>,
        action: Box<dyn Action>,
        enable: bool,
    ) -> Self {
        self.add_menu_item(label, None, action, !enable, false);
        self
    }

    /// Add Menu Item with disabled state
    pub fn menu_with_disabled(
        mut self,
        label: impl Into<SharedString>,
        action: Box<dyn Action>,
        disabled: bool,
    ) -> Self {
        self.add_menu_item(label, None, action, disabled, false);
        self
    }

    /// Add label
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.menu_items.push(PopupMenuItem::label(label.into()));
        self
    }

    /// Add Menu to open link
    pub fn link(self, label: impl Into<SharedString>, href: impl Into<String>) -> Self {
        self.link_with_disabled(label, href, false)
    }

    /// Add Menu to open link with disabled state
    pub fn link_with_disabled(
        mut self,
        label: impl Into<SharedString>,
        href: impl Into<String>,
        disabled: bool,
    ) -> Self {
        let href = href.into();
        self.menu_items
            .push(PopupMenuItem::link(label, href).disabled(disabled));
        self
    }

    /// Add Menu to open link
    pub fn link_with_icon(
        self,
        label: impl Into<SharedString>,
        icon: impl Into<Icon>,
        href: impl Into<String>,
    ) -> Self {
        self.link_with_icon_and_disabled(label, icon, href, false)
    }

    /// Add Menu to open link with icon and disabled state
    fn link_with_icon_and_disabled(
        mut self,
        label: impl Into<SharedString>,
        icon: impl Into<Icon>,
        href: impl Into<String>,
        disabled: bool,
    ) -> Self {
        let href = href.into();
        self.menu_items.push(
            PopupMenuItem::link(label, href)
                .icon(icon)
                .disabled(disabled),
        );
        self
    }

    /// Add Menu Item with Icon.
    pub fn menu_with_icon(
        self,
        label: impl Into<SharedString>,
        icon: impl Into<Icon>,
        action: Box<dyn Action>,
    ) -> Self {
        self.menu_with_icon_and_disabled(label, icon, action, false)
    }

    /// Add Menu Item with Icon and disabled state
    pub fn menu_with_icon_and_disabled(
        mut self,
        label: impl Into<SharedString>,
        icon: impl Into<Icon>,
        action: Box<dyn Action>,
        disabled: bool,
    ) -> Self {
        self.add_menu_item(label, Some(icon.into()), action, disabled, false);
        self
    }

    /// Add Menu Item with check icon
    pub fn menu_with_check(
        self,
        label: impl Into<SharedString>,
        checked: bool,
        action: Box<dyn Action>,
    ) -> Self {
        self.menu_with_check_and_disabled(label, checked, action, false)
    }

    /// Add Menu Item with check icon and disabled state
    pub fn menu_with_check_and_disabled(
        mut self,
        label: impl Into<SharedString>,
        checked: bool,
        action: Box<dyn Action>,
        disabled: bool,
    ) -> Self {
        self.add_menu_item(label, None, action, disabled, checked);
        self
    }

    /// Add Menu Item with custom element render.
    pub fn menu_element<F, E>(self, action: Box<dyn Action>, builder: F) -> Self
    where
        F: Fn(&mut Window, &mut App) -> E + 'static,
        E: IntoElement,
    {
        self.menu_element_with_check(false, action, builder)
    }

    /// Add Menu Item with custom element render with disabled state.
    pub fn menu_element_with_disabled<F, E>(
        self,
        action: Box<dyn Action>,
        disabled: bool,
        builder: F,
    ) -> Self
    where
        F: Fn(&mut Window, &mut App) -> E + 'static,
        E: IntoElement,
    {
        self.menu_element_with_check_and_disabled(false, action, disabled, builder)
    }

    /// Add Menu Item with custom element render with icon.
    pub fn menu_element_with_icon<F, E>(
        self,
        icon: impl Into<Icon>,
        action: Box<dyn Action>,
        builder: F,
    ) -> Self
    where
        F: Fn(&mut Window, &mut App) -> E + 'static,
        E: IntoElement,
    {
        self.menu_element_with_icon_and_disabled(icon, action, false, builder)
    }

    /// Add Menu Item with custom element render with check state
    pub fn menu_element_with_check<F, E>(
        self,
        checked: bool,
        action: Box<dyn Action>,
        builder: F,
    ) -> Self
    where
        F: Fn(&mut Window, &mut App) -> E + 'static,
        E: IntoElement,
    {
        self.menu_element_with_check_and_disabled(checked, action, false, builder)
    }

    /// Add Menu Item with custom element render with icon and disabled state
    fn menu_element_with_icon_and_disabled<F, E>(
        mut self,
        icon: impl Into<Icon>,
        action: Box<dyn Action>,
        disabled: bool,
        builder: F,
    ) -> Self
    where
        F: Fn(&mut Window, &mut App) -> E + 'static,
        E: IntoElement,
    {
        self.menu_items.push(
            PopupMenuItem::element(builder)
                .action(action)
                .icon(icon)
                .disabled(disabled),
        );
        self
    }

    /// Add Menu Item with custom element render with check state and disabled state
    fn menu_element_with_check_and_disabled<F, E>(
        mut self,
        checked: bool,
        action: Box<dyn Action>,
        disabled: bool,
        builder: F,
    ) -> Self
    where
        F: Fn(&mut Window, &mut App) -> E + 'static,
        E: IntoElement,
    {
        self.menu_items.push(
            PopupMenuItem::element(builder)
                .action(action)
                .checked(checked)
                .disabled(disabled),
        );
        self
    }

    /// Add a separator Menu Item
    pub fn separator(mut self) -> Self {
        if self.menu_items.is_empty() {
            return self;
        }

        if let Some(PopupMenuItem::Separator) = self.menu_items.last() {
            return self;
        }

        self.menu_items.push(PopupMenuItem::separator());
        self
    }

    /// Add a Submenu
    pub fn submenu(
        self,
        label: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    ) -> Self {
        self.submenu_with_icon(None, label, window, cx, f)
    }

    /// Add a Submenu item with icon
    pub fn submenu_with_icon(
        mut self,
        icon: Option<Icon>,
        label: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    ) -> Self {
        let submenu = PopupMenu::build(window, cx, f);
        let parent_menu = cx.entity().downgrade();
        let parent_priority = self.priority;
        submenu.update(cx, |view, _| {
            view.parent_menu = Some(parent_menu);
            view.priority = parent_priority + 1;
        });

        self.menu_items.push(
            PopupMenuItem::submenu(label, submenu).when_some(icon, |this, icon| this.icon(icon)),
        );
        self
    }

    /// Add menu item.
    pub fn item(mut self, item: impl Into<PopupMenuItem>) -> Self {
        let item: PopupMenuItem = item.into();
        self.menu_items.push(item);
        self
    }

    /// Replace all menu items by re-running a builder on this menu, keeping its
    /// identity (focus, parent menu, layer priority).
    ///
    /// For menus whose content arrives asynchronously after the menu is shown,
    /// e.g. swapping a "loading…" placeholder for the loaded items:
    ///
    /// ```ignore
    /// cx.spawn_in(window, async move |menu, cx| {
    ///     let items = fetch_items().await;
    ///     _ = menu.update_in(cx, |menu, window, cx| {
    ///         menu.rebuild(window, cx, |menu, _, _| {
    ///             items.into_iter().fold(menu, |menu, item| {
    ///                 menu.menu(item.label, Box::new(item.action))
    ///             })
    ///         });
    ///     });
    /// })
    /// .detach();
    /// ```
    pub fn rebuild(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(Self, &mut Window, &mut Context<Self>) -> Self,
    ) {
        let mut menu = std::mem::replace(self, Self::new(cx));
        menu.menu_items.clear();
        menu.selected_index = None;
        *self = f(menu, window, cx);
        cx.notify();
    }

    fn add_menu_item(
        &mut self,
        label: impl Into<SharedString>,
        icon: Option<Icon>,
        action: Box<dyn Action>,
        disabled: bool,
        checked: bool,
    ) -> &mut Self {
        self.menu_items.push(
            PopupMenuItem::new(label)
                .when_some(icon, |item, icon| item.icon(icon))
                .disabled(disabled)
                .checked(checked)
                .action(action),
        );
        self
    }

    pub(crate) fn active_submenu(&self) -> Option<Entity<PopupMenu>> {
        match self.menu_items.get(self.selected_index?)? {
            PopupMenuItem::Submenu {
                menu,
                disabled: false,
                ..
            } => Some(menu.clone()),
            _ => None,
        }
    }

    fn publish_layout(&mut self, cx: &mut Context<Self>) {
        let active_id = self.active_submenu().map(|menu| menu.entity_id());
        if active_id != self.child_id {
            self.child_id = active_id;
            self.child_bounds.clear();
        }
        let mut bounds = Vec::new();
        if self.bounds.size.width > px(0.) && self.bounds.size.height > px(0.) {
            bounds.push(self.bounds);
        }
        bounds.extend_from_slice(&self.child_bounds);
        if bounds != self.reported_bounds {
            self.reported_bounds = bounds.clone();
            cx.emit(PopupMenuLayout(bounds.clone()));
        }
        // A retained submenu may reopen at exactly the same coordinates after
        // the parent has cleared its inactive child region. Always restore that
        // active relationship; local event deduplication cannot skip propagation.
        if let Some(parent) = self.parent_menu.clone() {
            let child_id = cx.entity_id();
            let _ = parent.update(cx, |parent, cx| {
                if parent
                    .active_submenu()
                    .is_some_and(|menu| menu.entity_id() == child_id)
                    && (parent.child_id != Some(child_id) || parent.child_bounds != bounds)
                {
                    parent.child_id = Some(child_id);
                    parent.child_bounds = bounds;
                    parent.publish_layout(cx);
                }
            });
        }
    }

    pub fn is_empty(&self) -> bool {
        self.menu_items.is_empty()
    }

    fn on_click(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        window.prevent_default();
        self.selected_index = Some(ix);
        self.confirm(&Confirm { secondary: false }, window, cx);
    }

    fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.selected_index else {
            return;
        };
        match self.menu_items.get(index) {
            Some(
                PopupMenuItem::Item {
                    handler,
                    action,
                    disabled: false,
                    ..
                }
                | PopupMenuItem::ElementItem {
                    handler,
                    action,
                    disabled: false,
                    ..
                },
            ) => {
                if let Some(handler) = handler {
                    handler(&ClickEvent::default(), window, cx);
                } else if let Some(action) = action.as_ref() {
                    self.dispatch_confirm_action(action.as_ref(), window, cx);
                }
                self.dismiss(&Cancel, window, cx);
            }
            Some(PopupMenuItem::Submenu {
                disabled: false, ..
            }) => {
                self._select_submenu(window, cx);
            }
            _ => {}
        }
    }

    fn dispatch_confirm_action(
        &self,
        action: &dyn Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(context) = self.resolved_action_context(cx) {
            context.focus(window, cx);
        }

        window.dispatch_action(action.boxed_clone(), cx);
    }

    fn resolved_action_context(&self, cx: &App) -> Option<FocusHandle> {
        self.action_context.clone().or_else(|| {
            let parent = self.parent_menu.as_ref()?.upgrade()?;
            parent.read(cx).resolved_action_context(cx)
        })
    }

    fn set_selected_index(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.selected_index != Some(ix) {
            self.selected_index = Some(ix);
            self.scroll_handle.scroll_to_item(ix);
            cx.notify();
        }
    }

    fn move_selection(&mut self, backwards: bool, cx: &mut Context<Self>) {
        let next = super::next_actionable(
            self.menu_items.iter().map(PopupMenuItem::is_clickable),
            self.selected_index,
            backwards,
        );
        if let Some(index) = next {
            self.set_selected_index(index, cx);
        } else if self.selected_index.take().is_some() {
            cx.notify();
        }
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        self.move_selection(true, cx);
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        self.move_selection(false, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, window: &mut Window, cx: &mut Context<Self>) {
        let handled = if matches!(self.submenu_anchor.0, Anchor::TopLeft | Anchor::BottomLeft) {
            self._unselect_submenu(window, cx)
        } else {
            self._select_submenu(window, cx)
        };

        if self.parent_side(cx).is_left() {
            self._focus_parent_menu(window, cx);
        }

        if handled {
            return;
        }

        // For parent AppMenuBar to handle.
        if self.parent_menu.is_none() {
            cx.propagate();
        }
    }

    fn select_right(&mut self, _: &SelectRight, window: &mut Window, cx: &mut Context<Self>) {
        let handled = if matches!(self.submenu_anchor.0, Anchor::TopLeft | Anchor::BottomLeft) {
            self._select_submenu(window, cx)
        } else {
            self._unselect_submenu(window, cx)
        };

        if self.parent_side(cx).is_right() {
            self._focus_parent_menu(window, cx);
        }

        if handled {
            return;
        }

        // For parent AppMenuBar to handle.
        if self.parent_menu.is_none() {
            cx.propagate();
        }
    }

    fn _select_submenu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if let Some(active_submenu) = self.active_submenu() {
            // Focus the submenu, so that can be handle the action.
            active_submenu.update(cx, |view, cx| {
                view.selected_index = None;
                view.move_selection(false, cx);
                view.focus_handle.focus(window, cx);
            });
            cx.notify();
            return true;
        }

        false
    }

    fn _unselect_submenu(&mut self, _: &mut Window, cx: &mut Context<Self>) -> bool {
        if let Some(active_submenu) = self.active_submenu() {
            active_submenu.update(cx, |view, cx| {
                view.selected_index = None;
                cx.notify();
            });
            return true;
        }

        false
    }

    fn _focus_parent_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(parent) = self.parent_menu.as_ref() else {
            return;
        };
        let Some(parent) = parent.upgrade() else {
            return;
        };

        self.selected_index = None;
        parent.update(cx, |view, cx| {
            view.focus_handle.focus(window, cx);
            cx.notify();
        });
    }

    fn parent_side(&self, cx: &App) -> Side {
        let Some(parent) = self.parent_menu.as_ref() else {
            return Side::Left;
        };

        let Some(parent) = parent.upgrade() else {
            return Side::Left;
        };

        match parent.read(cx).submenu_anchor.0 {
            Anchor::TopLeft | Anchor::BottomLeft => Side::Left,
            Anchor::TopRight | Anchor::BottomRight => Side::Right,
            // Center anchors are not used for submenu positioning, but we must cover them.
            _ => Side::Left,
        }
    }

    /// Dismiss the menu and the entire parent chain.
    ///
    /// The submenu is closed together with its parent, same as macOS menus.
    fn dismiss(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        self.selected_index = None;
        self.reported_bounds.clear();
        self.child_bounds.clear();
        self.child_id = None;
        cx.emit(DismissEvent);

        // Focus back to the previous focused handle, unless the item's click
        // handler has already moved focus elsewhere (e.g. opened a dialog and
        // focused its input) -- stealing focus back would break that.
        let focus_moved_away =
            window.focused(cx).is_some() && !self.focus_handle.contains_focused(window, cx);
        if !focus_moved_away
            && let Some(handle) = self
                .previous_focus_handle
                .as_ref()
                .or(self.action_context.as_ref())
        {
            window.focus(handle, cx);
        }

        let Some(parent_menu) = self.parent_menu.clone() else {
            return;
        };

        // Dismiss parent menu, when this menu is dismissed
        _ = parent_menu.update(cx, |view, cx| {
            view.dismiss(&Cancel, window, cx);
        });
    }

    fn handle_dismiss(
        &mut self,
        position: &Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Do not dismiss, if click inside the parent menu
        if self
            .parent_menu
            .as_ref()
            .and_then(WeakEntity::upgrade)
            .is_some_and(|parent| parent.read(cx).bounds.contains(position))
        {
            return;
        }

        // Do not dismiss, if there have an active submenu, the click may be
        // inside the submenu, let the submenu to handle it.
        //
        // Otherwise the submenu will be dismissed before its item's `on_click`.
        if self.active_submenu().is_some() {
            return;
        }

        self.dismiss(&Cancel, window, cx);
    }

    fn on_mouse_down_out(
        &mut self,
        e: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_dismiss(&e.position, window, cx);
    }

    fn render_key_binding(
        &self,
        action: Option<Box<dyn Action>>,
        foreground: gpui_kit::Hsla,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Kbd> {
        let action = action?;

        // Each handle names a dispatch path GPUI can resolve in the previously
        // rendered frame: the explicit action target, the trigger the menu
        // opened from (already rendered when the menu first draws), the focus
        // the menu interrupted, then the menu's own path once it has been
        // rendered. A binding registered without a key context applies on
        // every path, so it is the last resort rather than the window's
        // leftover context stack, which only holds the path of whatever
        // element happened to paint last.
        let action_context = self.resolved_action_context(cx);
        [
            action_context.as_ref(),
            self.trigger_focus_handle.as_ref(),
            self.previous_focus_handle.as_ref(),
            Some(&self.focus_handle),
        ]
        .into_iter()
        .flatten()
        .find_map(|handle| Kbd::binding_for_action_in(action.as_ref(), handle, window))
        .or_else(|| Kbd::global_binding_for_action(action.as_ref(), window))
        .map(|this| {
            this.p_0()
                .flex_nowrap()
                .border_0()
                .bg(gpui_kit::transparent_white())
                .text_color(foreground)
        })
    }

    fn render_icon(
        has_icon: bool,
        checked: bool,
        icon: Option<Icon>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        if !has_icon {
            return None;
        }

        let icon = if let Some(icon) = icon {
            icon.clone()
        } else if checked {
            Icon::new(IconName::Check)
        } else {
            Icon::empty()
        };

        Some(icon.xsmall())
    }

    #[inline]
    fn max_width(&self) -> Pixels {
        self.max_width.unwrap_or(px(f32::from(
            self.effective_appearance()
                .surface(NoticeStyle::PopupMenu)
                .width,
        )))
    }

    /// Calculate the anchor corner and left offset for child submenu
    fn update_submenu_menu_anchor(&mut self, window: &Window) {
        let bounds = self.bounds;
        let max_width = self
            .max_width()
            .min((window.bounds().size.width - px(8.)).max(px(0.)));
        let (anchor, left) = if max_width + bounds.origin.x + bounds.size.width - px(8.)
            > window.bounds().size.width
        {
            (Anchor::TopRight, -px(16.))
        } else {
            (Anchor::TopLeft, bounds.size.width - px(8.))
        };

        let is_bottom_pos = bounds.origin.y + bounds.size.height > window.bounds().size.height;
        self.submenu_anchor = if is_bottom_pos {
            (anchor.other_side_along(gpui_kit::Axis::Vertical), left)
        } else {
            (anchor, left)
        };
    }

    fn render_item(
        &self,
        ix: usize,
        item: &PopupMenuItem,
        options: RenderOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> MenuItemElement {
        let has_left_icon = options.has_left_icon;
        let is_left_check = options.check_side.is_left() && item.is_checked();
        let right_check_icon = if options.check_side.is_right() && item.is_checked() {
            Some(Icon::new(IconName::Check).xsmall())
        } else {
            None
        };

        let selected = self.selected_index == Some(ix);
        const EDGE_PADDING: Pixels = px(4.);
        const INNER_PADDING: Pixels = px(8.);

        let is_submenu = matches!(item, PopupMenuItem::Submenu { .. });

        let item_height = px(options.metrics.row_height);
        let radius = options.radius;

        let this = MenuItemElement::new(ix)
            .relative()
            .text_sm()
            .py_0()
            .px(INNER_PADDING)
            .rounded(radius)
            .items_center()
            .selected(selected)
            .on_hover(cx.listener(move |this, hovered, window, cx| {
                if *hovered {
                    // Pointing elsewhere closes the open submenu; take back
                    // the focus the keyboard may have moved into it.
                    if this.selected_index != Some(ix)
                        && this.active_submenu().is_some_and(|submenu| {
                            submenu.focus_handle(cx).contains_focused(window, cx)
                        })
                    {
                        this.focus_handle.focus(window, cx);
                    }
                    this.selected_index = Some(ix);
                } else if !is_submenu
                    && this.selected_index == Some(ix)
                    // A key press ends hover under a still pointer; keep the
                    // highlight so the next arrow key moves on from this item.
                    && !window.last_input_was_keyboard()
                {
                    // TODO: Better handle the submenu unselection when hover out
                    this.selected_index = None;
                }

                cx.notify();
            }))
            .when_some(item.a11y_label(), |this, label| this.aria_label(label));

        match item {
            PopupMenuItem::Separator => this
                .h_auto()
                .p_0()
                .my_0p5()
                .mx_neg_1()
                .border_b(px(2.))
                .border_color(options.border)
                .disabled(true),
            PopupMenuItem::Label(label) => this.disabled(true).cursor_default().child(
                h_flex()
                    .cursor_default()
                    .items_center()
                    .gap_x_1()
                    .children(Self::render_icon(has_left_icon, false, None, window, cx))
                    .child(
                        div().flex_1().child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .text_ellipsis()
                                .child(label.clone()),
                        ),
                    ),
            ),
            PopupMenuItem::ElementItem {
                render,
                icon,
                disabled,
                ..
            } => this
                .when(!disabled, |this| {
                    this.on_click(
                        cx.listener(move |this, _, window, cx| this.on_click(ix, window, cx)),
                    )
                })
                .disabled(*disabled)
                .child(
                    h_flex()
                        .flex_1()
                        .min_h(item_height)
                        .items_center()
                        .gap_x_1()
                        .children(Self::render_icon(
                            has_left_icon,
                            is_left_check,
                            icon.clone(),
                            window,
                            cx,
                        ))
                        .child((render)(window, cx))
                        .children(right_check_icon.map(|icon| icon.ml_3())),
                ),
            PopupMenuItem::Item {
                icon,
                label,
                action,
                disabled,
                is_link,
                ..
            } => {
                let show_link_icon = *is_link && self.external_link_icon;
                let action = action.as_ref().map(|action| action.boxed_clone());
                let key = self.render_key_binding(action, options.foreground, window, cx);

                this.when(!disabled, |this| {
                    this.on_click(
                        cx.listener(move |this, _, window, cx| this.on_click(ix, window, cx)),
                    )
                })
                .disabled(*disabled)
                .h(item_height)
                .gap_x_1()
                .children(Self::render_icon(
                    has_left_icon,
                    is_left_check,
                    icon.clone(),
                    window,
                    cx,
                ))
                .child(
                    h_flex()
                        .w_full()
                        .gap_3()
                        .items_center()
                        .justify_between()
                        .when(!show_link_icon, |this| {
                            this.child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .text_ellipsis()
                                    .child(label.clone()),
                            )
                        })
                        .children(right_check_icon)
                        .when(show_link_icon, |this| {
                            this.child(
                                h_flex()
                                    .w_full()
                                    .justify_between()
                                    .gap_1p5()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .text_ellipsis()
                                            .child(label.clone()),
                                    )
                                    .child(
                                        Icon::new(IconName::ExternalLink)
                                            .xsmall()
                                            .text_color(options.foreground),
                                    ),
                            )
                        })
                        .children(key),
                )
            }
            PopupMenuItem::Submenu {
                icon,
                label,
                menu,
                disabled,
            } => this
                .selected(selected)
                .disabled(*disabled)
                .items_start()
                .child(
                    h_flex()
                        .min_h(item_height)
                        .size_full()
                        .items_center()
                        .gap_x_1()
                        .children(Self::render_icon(
                            has_left_icon,
                            false,
                            icon.clone(),
                            window,
                            cx,
                        ))
                        .child(
                            h_flex()
                                .flex_1()
                                .gap_2()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .text_ellipsis()
                                        .child(label.clone()),
                                )
                                .child(
                                    Icon::new(IconName::ChevronRight)
                                        .xsmall()
                                        .text_color(options.foreground),
                                ),
                        ),
                )
                .when(selected && !disabled, |this| {
                    this.child({
                        let (anchor, left) = self.submenu_anchor;
                        let is_bottom_pos =
                            matches!(anchor, Anchor::BottomLeft | Anchor::BottomRight);
                        deferred(
                            anchored()
                                .anchor(anchor)
                                .child(
                                    div()
                                        .id("submenu")
                                        .test_support()
                                        .occlude()
                                        .when(is_bottom_pos, |this| this.bottom_0())
                                        .when(!is_bottom_pos, |this| this.top_neg_1())
                                        .left(left)
                                        .child(menu.clone()),
                                )
                                .snap_to_window_with_margin(Edges::all(EDGE_PADDING)),
                        )
                        .with_priority(self.priority + 1)
                    })
                }),
        }
    }
}

impl FluentBuilder for PopupMenu {}
impl EventEmitter<DismissEvent> for PopupMenu {}
impl EventEmitter<PopupMenuLayout> for PopupMenu {}
impl Focusable for PopupMenu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[derive(Clone, Copy)]
struct RenderOptions {
    has_left_icon: bool,
    check_side: Side,
    radius: Pixels,
    metrics: super::MenuMetrics,
    foreground: gpui_kit::Hsla,
    border: gpui_kit::Hsla,
}

impl Render for PopupMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let appearance = self.effective_appearance();
        let surface = appearance.surface(NoticeStyle::PopupMenu);
        let metrics =
            super::MenuMetrics::new(appearance.layout(NoticeStyle::PopupMenu), surface.density);
        self.update_submenu_menu_anchor(window);

        // Submenus attached via the public `item()` + `PopupMenuItem::submenu()`
        // path (from contexts that only have the menu value, e.g. a table
        // delegate's `context_menu`) have no parent wired at construction time.
        // Wire them here so the dismiss chain, click-outside checks and keyboard
        // navigation treat them the same as `submenu()`-built children.
        let parent = cx.entity().downgrade();
        let parent_priority = self.priority;
        for item in &self.menu_items {
            if let PopupMenuItem::Submenu { menu, .. } = item {
                if menu.read(cx).appearance != self.appearance {
                    menu.update(cx, |menu, cx| {
                        menu.appearance = self.appearance.clone();
                        cx.notify();
                    });
                }
                if menu.read(cx).parent_menu.is_none() {
                    menu.update(cx, |menu, _| {
                        menu.parent_menu = Some(parent.clone());
                        menu.priority = parent_priority + 1;
                    });
                }
            }
        }

        let view = cx.entity().clone();
        let items_count = self.menu_items.len();

        let viewport = window.bounds().size;
        let (width, max_height) = super::menu_extent(
            f32::from(surface.width),
            f32::from(surface.max_height),
            self.min_width.map(f32::from),
            self.max_width.map(f32::from),
            self.max_height.map(f32::from),
            f32::from(viewport.width),
            f32::from(viewport.height),
        );

        let has_left_icon = self
            .menu_items
            .iter()
            .any(|item| item.has_left_icon(self.check_side));

        let options = RenderOptions {
            has_left_icon,
            check_side: self.check_side,
            radius: px(f32::from(surface.radius)).min(px(8.)),
            metrics,
            foreground: crate::interactive_surface::menu_foreground(&appearance),
            border: crate::interactive_surface::menu_border(&appearance),
        };

        crate::interactive_surface::frame(NoticeStyle::PopupMenu, &appearance)
            .flex()
            .flex_col()
            .w(px(width))
            .id("popup-menu")
            .test_support()
            .role(Role::Menu)
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::dismiss))
            .on_mouse_down_out(cx.listener(Self::on_mouse_down_out))
            .relative()
            .occlude()
            .child(
                v_flex()
                    .id("items")
                    .p(px(metrics.padding))
                    .gap_y(px(metrics.gap))
                    .w_full()
                    .min_w_0()
                    .max_h(px(
                        (max_height - f32::from(surface.border_width) * 2.).max(0.)
                    ))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
                    .children(
                        self.menu_items
                            .iter()
                            .enumerate()
                            // Ignore last separator
                            .filter(|(ix, item)| !(*ix + 1 == items_count && item.is_separator()))
                            .map(|(ix, item)| self.render_item(ix, item, options, window, cx)),
                    ),
            )
            .vertical_scrollbar(&self.scroll_handle)
            .on_prepaint(move |bounds, _, cx| {
                view.update(cx, |r, cx| {
                    r.bounds = crate::interactive_surface::outer_bounds(
                        bounds,
                        px(f32::from(surface.border_width)),
                    );
                    r.publish_layout(cx);
                })
            })
    }
}

#[cfg(all(test, feature = "ui-test"))]
mod tests {
    use super::{PopupMenu, PopupMenuItem};
    use gpui_kit::{AppContext, Bounds, div, px};

    #[gpui_kit::test]
    fn retained_submenu_reopening_restores_parent_occlusion(cx: &mut gpui_kit::TestAppContext) {
        let parent = cx.update(|cx| cx.new(|cx| PopupMenu::new(cx)));
        let child = cx.update(|cx| cx.new(|cx| PopupMenu::new(cx)));
        let parent_bounds = Bounds::new(
            gpui_kit::point(px(20.), px(20.)),
            gpui_kit::size(px(180.), px(80.)),
        );
        let child_bounds = Bounds::new(
            gpui_kit::point(px(200.), px(20.)),
            gpui_kit::size(px(180.), px(80.)),
        );
        parent.update(cx, |menu, cx| {
            menu.bounds = parent_bounds;
            menu.menu_items = vec![PopupMenuItem::submenu("Move", child.clone())];
            menu.selected_index = Some(0);
            menu.publish_layout(cx);
        });
        child.update(cx, |menu, cx| {
            menu.bounds = child_bounds;
            menu.parent_menu = Some(parent.downgrade());
            menu.publish_layout(cx);
        });
        parent.update(cx, |menu, cx| {
            assert_eq!(menu.reported_bounds, vec![parent_bounds, child_bounds]);
            menu.selected_index = None;
            menu.publish_layout(cx);
            assert_eq!(menu.reported_bounds, vec![parent_bounds]);
            menu.selected_index = Some(0);
            menu.publish_layout(cx);
        });
        child.update(cx, |menu, cx| menu.publish_layout(cx));
        parent.update(cx, |menu, _| {
            assert_eq!(menu.reported_bounds, vec![parent_bounds, child_bounds])
        });
    }

    #[gpui_kit::test]
    fn popup_menu_item_a11y_label_uses_visible_label(cx: &mut gpui_kit::TestAppContext) {
        let submenu = cx.update(|cx| cx.new(|cx| PopupMenu::new(cx)));

        assert_eq!(PopupMenuItem::new("Open").a11y_label(), Some("Open".into()));
        assert_eq!(
            PopupMenuItem::link("Docs", "https://example.com").a11y_label(),
            Some("Docs".into())
        );
        assert_eq!(
            PopupMenuItem::label("Recent files").a11y_label(),
            Some("Recent files".into())
        );
        assert_eq!(
            PopupMenuItem::submenu("More", submenu).a11y_label(),
            Some("More".into())
        );
        assert_eq!(PopupMenuItem::separator().a11y_label(), None);
        assert_eq!(PopupMenuItem::element(|_, _| div()).a11y_label(), None);
    }

    struct MenuFixture(gpui_kit::Entity<PopupMenu>);
    impl gpui_kit::Render for MenuFixture {
        fn render(
            &mut self,
            _: &mut gpui_kit::Window,
            _: &mut gpui_kit::Context<Self>,
        ) -> impl gpui_kit::IntoElement {
            use gpui_kit::{ParentElement, Styled};
            div().size_full().child(self.0.clone())
        }
    }

    fn draw(cx: &mut gpui_kit::VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    #[gpui_kit::test]
    fn actual_menu_frame_uses_appearance_density_and_reports_its_outer_bounds(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::test::TestWindowExt;
        use studyapp_core::{NoticeLayout, NoticeStyle, NotificationAppearance};
        cx.update(gpui_kit::init);
        let mut appearance = NotificationAppearance::default();
        let patch = appearance.surface_patch_mut(NoticeStyle::PopupMenu);
        patch.width = Some(280);
        patch.border_width = Some(4);
        patch.radius = Some(24);
        patch.background = Some("#182638CC".into());
        patch.border_color = Some("#C45E25".into());
        patch.max_height = Some(240);
        let (root, visual) = cx.add_window_view(|window, cx| {
            MenuFixture(PopupMenu::build(window, cx, |menu, _, cx| {
                let mut menu = menu
                    .item(PopupMenuItem::new("One").disabled(true))
                    .item(PopupMenuItem::new("Two").disabled(true))
                    .item(PopupMenuItem::new("Three").disabled(true));
                menu.set_appearance(appearance.clone(), cx);
                menu
            }))
        });
        draw(visual);
        let menu = visual.update(|_, cx| root.read(cx).0.clone());
        let standard = visual.update(|window, cx| {
            let frame = window.find("popup-menu").bounds();
            assert_eq!(frame.size.width, px(280.));
            assert_eq!(menu.read(cx).reported_bounds, vec![frame]);
            frame
        });
        appearance
            .layouts
            .insert(NoticeStyle::PopupMenu, NoticeLayout::Alternative);
        menu.update(visual, |menu, cx| {
            menu.set_appearance(appearance.clone(), cx)
        });
        draw(visual);
        visual.update(|window, cx| {
            let frame = window.find("popup-menu").bounds();
            assert_eq!(frame.size.width, standard.size.width);
            assert!(frame.size.height < standard.size.height);
            assert_eq!(menu.read(cx).reported_bounds, vec![frame]);
        });
        appearance
            .surface_patch_mut(NoticeStyle::PopupMenu)
            .max_height = Some(48);
        menu.update(visual, |menu, cx| menu.set_appearance(appearance, cx));
        draw(visual);
        visual
            .update(|window, _| assert!(window.find("popup-menu").bounds().size.height <= px(48.)));
    }

    #[gpui_kit::test]
    fn enter_confirms_first_destination_and_dismisses_both_submenu_directions(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::{prelude::FluentBuilder, test::TestWindowExt, *};
        use std::{cell::Cell, rc::Rc};
        struct EdgeMenu {
            menu: Entity<PopupMenu>,
            left_opening: bool,
        }
        impl Render for EdgeMenu {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
                    .size_full()
                    .flex()
                    .items_start()
                    .when(self.left_opening, |row| row.justify_end())
                    .child(self.menu.clone())
            }
        }
        cx.update(gpui_kit::init);
        for left_opening in [false, true] {
            let calls = Rc::new(Cell::new(0));
            let dismissals = Rc::new(Cell::new(0));
            let (root, visual) = cx.add_window_view(|window, cx| EdgeMenu {
                left_opening,
                menu: PopupMenu::build(window, cx, |menu, window, cx| {
                    let dismissals = dismissals.clone();
                    cx.subscribe_self(move |_, _: &DismissEvent, _| {
                        dismissals.set(dismissals.get() + 1);
                    })
                    .detach();
                    let calls = calls.clone();
                    menu.submenu("Move", window, cx, move |menu, _, _| {
                        let calls = calls.clone();
                        menu.item(PopupMenuItem::new("Unclassified").on_click(move |_, _, _| {
                            calls.set(calls.get() + 1);
                        }))
                    })
                }),
            });
            draw(visual);
            let menu = visual.update(|_, cx| root.read(cx).menu.clone());
            visual.update(|window, cx| {
                assert_eq!(
                    matches!(
                        menu.read(cx).submenu_anchor.0,
                        Anchor::TopRight | Anchor::BottomRight
                    ),
                    left_opening,
                );
                menu.read(cx).focus_handle(cx).focus(window, cx);
                window.press("down", cx);
            });
            draw(visual);
            visual.update(|window, cx| window.press("enter", cx));
            draw(visual);
            visual.update(|window, cx| {
                let child = menu
                    .read(cx)
                    .active_submenu()
                    .expect("selected Move submenu");
                assert!(child.read(cx).focus_handle(cx).is_focused(window));
                assert_eq!(child.read(cx).selected_index, Some(0));
                window.press("enter", cx);
            });
            visual.run_until_parked();
            assert_eq!(calls.get(), 1);
            assert_eq!(dismissals.get(), 1);
            assert_eq!(menu.read_with(visual, |menu, _| menu.selected_index), None);
        }
    }

    #[gpui_kit::test]
    fn keyboard_skips_noncommands_enters_submenus_and_cannot_confirm_a_disabled_row(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::test::TestWindowExt;
        use gpui_kit::{Focusable, base::actions::Confirm};
        use std::{cell::Cell, rc::Rc};
        cx.update(gpui_kit::init);
        let calls = Rc::new(Cell::new(0));
        let (root, visual) = cx.add_window_view(|window, cx| {
            MenuFixture(PopupMenu::build(window, cx, |menu, window, cx| {
                let calls = calls.clone();
                menu.item(PopupMenuItem::label("Commands"))
                    .item(
                        PopupMenuItem::new("Unavailable")
                            .disabled(true)
                            .on_click(move |_, _, _| calls.set(calls.get() + 1)),
                    )
                    .separator()
                    .submenu("Move", window, cx, |menu, _, _| {
                        menu.item(PopupMenuItem::label("Destinations"))
                            .item(PopupMenuItem::new("Folder"))
                    })
            }))
        });
        draw(visual);
        let menu = visual.update(|_, cx| root.read(cx).0.clone());
        visual.update(|window, cx| {
            menu.update(cx, |menu, cx| {
                menu.selected_index = Some(1);
                menu.confirm(&Confirm { secondary: false }, window, cx);
                menu.selected_index = None;
                menu.focus_handle(cx).focus(window, cx);
            });
            assert_eq!(calls.get(), 0);
            window.press("down", cx);
        });
        draw(visual);
        visual.update(|window, cx| {
            assert_eq!(menu.read(cx).selected_index, Some(3));
            window.press("enter", cx);
        });
        draw(visual);
        visual.update(|window, cx| {
            let child = menu.read(cx).active_submenu().expect("submenu opens");
            assert!(child.read(cx).focus_handle(cx).is_focused(window));
            assert_eq!(child.read(cx).selected_index, Some(1));
            window.press("escape", cx);
        });
    }
}
