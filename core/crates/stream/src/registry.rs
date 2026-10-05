use std::collections::HashMap;

use meridian_types::SecurityKey;
use parking_lot::RwLock;

/// Process-local handle for an instrument. Never persisted.
pub type InstrumentId = u32;

/// Interns [`SecurityKey`]s into dense [`InstrumentId`]s.
#[derive(Debug, Default)]
pub struct InstrumentRegistry {
    inner: RwLock<RegistryInner>,
}

#[derive(Debug, Default)]
struct RegistryInner {
    by_key: HashMap<SecurityKey, InstrumentId>,
    keys: Vec<SecurityKey>,
}

impl InstrumentRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the existing ID or assigns the next one.
    pub fn intern(&self, key: &SecurityKey) -> InstrumentId {
        if let Some(id) = self.inner.read().by_key.get(key) {
            return *id;
        }
        let mut w = self.inner.write();
        if let Some(id) = w.by_key.get(key) {
            return *id;
        }
        let id = InstrumentId::try_from(w.keys.len()).unwrap_or(InstrumentId::MAX);
        w.keys.push(key.clone());
        w.by_key.insert(key.clone(), id);
        id
    }

    #[must_use]
    pub fn get(&self, key: &SecurityKey) -> Option<InstrumentId> {
        self.inner.read().by_key.get(key).copied()
    }

    #[must_use]
    pub fn key(&self, id: InstrumentId) -> Option<SecurityKey> {
        self.inner.read().keys.get(id as usize).cloned()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.read().keys.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_is_stable() {
        let r = InstrumentRegistry::new();
        let a = r.intern(&SecurityKey::equity("AAPL"));
        let b = r.intern(&SecurityKey::equity("MSFT"));
        assert_ne!(a, b);
        assert_eq!(r.intern(&SecurityKey::equity("AAPL")), a);
        assert_eq!(r.key(b), Some(SecurityKey::equity("MSFT")));
    }
}
