//! Against the real Secret Service of the session (KWallet, GNOME Keyring).
//! Ignored by default: CI has no keyring. Run with `--ignored` on a desktop.

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

use ikeloud_keyring::Keyring;

#[test]
#[ignore = "needs a desktop session with a Secret Service"]
fn store_load_replace_delete() {
    let keyring = Keyring::open_kind("test").expect("Secret Service");
    keyring.delete().unwrap();
    assert_eq!(keyring.load().unwrap(), None);
    keyring.store(b"{\"a\":1}").unwrap();
    assert_eq!(keyring.load().unwrap().as_deref(), Some(&b"{\"a\":1}"[..]));
    keyring.store(b"{\"a\":2}").unwrap();
    assert_eq!(
        keyring.load().unwrap().as_deref(),
        Some(&b"{\"a\":2}"[..]),
        "replaced"
    );
    keyring.delete().unwrap();
    assert_eq!(keyring.load().unwrap(), None);
}

#[test]
#[ignore = "needs a desktop session with a Secret Service"]
fn a_session_file_moves_into_the_keyring() {
    use ikeloud_keyring::SessionStore;
    let dir = std::env::temp_dir().join(format!("ikeloud-keyring-{}", std::process::id()));
    let paths = ikeloud_config::Paths::new(&dir.join("c"), &dir.join("s"), &dir.join("d"));
    let keyring = Keyring::open_kind("test-store").expect("Secret Service");
    keyring.delete().unwrap();

    // A file from before the keyring: loaded, moved into the keyring.
    ikeloud_config::write_private(&paths.session, b"{\"old\":1}").unwrap();
    let store = SessionStore::with_kind(&paths, "test-store");
    assert!(store.uses_keyring());
    assert_eq!(store.load().as_deref(), Some("{\"old\":1}"));
    assert!(!paths.session.exists(), "the file moved into the keyring");
    assert_eq!(
        keyring.load().unwrap().as_deref(),
        Some(&b"{\"old\":1}"[..])
    );

    // Saved and loaded through the keyring; no file comes back.
    store.save("{\"new\":2}");
    assert!(!paths.session.exists());
    let again = SessionStore::with_kind(&paths, "test-store");
    assert_eq!(again.load().as_deref(), Some("{\"new\":2}"));

    // Sign out: gone everywhere.
    again.delete();
    assert_eq!(keyring.load().unwrap(), None);
    std::fs::remove_dir_all(dir).ok();
}
