//! ike loud: the app.
//!
//! First run: sign in (password, then the SMS code), choose a new or empty
//! folder, start the daemon. Afterwards: the status window and the
//! saxophone in the tray. The app signs in itself and hands the session to
//! the daemon through `ikeloud_config::Paths::session`; the password is
//! never stored.

mod daemon;
mod icons;
mod tray;

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use adw::prelude::*;
use gtk::{gio, glib};

use ikeloud_api::{ApiError, Client, HttpsTransport, SavedSession, SignIn, TrustedPhone};
use ikeloud_config::{
    FolderProblem, Paths, Settings, State, Status, check_folder, protected_folder, write_private,
};
use ikeloud_i18n::{Message, Translations, Value, languages_from_env};

use crate::daemon::{DaemonProcess, EXIT_SIGNED_OUT};
use crate::icons::{APP_ID, Badge};
use crate::tray::{MenuItem, Tray, TrayAction};

type SharedClient = Arc<Mutex<Option<Client<HttpsTransport>>>>;

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(|app| {
        // A second start only shows the window of the first.
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        match Paths::from_env() {
            Ok(paths) => App::start(app, paths),
            Err(_) => app.quit(),
        }
    });
    app.run()
}

/// Widgets of the status page.
struct StatusView {
    icon: gtk::Image,
    title: gtk::Label,
    detail: gtk::Label,
    pause: gtk::Button,
}

struct App {
    app: adw::Application,
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    nav: adw::NavigationView,
    paths: Paths,
    texts: Option<Translations>,
    client: SharedClient,
    phone: RefCell<Option<TrustedPhone>>,
    folder: RefCell<PathBuf>,
    daemon: RefCell<DaemonProcess>,
    paused: Cell<bool>,
    tray: RefCell<Option<Tray>>,
    status_view: RefCell<Option<StatusView>>,
    shown: Cell<Option<(Badge, String)>>,
    _hold: gio::ApplicationHoldGuard,
}

impl App {
    fn start(app: &adw::Application, paths: Paths) {
        let texts = Translations::new(&languages_from_env()).ok();
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .default_width(460)
            .default_height(560)
            .title("ike loud")
            .icon_name(APP_ID)
            .build();
        let nav = adw::NavigationView::new();
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&nav));
        window.set_content(Some(&toasts));

        let icon_dir = paths
            .state_db
            .parent()
            .unwrap_or(Path::new("/tmp"))
            .join("icons");
        let icons_ready = icons::install(&icon_dir).is_ok();
        if icons_ready && let Some(display) = gtk::gdk::Display::default() {
            gtk::IconTheme::for_display(&display).add_search_path(&icon_dir);
        }

        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let this = Rc::new(Self {
            app: app.clone(),
            window,
            toasts,
            nav,
            paths,
            texts,
            client: Arc::new(Mutex::new(None)),
            phone: RefCell::new(None),
            folder: RefCell::new(home.join("iCloud")),
            daemon: RefCell::new(DaemonProcess::default()),
            paused: Cell::new(false),
            tray: RefCell::new(None),
            status_view: RefCell::new(None),
            shown: Cell::new(None),
            _hold: app.hold(),
        });

        if icons_ready {
            let weak = Rc::downgrade(&this);
            let tray = Tray::start(&icon_dir, this.menu(), move |action| {
                if let Some(app) = weak.upgrade() {
                    app.tray_action(action);
                }
            });
            *this.tray.borrow_mut() = tray.ok();
        }

        let weak = Rc::downgrade(&this);
        this.window.connect_close_request(move |window| {
            // With a tray icon the app lives on in the tray.
            if weak
                .upgrade()
                .is_some_and(|app| app.tray.borrow().is_some())
            {
                window.set_visible(false);
                glib::Propagation::Stop
            } else {
                if let Some(app) = weak.upgrade() {
                    app.app.quit();
                }
                glib::Propagation::Proceed
            }
        });

        let settings = Settings::load(&this.paths.settings).unwrap_or_default();
        let session = std::fs::read_to_string(&this.paths.session)
            .ok()
            .and_then(|t| SavedSession::from_json(&t));
        match (settings.folder, session) {
            (Some(folder), Some(_)) => {
                *this.folder.borrow_mut() = folder;
                this.show_status();
                this.start_daemon();
            }
            (None, Some(_)) => this.nav.replace(&[this.folder_page()]),
            (_, None) => this.nav.replace(&[this.signin_page()]),
        }

        let weak = Rc::downgrade(&this);
        glib::timeout_add_seconds_local(1, move || match weak.upgrade() {
            Some(app) => {
                app.tick();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        this.window.present();
        // Every handler holds only a weak reference; this is the one strong
        // reference, kept by the application until it shuts down. Without it
        // the app state was freed right after start and no button reacted.
        let keep = Rc::clone(&this);
        app.connect_shutdown(move |_| {
            let _ = &keep;
        });
    }

    fn t(&self, key: &'static str) -> String {
        self.message(&Message::new(key))
    }

    fn message(&self, message: &Message) -> String {
        match &self.texts {
            Some(t) => t.text(message),
            None => message.key.to_string(),
        }
    }

    fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::new(text));
    }

    fn error_text(&self, error: &ApiError) -> String {
        let key = match error {
            ApiError::BadCredentials => "gui-error-credentials",
            ApiError::BadCode => "gui-error-code",
            ApiError::TooManyCodes => "gui-error-too-many",
            ApiError::NoTrustedPhone => "gui-error-no-phone",
            ApiError::Transport(_) => "gui-error-offline",
            ApiError::DriveNotSetUp => "gui-error-drive-not-set-up",
            ApiError::DriveUnavailable => "gui-error-drive-unavailable",
            other => {
                return self.message(
                    &Message::new("gui-error-other").arg("detail", Value::Text(other.to_string())),
                );
            }
        };
        self.t(key)
    }

    /// A page: header bar, a clamp, and `content` in a vertical box.
    fn page(&self, title: &str, content: &gtk::Box) -> adw::NavigationPage {
        self.page_with(title, content, None)
    }

    /// A page with a button at the end of the header bar.
    fn page_with(
        &self,
        title: &str,
        content: &gtk::Box,
        header_end: Option<&gtk::Button>,
    ) -> adw::NavigationPage {
        let toolbar = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        if let Some(button) = header_end {
            header.pack_end(button);
        }
        toolbar.add_top_bar(&header);
        let clamp = adw::Clamp::builder()
            .maximum_size(420)
            .child(content)
            .margin_top(12)
            .margin_bottom(24)
            .margin_start(12)
            .margin_end(12)
            .build();
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&clamp)
            .build();
        toolbar.set_content(Some(&scroll));
        adw::NavigationPage::new(&toolbar, title)
    }

    fn intro(title: &str, text: &str) -> gtk::Box {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
        let heading = gtk::Label::builder()
            .label(title)
            .css_classes(["title-1"])
            .wrap(true)
            .build();
        let body = gtk::Label::builder()
            .label(text)
            .wrap(true)
            .justify(gtk::Justification::Center)
            .css_classes(["dim-label"])
            .build();
        content.append(&heading);
        content.append(&body);
        content
    }

    fn action_button(label: &str) -> gtk::Button {
        gtk::Button::builder()
            .label(label)
            .halign(gtk::Align::Center)
            .css_classes(["suggested-action", "pill"])
            .build()
    }

    /// Runs `work` off the main thread with the client, then `done` on it.
    fn with_client<T: Send + 'static>(
        self: &Rc<Self>,
        button: &gtk::Button,
        work: impl FnOnce(&mut Option<Client<HttpsTransport>>) -> Result<T, ApiError> + Send + 'static,
        done: impl FnOnce(&Rc<Self>, Result<T, ApiError>) + 'static,
    ) {
        button.set_sensitive(false);
        let spinner = gtk::Spinner::builder().spinning(true).build();
        let label = button.label();
        button.set_child(Some(&spinner));
        let client = Arc::clone(&self.client);
        let this = Rc::clone(self);
        let button = button.clone();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || {
                let mut guard = client.lock().map_err(|_| ApiError::NotSignedIn)?;
                work(&mut guard)
            })
            .await
            .unwrap_or(Err(ApiError::NotSignedIn));
            button.set_child(None::<&gtk::Widget>);
            if let Some(label) = label {
                button.set_label(&label);
            }
            button.set_sensitive(true);
            done(&this, result);
        });
    }

    fn save_session(&self, client: &Client<HttpsTransport>) {
        let _ = write_private(&self.paths.session, client.saved().to_json().as_bytes());
    }

    fn signin_page(self: &Rc<Self>) -> adw::NavigationPage {
        let content = Self::intro(
            &self.t("gui-signin-title"),
            &self.t("gui-signin-description"),
        );
        let group = adw::PreferencesGroup::new();
        let account = adw::EntryRow::builder()
            .title(self.t("gui-account"))
            .build();
        account.set_input_purpose(gtk::InputPurpose::Email);
        let password = adw::PasswordEntryRow::builder()
            .title(self.t("gui-password"))
            .build();
        group.add(&account);
        group.add(&password);
        content.append(&group);
        let button = Self::action_button(&self.t("gui-signin"));
        content.append(&button);

        let this = Rc::downgrade(self);
        let submit = {
            let account = account.clone();
            let password = password.clone();
            move |button: &gtk::Button| {
                let Some(this) = this.upgrade() else { return };
                let name = account.text().trim().to_string();
                let secret = password.text().to_string();
                if name.is_empty() || secret.is_empty() {
                    return;
                }
                // Keep the trust token of an earlier sign-in of this account.
                let previous = std::fs::read_to_string(&this.paths.session)
                    .ok()
                    .and_then(|t| SavedSession::from_json(&t))
                    .filter(|s| s.account_name() == name);
                let password = password.clone();
                this.with_client(
                    button,
                    move |slot| {
                        let mut client = match previous {
                            Some(saved) => Client::restore(HttpsTransport::new(), saved),
                            None => Client::new(HttpsTransport::new(), &name),
                        };
                        let outcome = client.sign_in(&secret);
                        let outcome = match outcome {
                            Ok(SignIn::NeedsCode { phones }) => {
                                let phone =
                                    phones.into_iter().next().ok_or(ApiError::NoTrustedPhone);
                                phone.and_then(|p| client.send_sms(p.id).map(|()| Some(p)))
                            }
                            Ok(SignIn::Done) => Ok(None),
                            Err(e) => Err(e),
                        };
                        *slot = Some(client);
                        outcome
                    },
                    move |this, result| {
                        password.set_text("");
                        match result {
                            Ok(Some(phone)) => {
                                *this.phone.borrow_mut() = Some(phone);
                                this.nav.push(&this.code_page());
                            }
                            Ok(None) => this.signed_in(),
                            Err(e) => this.toast(&this.error_text(&e)),
                        }
                    },
                );
            }
        };
        let on_enter = {
            let button = button.clone();
            let submit = submit.clone();
            move |_: &adw::PasswordEntryRow| submit(&button)
        };
        password.connect_entry_activated(on_enter);
        button.connect_clicked(submit);
        self.page(&self.t("gui-signin-title"), &content)
    }

    fn code_page(self: &Rc<Self>) -> adw::NavigationPage {
        let phone = self.phone.borrow().clone();
        let ending: String = phone
            .as_ref()
            .map(|p| p.masked.chars().filter(char::is_ascii_digit).collect())
            .unwrap_or_default();
        let description =
            self.message(&Message::new("gui-code-description").arg("phone", Value::Text(ending)));
        let content = Self::intro(&self.t("gui-code-title"), &description);
        let group = adw::PreferencesGroup::new();
        let code = adw::EntryRow::builder().title(self.t("gui-code")).build();
        code.set_input_purpose(gtk::InputPurpose::Digits);
        group.add(&code);
        content.append(&group);
        let verify = Self::action_button(&self.t("gui-verify"));
        let resend = gtk::Button::builder()
            .label(self.t("gui-resend"))
            .halign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        content.append(&verify);
        content.append(&resend);
        let phone_id = phone.map(|p| p.id).unwrap_or(1);

        let this = Rc::downgrade(self);
        let submit = {
            let code = code.clone();
            move |button: &gtk::Button| {
                let Some(this) = this.upgrade() else { return };
                let text = code.text().trim().to_string();
                if text.is_empty() {
                    return;
                }
                let code = code.clone();
                this.with_client(
                    button,
                    move |slot| {
                        let client = slot.as_mut().ok_or(ApiError::NotSignedIn)?;
                        client.verify_sms(phone_id, &text)
                    },
                    move |this, result| match result {
                        Ok(()) => this.signed_in(),
                        Err(e) => {
                            code.set_text("");
                            this.toast(&this.error_text(&e));
                        }
                    },
                );
            }
        };
        let on_enter = {
            let verify = verify.clone();
            let submit = submit.clone();
            move |_: &adw::EntryRow| submit(&verify)
        };
        code.connect_entry_activated(on_enter);
        verify.connect_clicked(submit);
        let this = Rc::downgrade(self);
        resend.connect_clicked(move |button| {
            let Some(this) = this.upgrade() else { return };
            this.with_client(
                button,
                move |slot| {
                    let client = slot.as_mut().ok_or(ApiError::NotSignedIn)?;
                    client.send_sms(phone_id)
                },
                |this, result| match result {
                    Ok(()) => this.toast(&this.t("gui-code-sent")),
                    Err(e) => this.toast(&this.error_text(&e)),
                },
            );
        });
        self.page(&self.t("gui-code-title"), &content)
    }

    /// After a successful sign-in: save the session, then the folder (first
    /// run) or the status.
    fn signed_in(self: &Rc<Self>) {
        if let Ok(guard) = self.client.lock()
            && let Some(client) = guard.as_ref()
        {
            self.save_session(client);
        }
        let configured = Settings::load(&self.paths.settings)
            .ok()
            .and_then(|s| s.folder);
        match configured {
            Some(folder) => {
                *self.folder.borrow_mut() = folder;
                self.show_status();
                self.start_daemon();
            }
            None => self.nav.replace(&[self.folder_page()]),
        }
    }

    fn folder_page(self: &Rc<Self>) -> adw::NavigationPage {
        let content = Self::intro(
            &self.t("gui-folder-title"),
            &self.t("gui-folder-description"),
        );
        let group = adw::PreferencesGroup::new();
        let row = adw::ActionRow::builder()
            .title(self.t("gui-folder"))
            .subtitle(self.folder.borrow().to_string_lossy().as_ref())
            .build();
        let choose = gtk::Button::builder()
            .label(self.t("gui-folder-choose"))
            .valign(gtk::Align::Center)
            .build();
        row.add_suffix(&choose);
        group.add(&row);
        content.append(&group);
        let problem = gtk::Label::builder().wrap(true).visible(false).build();
        content.append(&problem);
        let done = Self::action_button(&self.t("gui-folder-done"));
        content.append(&done);

        let this = Rc::downgrade(self);
        let check = {
            let problem = problem.clone();
            let done = done.clone();
            move |this: &Rc<Self>| {
                let folder = this.folder.borrow().clone();
                let (text, error) = match this.folder_verdict(&folder, None) {
                    Ok(0) => (None, false),
                    Ok(count) => (Some(this.not_empty_text(count)), false),
                    Err(text) => (Some(text), true),
                };
                problem.set_visible(text.is_some());
                problem.set_label(text.as_deref().unwrap_or_default());
                problem.set_css_classes(if error { &["error"] } else { &["warning"] });
                done.set_sensitive(!error);
            }
        };
        check(self);
        {
            let check = check.clone();
            let row = row.clone();
            choose.connect_clicked(move |_| {
                let Some(this) = this.upgrade() else { return };
                let dialog = gtk::FileDialog::builder()
                    .title(this.t("gui-folder"))
                    .modal(true)
                    .build();
                let check = check.clone();
                let row = row.clone();
                let weak = Rc::downgrade(&this);
                dialog.select_folder(Some(&this.window), gio::Cancellable::NONE, move |result| {
                    let (Some(this), Ok(file)) = (weak.upgrade(), result) else {
                        return;
                    };
                    if let Some(path) = file.path() {
                        row.set_subtitle(&path.to_string_lossy());
                        *this.folder.borrow_mut() = path;
                        check(&this);
                    }
                });
            });
        }
        let this = Rc::downgrade(self);
        done.connect_clicked(move |_| {
            let Some(this) = this.upgrade() else { return };
            let folder = this.folder.borrow().clone();
            let count = match this.folder_verdict(&folder, None) {
                Ok(count) => count,
                Err(_) => {
                    check(&this);
                    return;
                }
            };
            let start = move |this: &Rc<Self>| {
                if let Err(text) = this.empty_into_trash(&folder) {
                    this.toast(&text);
                    return;
                }
                let saved = std::fs::create_dir_all(&folder).and_then(|()| {
                    Settings {
                        folder: Some(folder.clone()),
                    }
                    .save(&this.paths.settings)
                });
                match saved {
                    Ok(()) => {
                        this.show_status();
                        this.start_daemon();
                    }
                    Err(e) => this.toast(&format!("{}: {e}", this.t("gui-folder-error"))),
                }
            };
            if count == 0 {
                start(&this);
                return;
            }
            let body = this.not_empty_text(count);
            this.confirm(
                &this.t("gui-folder-title"),
                &body,
                &[("start", "gui-folder-trash-and-start", true)],
                move |this, response| {
                    if response == "start" {
                        start(this);
                    }
                },
            );
        });
        self.page(&self.t("gui-folder-title"), &content)
    }

    /// Whether `folder` can become the synced folder: `Ok(n)` with the
    /// number of items that would go to the trash first (0: new or empty),
    /// or why not. `current` is the folder synced now (it must not contain
    /// the new one or lie inside it).
    fn folder_verdict(&self, folder: &Path, current: Option<&Path>) -> Result<usize, String> {
        if let Some(current) = current
            && (folder.starts_with(current) || current.starts_with(folder))
        {
            return Err(self.t("gui-folder-nested"));
        }
        match check_folder(folder, &self.paths) {
            Ok(()) => Ok(0),
            Err(FolderProblem::NotEmpty) => {
                if protected_folder(folder, &home(), &user_dirs()) {
                    return Err(self.t("gui-folder-protected"));
                }
                std::fs::read_dir(folder)
                    .map(Iterator::count)
                    .map_err(|e| format!("{}: {e}", self.t("gui-folder-error")))
            }
            Err(FolderProblem::NotAFolder) => Err(self.t("gui-folder-not-a-folder")),
            Err(FolderProblem::Overlaps(_)) => Err(self.t("gui-folder-overlaps")),
            Err(_) => Err(self.t("gui-folder-error")),
        }
    }

    fn not_empty_text(&self, count: usize) -> String {
        self.message(&Message::new("gui-folder-will-be-emptied").arg(
            "count",
            Value::Number(u64::try_from(count).unwrap_or(u64::MAX)),
        ))
    }

    /// Moves everything in `folder` to the trash (recoverable), so only
    /// iCloud ends up there. Stops at the first item that cannot be moved.
    fn empty_into_trash(&self, folder: &Path) -> Result<(), String> {
        if protected_folder(folder, &home(), &user_dirs()) {
            return Err(self.t("gui-folder-protected"));
        }
        let entries = match std::fs::read_dir(folder) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(format!("{}: {e}", self.t("gui-folder-error"))),
        };
        for entry in entries {
            let path = entry
                .map_err(|e| format!("{}: {e}", self.t("gui-folder-error")))?
                .path();
            gio::File::for_path(&path)
                .trash(gio::Cancellable::NONE)
                .map_err(|e| {
                    self.message(
                        &Message::new("gui-folder-trash-failed")
                            .arg("detail", Value::Text(format!("{}: {e}", path.display()))),
                    )
                })?;
        }
        Ok(())
    }

    /// A question with Cancel and the given answers `(id, text key,
    /// destructive)`; `on_answer` gets the id (or "cancel").
    fn confirm(
        self: &Rc<Self>,
        heading: &str,
        body: &str,
        answers: &[(&str, &'static str, bool)],
        on_answer: impl FnOnce(&Rc<Self>, &str) + 'static,
    ) {
        let dialog = adw::AlertDialog::new(Some(heading), Some(body));
        dialog.add_response("cancel", &self.t("gui-cancel"));
        for (id, key, destructive) in answers {
            dialog.add_response(id, &self.t(key));
            dialog.set_response_appearance(
                id,
                if *destructive {
                    adw::ResponseAppearance::Destructive
                } else {
                    adw::ResponseAppearance::Suggested
                },
            );
        }
        dialog.set_close_response("cancel");
        dialog.set_default_response(Some("cancel"));
        let weak = Rc::downgrade(self);
        dialog.choose(Some(&self.window), gio::Cancellable::NONE, move |answer| {
            if let Some(this) = weak.upgrade() {
                on_answer(&this, answer.as_str());
            }
        });
    }

    fn show_status(self: &Rc<Self>) {
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .valign(gtk::Align::Center)
            .vexpand(true)
            .build();
        // A small icon, so the page fits a small screen without scrolling.
        let icon = gtk::Image::builder()
            .icon_name(Badge::Quiet.icon_name())
            .pixel_size(64)
            .build();
        let title = gtk::Label::builder()
            .label(self.t("gui-state-starting"))
            .wrap(true)
            .css_classes(["title-2"])
            .build();
        let detail = gtk::Label::builder()
            .wrap(true)
            .justify(gtk::Justification::Center)
            .css_classes(["dim-label"])
            .build();
        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .halign(gtk::Align::Center)
            .margin_top(12)
            .build();
        let open = Self::action_button(&self.t("gui-open-folder"));
        let pause = gtk::Button::builder()
            .label(self.t("gui-pause"))
            .css_classes(["pill"])
            .build();
        buttons.append(&open);
        buttons.append(&pause);
        let note = gtk::Label::builder()
            .label(self.t("gui-read-only"))
            .wrap(true)
            .justify(gtk::Justification::Center)
            .margin_top(12)
            .css_classes(["dim-label", "caption"])
            .build();
        content.append(&icon);
        content.append(&title);
        content.append(&detail);
        content.append(&buttons);
        content.append(&note);

        let settings = gtk::Button::builder()
            .icon_name("emblem-system-symbolic")
            .tooltip_text(self.t("gui-settings"))
            .build();
        let this = Rc::downgrade(self);
        settings.connect_clicked(move |_| {
            if let Some(this) = this.upgrade() {
                this.nav.push(&this.settings_page());
            }
        });
        let this = Rc::downgrade(self);
        open.connect_clicked(move |_| {
            if let Some(this) = this.upgrade() {
                this.open_folder();
            }
        });
        let this = Rc::downgrade(self);
        pause.connect_clicked(move |_| {
            if let Some(this) = this.upgrade() {
                this.toggle_pause();
            }
        });
        *self.status_view.borrow_mut() = Some(StatusView {
            icon,
            title,
            detail,
            pause,
        });
        self.shown.set(None);
        self.nav
            .replace(&[self.page_with("ike loud", &content, Some(&settings))]);
        self.tick();
    }

    /// Settings: the synced folder and the account.
    fn settings_page(self: &Rc<Self>) -> adw::NavigationPage {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 24);

        let folder_group = adw::PreferencesGroup::builder()
            .title(self.t("gui-folder"))
            .description(self.t("gui-folder-change-description"))
            .build();
        let folder_row = adw::ActionRow::builder()
            .title(self.t("gui-folder"))
            .subtitle(self.folder.borrow().to_string_lossy().as_ref())
            .build();
        folder_row.add_css_class("property");
        let change = gtk::Button::builder()
            .label(self.t("gui-folder-change"))
            .valign(gtk::Align::Center)
            .build();
        folder_row.add_suffix(&change);
        folder_group.add(&folder_row);
        content.append(&folder_group);

        let account_group = adw::PreferencesGroup::builder()
            .title(self.t("gui-account-section"))
            .build();
        let account = std::fs::read_to_string(&self.paths.session)
            .ok()
            .and_then(|t| SavedSession::from_json(&t))
            .map(|s| s.account_name().to_string())
            .unwrap_or_default();
        let account_row = adw::ActionRow::builder()
            .title(self.t("gui-account"))
            .subtitle(account)
            .build();
        account_row.add_css_class("property");
        let sign_out = gtk::Button::builder()
            .label(self.t("gui-sign-out"))
            .valign(gtk::Align::Center)
            .css_classes(["destructive-action"])
            .build();
        account_row.add_suffix(&sign_out);
        account_group.add(&account_row);
        content.append(&account_group);

        let this = Rc::downgrade(self);
        change.connect_clicked(move |_| {
            let Some(this) = this.upgrade() else { return };
            let dialog = gtk::FileDialog::builder()
                .title(this.t("gui-folder"))
                .modal(true)
                .build();
            let weak = Rc::downgrade(&this);
            let row = folder_row.clone();
            dialog.select_folder(Some(&this.window), gio::Cancellable::NONE, move |result| {
                let (Some(this), Ok(file)) = (weak.upgrade(), result) else {
                    return;
                };
                if let Some(path) = file.path() {
                    this.change_folder(&path, &row);
                }
            });
        });
        let this = Rc::downgrade(self);
        sign_out.connect_clicked(move |_| {
            if let Some(this) = this.upgrade() {
                this.sign_out();
            }
        });
        self.page(&self.t("gui-settings"), &content)
    }

    /// Moves syncing to another folder after the user confirms: what is in
    /// the new folder goes to the trash first, the old folder is kept or
    /// goes to the trash, and iCloud Drive is downloaded again. The sync
    /// state belongs to the old folder and is dropped: kept, it would take
    /// the files missing in the new folder for deleted ones. Read-only, so
    /// iCloud itself is never touched.
    fn change_folder(self: &Rc<Self>, folder: &Path, row: &adw::ActionRow) {
        let old = self.folder.borrow().clone();
        if folder == old.as_path() {
            return;
        }
        let count = match self.folder_verdict(folder, Some(&old)) {
            Ok(count) => count,
            Err(text) => {
                self.toast(&text);
                return;
            }
        };
        let mut body = self.message(
            &Message::new("gui-change-body")
                .arg("folder", Value::Text(folder.display().to_string())),
        );
        if count > 0 {
            body.push_str("\n\n");
            body.push_str(&self.not_empty_text(count));
        }
        let old_exists = old.is_dir() && !protected_folder(&old, &home(), &user_dirs());
        let mut answers = vec![("keep", "gui-change-keep-old", false)];
        if old_exists {
            body.push_str("\n\n");
            body.push_str(&self.message(
                &Message::new("gui-change-old").arg("old", Value::Text(old.display().to_string())),
            ));
            answers.push(("trash", "gui-change-trash-old", true));
        }
        let folder = folder.to_path_buf();
        let row = row.clone();
        self.confirm(
            &self.t("gui-change-heading"),
            &body,
            &answers,
            move |this, answer| {
                if answer == "keep" || answer == "trash" {
                    this.apply_folder_change(&folder, &old, answer == "trash", &row);
                }
            },
        );
    }

    fn apply_folder_change(
        self: &Rc<Self>,
        folder: &Path,
        old: &Path,
        trash_old: bool,
        row: &adw::ActionRow,
    ) {
        let status = Status::load(&self.paths.status);
        let was_paused = self.paused.get();
        self.daemon.borrow_mut().stop(status.as_ref());
        let resume = |this: &Rc<Self>| {
            if !was_paused {
                this.start_daemon();
            }
            this.tick();
        };
        if let Err(text) = self.empty_into_trash(folder) {
            self.toast(&text);
            resume(self);
            return;
        }
        let saved = std::fs::create_dir_all(folder).and_then(|()| {
            Settings {
                folder: Some(folder.to_path_buf()),
            }
            .save(&self.paths.settings)
        });
        if let Err(e) = saved {
            self.toast(&format!("{}: {e}", self.t("gui-folder-error")));
            resume(self);
            return;
        }
        let db = self.paths.state_db.as_os_str().to_owned();
        for suffix in ["", "-wal", "-shm"] {
            let mut path = db.clone();
            path.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(path));
        }
        let _ = std::fs::remove_file(&self.paths.status);
        *self.folder.borrow_mut() = folder.to_path_buf();
        row.set_subtitle(&folder.to_string_lossy());
        let mut note = self.t("gui-folder-changed");
        if trash_old && let Err(e) = gio::File::for_path(old).trash(gio::Cancellable::NONE) {
            note = self.message(
                &Message::new("gui-folder-trash-failed")
                    .arg("detail", Value::Text(format!("{}: {e}", old.display()))),
            );
        }
        self.toast(&note);
        resume(self);
    }

    fn start_daemon(&self) {
        self.paused.set(false);
        let status = Status::load(&self.paths.status);
        if let Err(e) = self.daemon.borrow_mut().start(status.as_ref()) {
            self.toast(&e.to_string());
        }
    }

    fn toggle_pause(self: &Rc<Self>) {
        if self.paused.get() {
            self.start_daemon();
        } else {
            let status = Status::load(&self.paths.status);
            self.daemon.borrow_mut().stop(status.as_ref());
            self.paused.set(true);
        }
        self.tick();
    }

    fn sign_out(self: &Rc<Self>) {
        let status = Status::load(&self.paths.status);
        self.daemon.borrow_mut().stop(status.as_ref());
        let _ = std::fs::remove_file(&self.paths.session);
        let _ = std::fs::remove_file(&self.paths.status);
        if let Ok(mut guard) = self.client.lock() {
            *guard = None;
        }
        *self.status_view.borrow_mut() = None;
        self.paused.set(false);
        self.nav.replace(&[self.signin_page()]);
        self.tick();
    }

    fn open_folder(&self) {
        let file = gio::File::for_path(&*self.folder.borrow());
        gtk::FileLauncher::new(Some(&file)).launch(
            Some(&self.window),
            gio::Cancellable::NONE,
            |_| {},
        );
    }

    fn menu(&self) -> Vec<MenuItem> {
        let pause = if self.paused.get() {
            "gui-resume"
        } else {
            "gui-pause"
        };
        vec![
            MenuItem::Entry {
                label: self.t("gui-show"),
                action: TrayAction::Show,
            },
            MenuItem::Entry {
                label: self.t("gui-open-folder"),
                action: TrayAction::OpenFolder,
            },
            MenuItem::Separator,
            MenuItem::Entry {
                label: self.t(pause),
                action: TrayAction::TogglePause,
            },
            MenuItem::Separator,
            MenuItem::Entry {
                label: self.t("gui-quit"),
                action: TrayAction::Quit,
            },
        ]
    }

    fn tray_action(self: &Rc<Self>, action: TrayAction) {
        match action {
            TrayAction::Show => self.window.present(),
            TrayAction::OpenFolder => self.open_folder(),
            TrayAction::TogglePause => {
                if self.status_view.borrow().is_some() {
                    self.toggle_pause();
                }
            }
            // The daemon keeps syncing without the app.
            TrayAction::Quit => self.app.quit(),
        }
    }

    /// Once a second: what the daemon does, shown in the window and tray.
    fn tick(self: &Rc<Self>) {
        let exited = self.daemon.borrow_mut().exited();
        let signed_out_exit = exited.and_then(|s| s.code()) == Some(EXIT_SIGNED_OUT);
        let status = Status::load(&self.paths.status);
        let state = if signed_out_exit {
            Some(State::SignedOut)
        } else {
            status.as_ref().map(|s| s.state)
        };
        let paused = self.paused.get();
        let badge = Badge::for_state(state, paused);
        let (title, detail) = self.describe(state, status.as_ref(), paused);

        if let Some(tray) = self.tray.borrow().as_ref() {
            tray.update(badge, &title);
            tray.set_menu(self.menu());
        }
        let key = (badge, format!("{title}\n{detail}"));
        let previous = self.shown.replace(Some(key.clone()));
        if previous.as_ref() == Some(&key) {
            return;
        }
        if let Some(view) = self.status_view.borrow().as_ref() {
            view.icon.set_icon_name(Some(&badge.icon_name()));
            view.title.set_label(&title);
            view.detail.set_label(&detail);
            view.detail.set_visible(!detail.is_empty());
            view.pause
                .set_label(&self.t(if paused { "gui-resume" } else { "gui-pause" }));
        }
        if state == Some(State::SignedOut) && self.status_view.borrow().is_some() {
            *self.status_view.borrow_mut() = None;
            self.nav.replace(&[self.signin_page()]);
            self.window.present();
        }
    }

    fn describe(
        &self,
        state: Option<State>,
        status: Option<&Status>,
        paused: bool,
    ) -> (String, String) {
        if paused {
            return (
                self.t("gui-state-paused"),
                self.t("gui-state-paused-detail"),
            );
        }
        let files = status.map(|s| s.remote_files).unwrap_or(0);
        let downloaded = status.map(|s| s.downloaded).unwrap_or(0);
        let raw = status.map(|s| s.detail.clone()).unwrap_or_default();
        match state {
            Some(State::Idle) => (
                self.t("gui-state-idle"),
                self.message(
                    &Message::new("gui-state-idle-detail")
                        .arg("files", Value::Number(files))
                        .arg("downloaded", Value::Number(downloaded)),
                ),
            ),
            Some(State::Syncing) => (
                self.t("gui-state-syncing"),
                self.message(
                    &Message::new("gui-state-syncing-detail")
                        .arg("downloaded", Value::Number(downloaded)),
                ),
            ),
            Some(State::NeedsConfirmation) => (self.t("gui-state-needs-confirmation"), raw),
            Some(State::SignedOut) => (
                self.t("gui-state-signed-out"),
                self.t("gui-state-signed-out-detail"),
            ),
            Some(State::Offline) => (
                self.t("gui-state-offline"),
                self.t("gui-state-offline-detail"),
            ),
            Some(State::Error) => (self.t("gui-state-error"), raw),
            Some(State::Starting) | None => (self.t("gui-state-starting"), String::new()),
        }
    }
}

fn home() -> PathBuf {
    glib::home_dir()
}

/// The user's standard folders (Documents, Desktop, …), never emptied.
fn user_dirs() -> Vec<PathBuf> {
    use glib::UserDirectory as U;
    [
        U::Desktop,
        U::Documents,
        U::Downloads,
        U::Music,
        U::Pictures,
        U::PublicShare,
        U::Templates,
        U::Videos,
    ]
    .into_iter()
    .filter_map(glib::user_special_dir)
    .collect()
}
