//! Availability policy independent of the UI and background service.

pub fn media_blocks_storage(read_only_scan: bool, media_active: bool) -> bool {
    !read_only_scan && media_active
}

#[cfg(test)]
mod tests {
    use super::media_blocks_storage;

    #[test]
    fn read_only_space_scan_is_available_during_preview_resolution() {
        assert!(!media_blocks_storage(true, true));
        assert!(!media_blocks_storage(true, false));
    }

    #[test]
    fn mutations_remain_blocked_while_media_owns_library_files() {
        assert!(media_blocks_storage(false, true));
        assert!(!media_blocks_storage(false, false));
    }
}
