//! Keychain storage for API keys, straight through the native Security
//! framework. No `security` subprocess anywhere: a key never appears in argv
//! (visible to `ps` and process-audit logs), on stderr, or in a pipe.
//!
//! Reads and deletes match by service alone, so the Settings panel can look up
//! keys other apps filed under any account; writes land under account `broom`.

use security_framework::item::{ItemClass, ItemSearchOptions, SearchResult};
use security_framework::passwords;

/// The account name Broom files its own keys under.
pub const ACCOUNT: &str = "broom";
pub const JEV_SERVICE: &str = "typesafe-ai";
pub const LAYA_SERVICE: &str = "laya-ai";

/// The stored value for a service, if there is one. Any failure reads as
/// "missing": callers branch on presence, and the value itself never surfaces.
pub fn read(service: &str) -> Option<String> {
    let mut query = ItemSearchOptions::new();
    query.class(ItemClass::generic_password()).service(service).load_data(true);
    let SearchResult::Data(bytes) = query.search().ok()?.pop()? else {
        return None;
    };
    let text = String::from_utf8(bytes).ok()?.trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Store (or replace) a service's value. The secret goes from here into the
/// Security framework call and appears nowhere else.
pub fn write(service: &str, secret: &str) -> Result<(), String> {
    passwords::set_generic_password(service, ACCOUNT, secret.as_bytes()).map_err(|e| short(&e.to_string()))
}

/// Remove every value filed under a service. Missing-or-gone is the same
/// outcome, so an item that was already absent still reports success.
pub fn delete(service: &str) -> Result<(), String> {
    let mut query = ItemSearchOptions::new();
    query.class(ItemClass::generic_password()).service(service);
    match query.delete() {
        Ok(()) | Err(_) => Ok(()),
    }
}

/// The masked display form: dots plus the last characters, nothing more.
pub fn tail(secret: &str) -> gpui::SharedString {
    let n = secret.chars().count();
    let last: String = secret.chars().skip(n.saturating_sub(3)).collect();
    format!("•••••{last}").into()
}

fn short(message: &str) -> String {
    message.chars().take(160).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SERVICE: &str = "broom-selftest";

    /// One real write→read→update→delete cycle against the login keychain,
    /// through the same calls Settings uses. The value is fake; nothing prints.
    #[test]
    fn roundtrips_through_the_keychain() {
        delete(TEST_SERVICE).unwrap();
        write(TEST_SERVICE, "sk-test-abc123xyz").unwrap();
        assert_eq!(read(TEST_SERVICE).as_deref(), Some("sk-test-abc123xyz"));
        // A second write updates in place rather than failing.
        write(TEST_SERVICE, "sk-test-second-value").unwrap();
        assert_eq!(read(TEST_SERVICE).as_deref(), Some("sk-test-second-value"));
        delete(TEST_SERVICE).unwrap();
        assert_eq!(read(TEST_SERVICE), None);
        // Deleting again is fine.
        delete(TEST_SERVICE).unwrap();
    }

    #[test]
    fn tail_shows_only_the_last_three() {
        assert_eq!(tail("sk-abcdef123456").as_ref(), "•••••456");
        assert_eq!(tail("abc").as_ref(), "•••••abc");
    }
}
