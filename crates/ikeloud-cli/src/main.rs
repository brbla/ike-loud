//! ike loud diagnostics for advanced users and bug reports.
//!
//! ```text
//! ikeloud-cli login <apple-account>   sign in (password from IKELOUD_PASSWORD or stdin)
//! ikeloud-cli ls [/path]              list a Drive folder
//! ikeloud-cli get </path> <file>      download a file
//! ikeloud-cli mkdir </path>           create a folder
//! ikeloud-cli put <file> </path>      upload: a new file, or a new version
//!                                     of the one there (checked by version)
//! ikeloud-cli mv </from> </to>        move or rename (checked by version)
//! ikeloud-cli rm </path>              move to "Recently Deleted"
//! ```
//!
//! The write commands bypass the sync engine and its safety layer: they are
//! for checking the client against the server, on a test account.
//!
//! The session is the one the app and the daemon use
//! (`$XDG_STATE_HOME/ikeloud/session.json`, mode 0600) until it moves to the
//! keyring. It holds cookies and the trust token, never the password.

// A command-line tool talks on stdout and stderr.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::fs::{self, File};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use ikeloud_api::{
    ApiError, Client, HttpsTransport, Item, Kind, ROOT, SavedSession, SignIn, UploadTarget,
};
use ikeloud_config::{Paths, write_private};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["login", account] => login(account),
        ["ls"] => ls("/"),
        ["ls", path] => ls(path),
        ["get", path, target] => get(path, target),
        ["mkdir", path] => mkdir(path),
        ["put", file, path] => put(file, path),
        ["mv", from, to] => mv(from, to),
        ["rm", path] => rm(path),
        _ => {
            eprintln!(
                "usage: ikeloud-cli login <apple-account> | ls [/path] | get </path> <file> \
                 | mkdir </path> | put <file> </path> | mv </from> </to> | rm </path>"
            );
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn session_path() -> Result<PathBuf> {
    Ok(Paths::from_env()?.session)
}

fn save(client: &Client<HttpsTransport>) -> Result<()> {
    write_private(&session_path()?, client.saved().to_json().as_bytes())?;
    Ok(())
}

fn load() -> Result<Client<HttpsTransport>> {
    let text = fs::read_to_string(session_path()?)
        .map_err(|_| "not signed in; run: ikeloud-cli login <apple-account>")?;
    let saved =
        SavedSession::from_json(&text).ok_or("the saved session is damaged; sign in again")?;
    let mut client = Client::restore(HttpsTransport::new(), saved);
    match client.validate() {
        Ok(()) => {}
        Err(ApiError::SessionExpired) => {
            return Err("the session expired; run: ikeloud-cli login <apple-account>".into());
        }
        Err(e) => return Err(e.into()),
    }
    save(&client)?;
    Ok(client)
}

fn read_line(prompt: &str) -> Result<String> {
    eprint!("{prompt}");
    io::stderr().flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

fn login(account: &str) -> Result<()> {
    // Keep the trust token of an earlier sign-in, so the SMS can be skipped.
    let previous = session_path()
        .ok()
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|t| SavedSession::from_json(&t))
        .filter(|s| s.account_name() == account);
    let mut client = match previous {
        Some(saved) => Client::restore(HttpsTransport::new(), saved),
        None => Client::new(HttpsTransport::new(), account),
    };
    let password = match std::env::var("IKELOUD_PASSWORD") {
        Ok(p) => p,
        Err(_) => read_line("Password (input is shown): ")?,
    };
    match client.sign_in(&password)? {
        SignIn::Done => eprintln!("Signed in without a code."),
        SignIn::NeedsCode { phones } => {
            let phone = phones.first().ok_or(ApiError::NoTrustedPhone)?;
            client.send_sms(phone.id)?;
            loop {
                let code = read_line(&format!(
                    "Code sent by SMS to {} (empty line sends a new one): ",
                    phone.masked
                ))?;
                if code.is_empty() {
                    client.send_sms(phone.id)?;
                    continue;
                }
                match client.verify_sms(phone.id, &code) {
                    Ok(()) => break,
                    Err(ApiError::BadCode) => eprintln!("Wrong code, try again."),
                    Err(e) => return Err(e.into()),
                }
            }
            eprintln!("Signed in.");
        }
    }
    save(&client)
}

/// Finds the item at an absolute path, walking from the root.
fn resolve(client: &mut Client<HttpsTransport>, path: &str) -> Result<Option<Item>> {
    let mut folder = ROOT.to_string();
    let mut found = None;
    for part in path.split('/').filter(|p| !p.is_empty()) {
        let listing = client.list_folder(&folder)?;
        let item = listing
            .items
            .into_iter()
            .find(|i| i.name == part)
            .ok_or_else(|| format!("not found: {part}"))?;
        folder = item.drivewsid.clone();
        found = Some(item);
    }
    Ok(found)
}

fn ls(path: &str) -> Result<()> {
    let mut client = load()?;
    let id = match resolve(&mut client, path)? {
        None => ROOT.to_string(),
        Some(item) if item.kind == Kind::Folder => item.drivewsid,
        Some(_) => return Err(format!("not a folder: {path}").into()),
    };
    let folder = client.list_folder(&id)?;
    println!(
        "{}  etag {}  ({} items)",
        if folder.name.is_empty() {
            "/"
        } else {
            &folder.name
        },
        folder.etag,
        folder.items.len()
    );
    for item in folder.items {
        let kind = match item.kind {
            Kind::Folder => "dir ",
            Kind::File => "file",
            Kind::Other => "?   ",
        };
        let size = item.size.map(|s| s.to_string()).unwrap_or_default();
        println!(
            "{kind} {size:>12}  {:<25} etag {:<6} {}",
            item.modified.as_deref().unwrap_or("-"),
            item.etag,
            item.name
        );
    }
    save(&client)
}

fn get(path: &str, target: &str) -> Result<()> {
    let mut client = load()?;
    let item = resolve(&mut client, path)?.ok_or("give a file path")?;
    if item.kind != Kind::File {
        return Err(format!("not a file: {path}").into());
    }
    // Into a temporary file first, renamed only when complete (spec: downloads
    // to temporary files).
    let tmp = PathBuf::from(format!("{target}.ikeloud-part"));
    let mut file = File::create(&tmp)?;
    let written = match client.download(&item, &mut file) {
        Ok(n) => n,
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            return Err(e.into());
        }
    };
    file.sync_all()?;
    fs::rename(&tmp, target)?;
    println!("{written} bytes -> {target}");
    save(&client)
}

/// Parent folder path and last name of `/a/b/c`: (`/a/b`, `c`).
fn split(path: &str) -> Result<(String, String)> {
    let trimmed = path.trim_end_matches('/');
    let (parent, name) = trimmed.rsplit_once('/').ok_or("give an absolute path")?;
    if name.is_empty() {
        return Err("give a name".into());
    }
    Ok((parent.to_string(), name.to_string()))
}

/// The folder at `path` as (drive id, document id).
fn folder(client: &mut Client<HttpsTransport>, path: &str) -> Result<(String, String)> {
    match resolve(client, path)? {
        None => Ok((ROOT.to_string(), "root".to_string())),
        Some(item) if item.kind == Kind::Folder => Ok((item.drivewsid, item.docwsid)),
        Some(_) => Err(format!("not a folder: {path}").into()),
    }
}

fn mkdir(path: &str) -> Result<()> {
    let mut client = load()?;
    let (parent, name) = split(path)?;
    let (parent_id, _) = folder(&mut client, &parent)?;
    let created = client.create_folder(&parent_id, &name)?;
    println!("created {path} (etag {})", created.etag);
    save(&client)
}

fn put(file: &str, path: &str) -> Result<()> {
    let mut client = load()?;
    let (parent, name) = split(path)?;
    let (_, parent_doc) = folder(&mut client, &parent)?;
    let existing = resolve(&mut client, path).ok().flatten();
    let target = match existing {
        Some(item) if item.kind == Kind::File => UploadTarget::Replace {
            folder_docwsid: parent_doc,
            docwsid: item.docwsid,
            etag: item.etag,
        },
        Some(_) => return Err(format!("not a file: {path}").into()),
        None => UploadTarget::New {
            folder_docwsid: parent_doc,
        },
    };
    let mut content = File::open(file)?;
    let meta = content.metadata()?;
    let mtime_ms = meta
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    let uploaded = client.upload(&target, &name, meta.len(), &mut content, mtime_ms)?;
    let how = if matches!(target, UploadTarget::Replace { .. }) {
        "replaced"
    } else {
        "uploaded"
    };
    println!("{how} {path} (etag {})", uploaded.etag);
    save(&client)
}

fn mv(from: &str, to: &str) -> Result<()> {
    let mut client = load()?;
    let item = resolve(&mut client, from)?.ok_or("give a path")?;
    let (from_parent, _) = split(from)?;
    let (to_parent, to_name) = split(to)?;
    let mut etag = item.etag.clone();
    if from_parent != to_parent {
        let (dest, _) = folder(&mut client, &to_parent)?;
        etag = client.move_item(&item.drivewsid, &etag, &dest)?;
    }
    if item.name != to_name {
        etag = client.rename(&item.drivewsid, &etag, &to_name)?;
    }
    println!("moved {from} -> {to} (etag {etag})");
    save(&client)
}

fn rm(path: &str) -> Result<()> {
    let mut client = load()?;
    let item = resolve(&mut client, path)?.ok_or("give a path")?;
    client.trash(&item.drivewsid, &item.etag)?;
    println!("moved {path} to Recently Deleted");
    save(&client)
}
