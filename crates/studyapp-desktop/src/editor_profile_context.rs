//! One foreground-thread context owns each live persistent browser profile.
//! Weak entries never keep a closed profile open across application relocation.
use std::{
    cell::RefCell,
    collections::BTreeMap,
    path::PathBuf,
    rc::{Rc, Weak},
};

pub(super) struct ProfileContexts<T> {
    entries: BTreeMap<PathBuf, Weak<RefCell<T>>>,
}

impl<T> Default for ProfileContexts<T> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }
}

impl<T> ProfileContexts<T> {
    // The native caller supplies a canonical, already-created profile directory.
    pub(super) fn acquire(
        &mut self,
        profile: PathBuf,
        create: impl FnOnce() -> T,
    ) -> Rc<RefCell<T>> {
        self.entries.retain(|_, owner| owner.strong_count() != 0);
        if let Some(owner) = self.entries.get(&profile).and_then(Weak::upgrade) {
            return owner;
        }
        let owner = Rc::new(RefCell::new(create()));
        self.entries.insert(profile, Rc::downgrade(&owner));
        owner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn simultaneous_editors_share_one_profile_owner_and_its_storage() {
        let mut contexts = ProfileContexts::default();
        let profile = PathBuf::from("editor");
        let reader = contexts.acquire(profile.clone(), BTreeMap::new);
        let preview =
            contexts.acquire(profile, || panic!("must not create a second profile owner"));
        assert!(Rc::ptr_eq(&reader, &preview));
        reader.borrow_mut().insert("marker", "persisted");
        assert_eq!(preview.borrow().get("marker"), Some(&"persisted"));
        drop(preview);
        assert_eq!(reader.borrow().get("marker"), Some(&"persisted"));
    }

    #[test]
    fn last_host_releases_context_before_a_new_profile_is_acquired() {
        struct Owner(Rc<Cell<usize>>);
        impl Drop for Owner {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let drops = Rc::new(Cell::new(0));
        let mut contexts = ProfileContexts::default();
        let a = contexts.acquire("source".into(), || Owner(drops.clone()));
        let b = contexts.acquire("source".into(), || panic!("duplicate"));
        let old = Rc::downgrade(&a);
        drop(a);
        assert_eq!(drops.get(), 0);
        drop(b);
        assert_eq!(drops.get(), 1);
        assert!(
            old.upgrade().is_none(),
            "the registry cannot retain a closed WebKit process"
        );
        let target = contexts.acquire("target".into(), || Owner(drops.clone()));
        assert_eq!(
            contexts.entries.len(),
            1,
            "expired profile entries are pruned"
        );
        drop(target);
        assert_eq!(drops.get(), 2);
    }

    #[test]
    fn different_profiles_are_isolated_and_returning_to_a_closed_path_is_fresh() {
        let mut contexts = ProfileContexts::default();
        let source = contexts.acquire("source".into(), || "source");
        let target = contexts.acquire("target".into(), || "target");
        assert!(!Rc::ptr_eq(&source, &target));
        assert_eq!(*target.borrow(), "target");
        let old = Rc::downgrade(&source);
        drop(source);
        let reopened = contexts.acquire("source".into(), || "reopened");
        assert!(old.upgrade().is_none());
        assert_eq!(*reopened.borrow(), "reopened");
        assert_eq!(*target.borrow(), "target");
    }
}
