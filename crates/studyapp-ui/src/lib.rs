//! Shared visual system and reusable GPUI controls. No application IO or business workers.
pub mod bubble;
pub mod compact_markdown;
pub mod components;
pub mod controls;
pub mod design;
pub mod feedback;
pub mod file_integrity;
pub mod markdown_content;
pub mod markdown_format;
pub mod markdown_view;
pub mod modal_frame;
pub mod notice_queue;
pub mod notification_anchor;
pub mod notification_card;
pub mod notifications;
pub mod release_notes;
pub mod settings_color;
pub mod settings_color_value;
pub mod settings_components;
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
