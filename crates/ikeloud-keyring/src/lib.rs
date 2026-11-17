//! The signed-in session (cookies, trust token; never the password) in the
//! desktop keyring, over the Secret Service D-Bus API (KWallet's ksecretd on
//! Plasma, GNOME Keyring on GNOME).
//!
//! One item, found by its attributes. The secret travels in the "plain"
//! session of the protocol: on the user's own session bus, between two
//! processes of the same user. When there is no keyring, or it is locked
//! and would need a prompt, the session stays in the private file
//! (`ikeloud_config::Paths::session`, mode 0600) as before. A file left
//! from before the keyring is moved into it on the first load.

use std::collections::HashMap;

use gio::prelude::*;
use glib::Variant;

const SERVICE: &str = "org.freedesktop.secrets";
const PATH: &str = "/org/freedesktop/secrets";
const IFACE: &str = "org.freedesktop.Secret.Service";
const DEFAULT: &str = "/org/freedesktop/secrets/aliases/default";
const TIMEOUT_MS: i32 = 10_000;

/// Attributes of our item of `kind` ("session"; tests use their own) for
/// one profile (its session file): two profiles of one user, e.g. a test
/// setup next to the installed app, never share a session.
fn attributes(kind: &str, profile: &str) -> HashMap<String, String> {
    HashMap::from([
        (
            "application".to_string(),
            "io.github.brbla.IkeLoud".to_string(),
        ),
        ("type".to_string(), kind.to_string()),
        ("profile".to_string(), profile.to_string()),
    ])
}

/// Why the keyring could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyringError {
    /// No Secret Service, or it failed.
    Unavailable(String),
    /// The keyring is locked and would need the user's password.
    Locked,
}

impl std::fmt::Display for KeyringError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(e) => write!(f, "keyring unavailable: {e}"),
            Self::Locked => write!(f, "keyring locked"),
        }
    }
}

impl std::error::Error for KeyringError {}

impl From<glib::Error> for KeyringError {
    fn from(e: glib::Error) -> Self {
        Self::Unavailable(e.to_string())
    }
}

/// A connection to the Secret Service with an open session.
pub struct Keyring {
    bus: gio::DBusConnection,
    session: Variant,
    kind: &'static str,
    profile: String,
}

impl std::fmt::Debug for Keyring {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keyring").finish_non_exhaustive()
    }
}

impl Keyring {
    /// Connects and opens a session, for the signed-in session item.
    pub fn open(profile: &str) -> Result<Self, KeyringError> {
        Self::open_kind("session", profile)
    }

    /// Connects for the item of `kind` (tests use their own kind).
    pub fn open_kind(kind: &'static str, profile: &str) -> Result<Self, KeyringError> {
        let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)?;
        let reply = call(
            &bus,
            PATH,
            IFACE,
            "OpenSession",
            &("plain", Variant::from_variant(&"".to_variant())).to_variant(),
        )?;
        let session = reply.child_value(1);
        Ok(Self {
            bus,
            session,
            kind,
            profile: profile.to_string(),
        })
    }

    fn search(&self) -> Result<(Vec<Variant>, Vec<Variant>), KeyringError> {
        self.search_with(attributes(self.kind, &self.profile))
    }

    fn search_with(
        &self,
        attributes: HashMap<String, String>,
    ) -> Result<(Vec<Variant>, Vec<Variant>), KeyringError> {
        let reply = call(
            &self.bus,
            PATH,
            IFACE,
            "SearchItems",
            &(attributes,).to_variant(),
        )?;
        let paths = |i: usize| -> Vec<Variant> {
            let list = reply.child_value(i);
            (0..list.n_children())
                .map(|n| list.child_value(n))
                .collect()
        };
        Ok((paths(0), paths(1)))
    }

    /// The stored session, if any.
    pub fn load(&self) -> Result<Option<Vec<u8>>, KeyringError> {
        let (unlocked, locked) = self.search()?;
        if unlocked.is_empty() && locked.is_empty() {
            return self.take_legacy();
        }
        self.read(&unlocked, &locked)
    }

    /// An item stored before items had a profile (one release, on the
    /// owner's machine): moved under this profile, the old one deleted.
    fn take_legacy(&self) -> Result<Option<Vec<u8>>, KeyringError> {
        let mut legacy = attributes(self.kind, "");
        legacy.remove("profile");
        let (unlocked, locked) = self.search_with(legacy)?;
        let without_profile = |items: &[Variant]| -> Vec<Variant> {
            items
                .iter()
                .filter(|item| !self.has_profile(item))
                .cloned()
                .collect()
        };
        let (unlocked, locked) = (without_profile(&unlocked), without_profile(&locked));
        let Some(secret) = self.read(&unlocked, &locked)? else {
            return Ok(None);
        };
        self.store(&secret)?;
        for item in unlocked.iter().chain(locked.iter()) {
            if let Some(path) = item.str() {
                let _ = call(
                    &self.bus,
                    path,
                    "org.freedesktop.Secret.Item",
                    "Delete",
                    &().to_variant(),
                );
            }
        }
        Ok(Some(secret))
    }

    fn has_profile(&self, item: &Variant) -> bool {
        let Some(path) = item.str() else { return true };
        let reply = call(
            &self.bus,
            path,
            "org.freedesktop.DBus.Properties",
            "Get",
            &("org.freedesktop.Secret.Item", "Attributes").to_variant(),
        );
        reply
            .ok()
            .and_then(|r| r.child_value(0).as_variant())
            .and_then(|v| v.get::<HashMap<String, String>>())
            .is_none_or(|a| a.contains_key("profile"))
    }

    fn read(
        &self,
        unlocked: &[Variant],
        locked: &[Variant],
    ) -> Result<Option<Vec<u8>>, KeyringError> {
        let item = match (unlocked.first(), locked.first()) {
            (Some(item), _) => item.clone(),
            (None, Some(item)) => {
                self.unlock(item)?;
                item.clone()
            }
            (None, None) => return Ok(None),
        };
        let items =
            Variant::array_from_iter_with_type(glib::VariantTy::OBJECT_PATH, [item.clone()]);
        let reply = call(
            &self.bus,
            PATH,
            IFACE,
            "GetSecrets",
            &Variant::tuple_from_iter([items, self.session.clone()]),
        )?;
        // a{o(oayays)}: the secret's value is the third field.
        let secrets = reply.child_value(0);
        if secrets.n_children() == 0 {
            return Ok(None);
        }
        let secret = secrets.child_value(0).child_value(1);
        Ok(secret.child_value(2).get::<Vec<u8>>())
    }

    /// Stores (replaces) the session.
    pub fn store(&self, secret: &[u8]) -> Result<(), KeyringError> {
        self.store_with(attributes(self.kind, &self.profile), secret)
    }

    /// Stores an item the way the first keyring release did (no profile).
    /// For the takeover test only.
    #[doc(hidden)]
    pub fn store_without_profile_for_tests(&self, secret: &[u8]) -> Result<(), KeyringError> {
        let mut legacy = attributes(self.kind, "");
        legacy.remove("profile");
        self.store_with(legacy, secret)
    }

    fn store_with(
        &self,
        item_attributes: HashMap<String, String>,
        secret: &[u8],
    ) -> Result<(), KeyringError> {
        let collection = call(
            &self.bus,
            PATH,
            IFACE,
            "ReadAlias",
            &("default",).to_variant(),
        )?
        .child_value(0);
        let collection_path = collection.str().unwrap_or(DEFAULT).to_string();
        let collection_path = if collection_path == "/" {
            DEFAULT.to_string()
        } else {
            collection_path
        };
        let properties: HashMap<String, Variant> = HashMap::from([
            (
                "org.freedesktop.Secret.Item.Label".to_string(),
                "ike loud: iCloud session".to_variant(),
            ),
            (
                "org.freedesktop.Secret.Item.Attributes".to_string(),
                item_attributes.to_variant(),
            ),
        ]);
        let secret = Variant::tuple_from_iter([
            self.session.clone(),
            Vec::<u8>::new().to_variant(),
            secret.to_vec().to_variant(),
            "application/json".to_variant(),
        ]);
        let reply = call(
            &self.bus,
            &collection_path,
            "org.freedesktop.Secret.Collection",
            "CreateItem",
            &Variant::tuple_from_iter([properties.to_variant(), secret, true.to_variant()]),
        );
        match reply {
            Ok(r) if r.child_value(1).str() == Some("/") => Ok(()),
            // A prompt: the collection is locked.
            Ok(_) => Err(KeyringError::Locked),
            Err(e) => Err(e),
        }
    }

    /// Removes the session.
    pub fn delete(&self) -> Result<(), KeyringError> {
        let (unlocked, locked) = self.search()?;
        for item in unlocked.iter().chain(locked.iter()) {
            if let Some(path) = item.str() {
                call(
                    &self.bus,
                    path,
                    "org.freedesktop.Secret.Item",
                    "Delete",
                    &().to_variant(),
                )?;
            }
        }
        Ok(())
    }

    fn unlock(&self, item: &Variant) -> Result<(), KeyringError> {
        let items =
            Variant::array_from_iter_with_type(glib::VariantTy::OBJECT_PATH, [item.clone()]);
        let reply = call(
            &self.bus,
            PATH,
            IFACE,
            "Unlock",
            &Variant::tuple_from_iter([items]),
        )?;
        // Unlocked at once, or only after a prompt (not shown by a daemon).
        if reply.child_value(0).n_children() > 0 {
            Ok(())
        } else {
            Err(KeyringError::Locked)
        }
    }
}

fn call(
    bus: &gio::DBusConnection,
    path: &str,
    iface: &str,
    method: &str,
    params: &Variant,
) -> Result<Variant, KeyringError> {
    Ok(bus.call_sync(
        Some(SERVICE),
        path,
        iface,
        method,
        Some(params),
        None,
        gio::DBusCallFlags::NONE,
        TIMEOUT_MS,
        gio::Cancellable::NONE,
    )?)
}

/// Where the session lives: the keyring when it works, else the file.
#[derive(Debug)]
pub struct SessionStore {
    keyring: Option<Keyring>,
    file: std::path::PathBuf,
    /// The last value written, to skip writes that change nothing.
    last: std::cell::RefCell<Option<String>>,
}

impl SessionStore {
    /// The store for `paths`.
    pub fn new(paths: &ikeloud_config::Paths) -> Self {
        Self::with_kind(paths, "session")
    }

    /// The store with the keyring item of `kind` (tests use their own).
    pub fn with_kind(paths: &ikeloud_config::Paths, kind: &'static str) -> Self {
        Self {
            keyring: Keyring::open_kind(kind, &paths.session.to_string_lossy()).ok(),
            file: paths.session.clone(),
            last: std::cell::RefCell::new(None),
        }
    }

    /// Whether the keyring is used (else the file).
    pub fn uses_keyring(&self) -> bool {
        self.keyring.is_some()
    }

    /// The saved session (JSON). A session file wins over the keyring: it
    /// is newer (left by a version before the keyring, or written while the
    /// keyring was away), and moves into the keyring now.
    pub fn load(&self) -> Option<String> {
        if let Ok(text) = std::fs::read_to_string(&self.file) {
            if let Some(keyring) = &self.keyring
                && keyring.store(text.as_bytes()).is_ok()
            {
                let _ = std::fs::remove_file(&self.file);
            }
            *self.last.borrow_mut() = Some(text.clone());
            return Some(text);
        }
        let text = String::from_utf8(self.keyring.as_ref()?.load().ok()??).ok()?;
        *self.last.borrow_mut() = Some(text.clone());
        Some(text)
    }

    /// Saves the session; does nothing when it did not change.
    pub fn save(&self, json: &str) {
        if self.last.borrow().as_deref() == Some(json) {
            return;
        }
        let stored = self
            .keyring
            .as_ref()
            .is_some_and(|k| k.store(json.as_bytes()).is_ok());
        if stored {
            let _ = std::fs::remove_file(&self.file);
        } else {
            let _ = ikeloud_config::write_private(&self.file, json.as_bytes());
        }
        *self.last.borrow_mut() = Some(json.to_string());
    }

    /// Removes the session everywhere (sign out).
    pub fn delete(&self) {
        if let Some(keyring) = &self.keyring {
            let _ = keyring.delete();
        }
        let _ = std::fs::remove_file(&self.file);
        *self.last.borrow_mut() = None;
    }
}
