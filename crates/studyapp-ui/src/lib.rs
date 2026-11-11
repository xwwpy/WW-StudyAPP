//! Shared visual system and reusable GPUI controls. No application IO or business workers.
pub mod action_icons;
pub mod activity_view;
pub mod bubble;
pub mod compact_markdown;
pub mod components;
pub mod controls;
pub mod design;
pub mod feedback;
pub mod file_integrity;
pub mod file_issues;
pub mod interactive_surface;
pub mod markdown_content;
pub mod markdown_format;
mod markdown_parser;
pub mod markdown_view;
pub mod menu;
pub mod modal_frame;
mod notice_placement;
pub mod notice_preview;
pub mod notice_queue;
pub mod notification_anchor;
pub mod notification_card;
pub mod notification_surfaces;
pub mod notifications;
pub mod release_notes;
pub mod settings_anchor;
pub mod settings_color;
pub mod settings_color_value;
pub mod settings_components;
pub mod settings_secret;
pub mod settings_switch;
pub mod settings_time;
pub mod theme;
pub mod timer_card;
pub mod transfer_progress;
pub mod update_available;
pub mod update_dialog;
pub mod virtual_list_scrollbar;
pub mod window_chrome;

#[cfg(all(test, feature = "ui-test"))]
mod modal_selection_regression;
#[cfg(all(test, feature = "ui-test"))]
mod scroll_regression;

mod native_html;
mod native_markdown_model;
