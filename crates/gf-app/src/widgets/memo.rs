//! A one-entry cache for values a view derives from its data.
//!
//! iced rebuilds the whole view after every message — a key press, a
//! scroll, each spinner tick while git runs. Values that walk a whole list
//! (the widest commit summary, the rows of a long diff) are computed once
//! per data change instead of once per frame.

use std::cell::RefCell;

/// Remembers the value computed for the last key.
#[derive(Debug)]
pub struct Memo<K, V>(RefCell<Option<(K, V)>>);

impl<K, V> Default for Memo<K, V> {
    fn default() -> Self {
        Self(RefCell::new(None))
    }
}

/// A copy starts empty (it is only a cache).
impl<K, V> Clone for Memo<K, V> {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl<K: PartialEq, V: Clone> Memo<K, V> {
    /// The value for `key`, computed by `f` only when the key changed.
    pub fn get(&self, key: K, f: impl FnOnce() -> V) -> V {
        if let Some((k, v)) = self.0.borrow().as_ref()
            && *k == key
        {
            return v.clone();
        }
        let v = f();
        *self.0.borrow_mut() = Some((key, v.clone()));
        v
    }
}

/// A shared value as a memo key, compared by identity. Holding the `Arc`
/// keeps the value alive, so a new one can never get its address: use it
/// when a stale value would be wrong, not only look off.
#[derive(Debug)]
pub struct Same<T>(pub std::sync::Arc<T>);

impl<T> PartialEq for Same<T> {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

/// Identity of borrowed data for a memo key: its address and length. The
/// length guards against a new value allocated where a dropped one was,
/// but not fully: only for values whose staleness is cosmetic (a width).
pub type Id = (usize, usize);

/// The [`Id`] of `data`.
pub fn identity<T>(data: &T, len: usize) -> Id {
    (std::ptr::from_ref(data) as usize, len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_value_is_never_mistaken_for_the_old_one() {
        use std::sync::Arc;
        let m: Memo<Same<Vec<u8>>, usize> = Memo::default();
        let first = Arc::new(vec![1, 2, 3]);
        assert_eq!(m.get(Same(first.clone()), || first.len()), 3);
        drop(first);
        // Same contents and length, maybe the same address: still new.
        let second = Arc::new(vec![9]);
        assert_eq!(m.get(Same(second.clone()), || second.len()), 1);
        assert!(Same(second.clone()) == Same(second.clone()));
        assert!(Same(Arc::new(1)) != Same(Arc::new(1)));
    }

    #[test]
    fn computes_once_per_key() {
        let m: Memo<u32, u32> = Memo::default();
        let mut runs = 0;
        let mut get = |k| {
            m.get(k, || {
                runs += 1;
                k * 2
            })
        };
        assert_eq!(get(1), 2);
        assert_eq!(get(1), 2);
        assert_eq!(get(2), 4);
        assert_eq!(runs, 2);
    }
}
