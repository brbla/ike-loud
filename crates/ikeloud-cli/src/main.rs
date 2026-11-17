//! ike loud diagnostics for advanced users and bug reports.
//!
//! ```text
//! ikeloud-cli login <apple-account>   sign in (password from IKELOUD_PASSWORD or stdin)
//! ikeloud-cli ls [/path]              list a Drive folder
//! ikeloud-cli get </path> <file>      download a file
//! ```
//!
//! The session is kept in `$XDG_STATE_HOME/ikeloud/cli-session.json`
//! (mode 0600) until the daemon keeps it in the keyring. It holds cookies and
//! the trust token, never the password.

// A command-line tool talks on stdout and stderr.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::process::ExitCode;

use ikeloud_api::{ApiError, Client, HttpsTransport, Item, Kind, ROOT, SavedSession, SignIn};

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
        _ => {
            eprintln!("usage: ikeloud-cli login <apple-account> | ls [/path] | get </path> <file>");
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
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .ok_or("neither XDG_STATE_HOME nor HOME is set")?;
    Ok(base.join("ikeloud").join("cli-session.json"))
}

fn save(client: &Client<HttpsTransport>) -> Result<()> {
    let path = session_path()?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    file.write_all(client.saved().to_json().as_bytes())?;
    file.sync_all()?;
    fs::rename(&tmp, &path)?;
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
