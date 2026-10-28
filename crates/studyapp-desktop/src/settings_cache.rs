//! Derived settings data is rebuilt only after its source input changes.
use std::{cell::RefCell, rc::Rc};

pub struct InputCache<K, T> {
    value: RefCell<Option<(K, Rc<T>)>>,
    rebuilds: std::cell::Cell<u64>,
}

impl<K, T> Default for InputCache<K, T> {
    fn default() -> Self {
        Self {
            value: RefCell::new(None),
            rebuilds: std::cell::Cell::new(0),
        }
    }
}

impl<K: PartialEq, T> InputCache<K, T> {
    pub fn get(&self, input: K, build: impl FnOnce(&K) -> T) -> Rc<T> {
        if let Some((cached_input, value)) = self.value.borrow().as_ref()
            && *cached_input == input
        {
            return value.clone();
        }
        let value = Rc::new(build(&input));
        self.rebuilds.set(self.rebuilds.get() + 1);
        *self.value.borrow_mut() = Some((input, value.clone()));
        value
    }

    pub fn rebuilds(&self) -> u64 {
        self.rebuilds.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_frames_share_one_analysis_and_next_edit_rebuilds_once() {
        let cache = InputCache::default();
        let first = cache.get("Ctrl+N", |_| "Ctrl+N".to_owned());
        for _ in 0..120 {
            let next = cache.get("Ctrl+N", |_| "unnecessary parse".to_owned());
            assert!(
                Rc::ptr_eq(&first, &next),
                "unchanged frame rebuilt analysis"
            );
        }
        assert_eq!(cache.rebuilds(), 1);
        let changed = cache.get("Ctrl+S", |_| "Ctrl+S".to_owned());
        assert_eq!(&*changed, "Ctrl+S");
        assert!(!Rc::ptr_eq(&first, &changed));
        assert_eq!(cache.rebuilds(), 2);
    }

    #[test]
    fn equal_owned_fields_reuse_preview_but_layout_or_text_changes_invalidate() {
        let cache = InputCache::default();
        let first = cache.get((vec!["#234567".to_owned()], 0), |_| 1);
        let same = cache.get((vec!["#234567".to_owned()], 0), |_| 2);
        assert!(Rc::ptr_eq(&first, &same));
        assert_eq!(cache.rebuilds(), 1);
        assert_eq!(*cache.get((vec!["#234567".to_owned()], 1), |_| 3), 3);
        assert_eq!(*cache.get((vec!["#abcdef".to_owned()], 1), |_| 4), 4);
        assert_eq!(cache.rebuilds(), 3);
    }
}
