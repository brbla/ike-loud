//! Translation tests: completeness, plural forms, fallback, new languages.

#![allow(clippy::expect_used, clippy::panic)] // test code may panic

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ikeloud_i18n::{BUILTIN, FormatError, LoadError, Message, ToMessage, Translations, Value};
use ikeloud_safety::names::NameError;
use ikeloud_safety::{Action, Context, DeferReason, ReadOnlyReason, Rejection, RelPath, assess};

fn lang(code: &str) -> Translations {
    Translations::new(&[code.to_string()]).expect("built-in translations load")
}

/// Message keys of an .ftl file (lines `key = …` at the start of a line).
fn keys(text: &str) -> BTreeSet<String> {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(key, _)| key.trim_end())
        .filter(|k| {
            k.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                && k.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
        .map(ToString::to_string)
        .collect()
}

/// One message for every variant the user can see.
fn all_messages() -> Vec<Message> {
    let c = Context::new(200, ikeloud_sync::names::key);
    let Err(mass) = assess(
        (0..120)
            .map(|i| Action::TrashLocal {
                path: RelPath::new(format!("f{i}")),
                expected: ikeloud_testkit::version(i, "h"),
            })
            .collect(),
        &c,
    ) else {
        panic!("expected the brake");
    };

    let read_only = [
        ReadOnlyReason::Settings,
        ReadOnlyReason::FirstDownload,
        ReadOnlyReason::UnknownApiResponse,
        ReadOnlyReason::RemoteKillSwitch,
    ];
    let mut messages: Vec<Message> = vec![
        Message::new("app-name"),
        Message::new("app-description"),
        Rejection::LocalFolderUnavailable.message(),
        Rejection::SuspiciousRemoteListing.message(),
        Rejection::Conflict(vec![RelPath::new("a")]).message(),
        mass.message(),
        Rejection::NotEnoughICloudSpace {
            needed: 2_000_000,
            free: 1_000,
        }
        .message(),
        Rejection::NotEnoughDiskSpace {
            needed: 2_000_000,
            free: 1_000,
        }
        .message(),
        DeferReason::StillBeingWritten.message(),
        DeferReason::Ignored.message(),
        DeferReason::CaseCollision {
            with: RelPath::new("a/Photo.jpg"),
        }
        .message(),
    ];
    for error in [
        NameError::EmptyComponent,
        NameError::DotOrDotDot,
        NameError::ForbiddenChar('\n'),
        NameError::TooLong,
    ] {
        messages.push(DeferReason::InvalidName(error).message());
    }
    for r in read_only {
        messages.push(r.message());
        messages.push(DeferReason::ReadOnly(r).message());
    }
    for key in ["size-bytes", "size-kb", "size-mb", "size-gb", "size-tb"] {
        messages.push(Message::new(key).arg("value", Value::Text("1".into())));
    }
    messages.push(Message::new("decimal-separator"));
    for number in [1, 2] {
        messages.push(
            Message::new("conflict-copy-name")
                .arg("stem", Value::Text("a".into()))
                .arg("extension", Value::Text(".txt".into()))
                .arg("date", Value::Text("2026-11-15".into()))
                .arg("number", Value::Number(number)),
        );
    }
    messages
}

#[test]
fn all_languages_have_the_same_keys() {
    let reference = keys(
        BUILTIN
            .iter()
            .find(|(k, _)| *k == "en")
            .map(|(_, t)| *t)
            .expect("English is built in"),
    );
    assert!(!reference.is_empty());
    for (code, text) in BUILTIN {
        let k = keys(text);
        let missing: Vec<_> = reference.difference(&k).collect();
        let extra: Vec<_> = k.difference(&reference).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "{code}: missing {missing:?}, extra {extra:?}"
        );
    }
}

#[test]
fn every_message_is_translated() {
    let messages = all_messages();
    let used: BTreeSet<String> = messages.iter().map(|m| m.key.to_string()).collect();
    for (code, text) in BUILTIN {
        let t = lang(code);
        assert_eq!(t.languages().first().map(String::as_str), Some(*code));
        for m in &messages {
            let s = t.text_exact(m);
            assert!(
                s.as_ref()
                    .is_ok_and(|s| !s.trim().is_empty() && !s.contains('{')),
                "{code}: {}: {s:?}",
                m.key
            );
        }
        // No dead keys in the file.
        let unused: Vec<_> = keys(text).difference(&used).cloned().collect();
        assert!(unused.is_empty(), "{code}: unused keys {unused:?}");
    }
}

#[test]
fn czech_plural_forms() {
    let cs = lang("cs");
    let conflict = |n: usize| {
        let paths = (0..n).map(|i| RelPath::new(format!("f{i}"))).collect();
        cs.text(&Rejection::Conflict(paths).message())
    };
    let cases: &[(usize, &str)] = &[
        (1, "Jeden soubor se mezitím změnil"),
        (2, "2 soubory se mezitím změnily"),
        (4, "4 soubory se mezitím změnily"),
        (5, "5 souborů se mezitím změnilo"),
        (22, "22 souborů se mezitím změnilo"),
    ];
    for (n, start) in cases {
        let s = conflict(*n);
        assert!(s.starts_with(start), "{n}: {s}");
    }
}

#[test]
fn english_plural_forms() {
    let en = lang("en");
    let conflict = |n: usize| {
        let paths = (0..n).map(|i| RelPath::new(format!("f{i}"))).collect();
        en.text(&Rejection::Conflict(paths).message())
    };
    assert!(conflict(1).starts_with("One file was"), "{}", conflict(1));
    assert!(conflict(3).starts_with("3 files were"), "{}", conflict(3));
}

#[test]
fn mass_change_question() {
    let question = |code: &str, changes: u64, moves: u64| {
        lang(code).text(
            &Message::new("rejected-mass-change")
                .arg("changes", Value::Number(changes))
                .arg("moves", Value::Number(moves))
                .arg("total", Value::Number(200)),
        )
    };
    let cases: &[(&str, u64, u64, &str)] = &[
        (
            "cs",
            120,
            0,
            "Synchronizace by během dne smazala nebo přepsala 120 souborů z 200. \
             Než bude pokračovat, zkontrolujte prosím seznam souborů a potvrďte, že je to v pořádku.",
        ),
        (
            "cs",
            0,
            3,
            "Synchronizace by během dne přesunula 3 soubory z 200. \
             Než bude pokračovat, zkontrolujte prosím seznam souborů a potvrďte, že je to v pořádku.",
        ),
        (
            "cs",
            1,
            101,
            "Synchronizace by během dne smazala nebo přepsala jeden soubor z 200. \
             Synchronizace by během dne přesunula 101 souborů z 200. \
             Než bude pokračovat, zkontrolujte prosím seznam souborů a potvrďte, že je to v pořádku.",
        ),
        (
            "en",
            51,
            0,
            "Syncing would delete or overwrite 51 files out of 200 within a day. \
             Please check the list of files and confirm before it continues.",
        ),
        (
            "en",
            0,
            1,
            "Syncing would move one file out of 200 within a day. \
             Please check the list of files and confirm before it continues.",
        ),
    ];
    for (code, changes, moves, expected) in cases {
        assert_eq!(
            question(code, *changes, *moves),
            *expected,
            "{code} {changes} {moves}"
        );
    }
}

#[test]
fn sizes_per_language() {
    let m = Rejection::NotEnoughICloudSpace {
        needed: 1_500_000_000,
        free: 250_000,
    }
    .message();
    let cases: &[(&str, &str)] = &[
        ("cs", "potřeba 1,5 GB, volno 250 kB"),
        ("en", "1.5 GB needed, 250 kB free"),
    ];
    for (code, expected) in cases {
        let s = lang(code).text(&m);
        assert!(s.contains(expected), "{code}: {s}");
    }
}

#[test]
fn nested_message_in_the_same_language() {
    let m = DeferReason::ReadOnly(ReadOnlyReason::FirstDownload).message();
    assert_eq!(
        lang("cs").text(&m),
        "Zatím se jen stahuje. Zápis na iCloud se zapne, až se dokončí a zkontroluje první stažení."
    );
    assert_eq!(
        lang("en").text(&m),
        "Only downloading for now. Writing to iCloud turns on once the first download has finished and been checked."
    );
}

#[test]
fn collision_shows_only_the_name() {
    let m = DeferReason::CaseCollision {
        with: RelPath::new("Documents/Příloha.pdf"),
    }
    .message();
    assert!(
        lang("cs")
            .text(&m)
            .starts_with("Na iCloudu už je „Příloha.pdf“.")
    );
}

#[test]
fn choosing_language_and_fallback() {
    let l = |requested: &[&str]| {
        Translations::new(
            &requested
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        )
        .expect("load")
        .languages()
    };
    assert_eq!(l(&["cs-CZ"]), ["cs", "en"]);
    assert_eq!(l(&["ja"]), ["en"]);
    assert_eq!(l(&[]), ["en"]);
}

#[test]
fn unknown_key_returns_the_key() {
    let m = Message::new("this-key-does-not-exist");
    assert_eq!(lang("cs").text(&m), "this-key-does-not-exist");
    assert_eq!(
        lang("cs").text_exact(&m),
        Err(FormatError::MissingKey("this-key-does-not-exist"))
    );
}

#[test]
fn missing_argument_is_an_error() {
    let m = Message::new("rejected-conflict");
    assert!(matches!(
        lang("cs").text_exact(&m),
        Err(FormatError::Format {
            key: "rejected-conflict",
            ..
        })
    ));
}

// --- languages from a folder ---------------------------------------------------------

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("ikeloud-i18n-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("temp dir");
        Self(path)
    }

    fn language(&self, code: &str, text: &str) -> &Self {
        let dir = self.0.join(code);
        std::fs::create_dir_all(&dir).expect("language dir");
        std::fs::write(dir.join("ikeloud.ftl"), text).expect("translation file");
        self
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn new_language_from_a_folder_with_english_fallback() {
    let d = TempDir::new("new");
    d.language(
        "de",
        "app-description = Inoffizieller Client für iCloud Drive\n",
    );
    let t = Translations::with_dir(&["de-AT".into()], d.path()).expect("load");
    assert_eq!(t.languages(), ["de", "en"]);
    assert_eq!(
        t.text(&Message::new("app-description")),
        "Inoffizieller Client für iCloud Drive"
    );
    // Missing text comes from English.
    assert_eq!(
        t.text(&DeferReason::StillBeingWritten.message()),
        "The file is still being saved. It will be uploaded once it is done."
    );
}

#[test]
fn folder_replaces_a_builtin_translation() {
    let d = TempDir::new("replace");
    d.language("cs", "app-description = Upravený popis\n");
    let t = Translations::with_dir(&["cs".into()], d.path()).expect("load");
    assert_eq!(t.text(&Message::new("app-description")), "Upravený popis");
}

#[test]
fn errors_in_the_folder() {
    let d = TempDir::new("errors");
    d.language("de", "app-description = {\n");
    assert!(matches!(
        Translations::with_dir(&["de".into()], d.path()),
        Err(LoadError::Syntax { ref language, .. }) if language == "de"
    ));
    // A broken language nobody asked for does not stop the app.
    assert_eq!(
        Translations::with_dir(&["cs".into()], d.path()).map(|t| t.languages()),
        Ok(vec!["cs".to_string(), "en".to_string()])
    );

    let d = TempDir::new("code");
    d.language("not_a_language!", "a = b\n");
    assert_eq!(
        Translations::with_dir(&[], d.path()).map(|t| t.languages()),
        Err(LoadError::InvalidLanguageCode("not_a_language!".into()))
    );

    let missing = std::env::temp_dir().join("ikeloud-i18n-does-not-exist-xyz");
    assert!(matches!(
        Translations::with_dir(&[], &missing),
        Err(LoadError::Read { .. })
    ));

    // A folder without a translation file is skipped.
    let d = TempDir::new("empty");
    std::fs::create_dir_all(d.path().join("fr")).expect("dir");
    assert_eq!(
        Translations::with_dir(&["fr".into()], d.path()).map(|t| t.languages()),
        Ok(vec!["en".to_string()])
    );
}

// --- conflict copy names -----------------------------------------------------------------

#[test]
fn conflict_copy_name() {
    let cases: &[(&str, &str, u32, &str)] = &[
        (
            "cs",
            "zprava.odt",
            1,
            "zprava (konflikt Linux 2026-11-15).odt",
        ),
        (
            "cs",
            "Dokumenty/Příloha.pdf",
            1,
            "Dokumenty/Příloha (konflikt Linux 2026-11-15).pdf",
        ),
        (
            "cs",
            "zprava.odt",
            3,
            "zprava (konflikt Linux 2026-11-15 3).odt",
        ),
        ("cs", "README", 1, "README (konflikt Linux 2026-11-15)"),
        (
            "cs",
            "a/.bashrc",
            1,
            "a/.bashrc (konflikt Linux 2026-11-15)",
        ),
        (
            "en",
            "report.odt",
            1,
            "report (Linux conflict 2026-11-15).odt",
        ),
        (
            "en",
            "report.odt",
            2,
            "report (Linux conflict 2026-11-15 2).odt",
        ),
    ];
    for (code, path, number, expected) in cases {
        assert_eq!(
            lang(code).conflict_copy_name(&RelPath::new(*path), "2026-11-15", *number),
            RelPath::new(*expected),
            "{code} {path} {number}"
        );
    }
}

#[test]
fn slash_from_a_translation_stays_in_the_folder() {
    let d = TempDir::new("slash");
    d.language("xx", "conflict-copy-name = { $stem }/copy{ $extension }\n");
    let t = Translations::with_dir(&["xx".into()], d.path()).expect("load");
    assert_eq!(
        t.conflict_copy_name(&RelPath::new("d/a.txt"), "2026-11-15", 1),
        RelPath::new("d/a-copy.txt")
    );
}

#[test]
fn planner_with_a_translated_copy_name() {
    use ikeloud_safety::DEFAULT_SETTLE_TIME;
    use ikeloud_sync::{States, build_plan};
    use ikeloud_testkit::{base, local, remote};

    let cs = lang("cs");
    let states = States {
        local: vec![local("zprava.odt", 1, "h2")],
        remote: vec![remote("zprava.odt", "d1", "e2")],
        base: vec![base("zprava.odt", 1, "d1", "h1", "e1")],
        ..States::default()
    };
    let plan = build_plan(&states, DEFAULT_SETTLE_TIME, &|p, n| {
        cs.conflict_copy_name(p, "2026-11-15", n)
    });
    assert!(plan.actions.iter().any(|a| matches!(
        a,
        Action::UploadConflictCopy { path, .. }
            if path.as_str() == "zprava (konflikt Linux 2026-11-15).odt"
    )));
}
