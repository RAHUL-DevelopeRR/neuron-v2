//! Provider API keys the user stored outside the environment (the CLI loads
//! them from the OS keychain, `neuron auth set <provider>`) and hands to this
//! crate in memory.
//!
//! Lookup order for a credential variable such as `ANTHROPIC_API_KEY`:
//!   1. the process environment,
//!   2. the `.env` file of a trusted workspace,
//!   3. a key installed here.
//!
//! Installed keys are never exported into the process environment, so tool
//! subprocesses do not inherit them.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{OnceLock, PoisonError, RwLock};

/// A stored key, redacted in `Debug` output.
#[derive(Clone, PartialEq, Eq)]
struct StoredKey(String);

impl fmt::Debug for StoredKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

fn slot() -> &'static RwLock<BTreeMap<String, StoredKey>> {
    static SLOT: OnceLock<RwLock<BTreeMap<String, StoredKey>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(BTreeMap::new()))
}

/// Installs `key` as the stored value of credential variable `env_name`
/// (for example `ANTHROPIC_API_KEY`). An empty key removes it.
pub fn install_user_key(env_name: &str, key: &str) {
    let mut keys = slot().write().unwrap_or_else(PoisonError::into_inner);
    if key.trim().is_empty() {
        keys.remove(env_name);
    } else {
        keys.insert(env_name.to_string(), StoredKey(key.trim().to_string()));
    }
}

/// Removes every installed key.
pub fn clear_user_keys() {
    slot()
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
}

/// The key installed for `env_name`, if any.
#[must_use]
pub fn user_key(env_name: &str) -> Option<String> {
    slot()
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(env_name)
        .map(|key| key.0.clone())
}

/// Whether a key is installed for `env_name`.
#[must_use]
pub fn has_user_key(env_name: &str) -> bool {
    slot()
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .contains_key(env_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_keys_are_trimmed_redacted_and_removable() {
        let name = "NEURON_TEST_USER_KEY_A";
        install_user_key(name, "  sk-test-123  ");
        assert_eq!(user_key(name).as_deref(), Some("sk-test-123"));
        assert!(has_user_key(name));
        let debug = format!("{:?}", slot().read().unwrap().get(name));
        assert!(!debug.contains("sk-test"), "{debug}");
        install_user_key(name, "");
        assert_eq!(user_key(name), None);
    }
}
