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
use ikeloud_config::{FolderProblem, Paths, Settings, State, Status, check_folder, write_private};
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
    page: adw::StatusPage,
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
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
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
        let problem = gtk::Label::builder()
            .wrap(true)
            .css_classes(["error"])
            .visible(false)
            .build();
        content.append(&problem);
        let done = Self::action_button(&self.t("gui-folder-done"));
        content.append(&done);

        let this = Rc::downgrade(self);
        let check = {
            let problem = problem.clone();
            let done = done.clone();
            move |this: &Rc<Self>| {
                let folder = this.folder.borrow().clone();
                let text = match check_folder(&folder, &this.paths) {
                    Ok(()) => None,
                    Err(FolderProblem::NotEmpty) => Some(this.t("gui-folder-not-empty")),
                    Err(FolderProblem::NotAFolder) => Some(this.t("gui-folder-not-a-folder")),
                    Err(FolderProblem::Overlaps(_)) => Some(this.t("gui-folder-overlaps")),
                    Err(_) => Some(this.t("gui-folder-error")),
                };
                problem.set_visible(text.is_some());
                problem.set_label(text.as_deref().unwrap_or_default());
                done.set_sensitive(text.is_none());
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
            if check_folder(&folder, &this.paths).is_err() {
                check(&this);
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
        });
        self.page(&self.t("gui-folder-title"), &content)
    }

    fn show_status(self: &Rc<Self>) {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let page = adw::StatusPage::builder()
            .icon_name(Badge::Quiet.icon_name())
            .title(self.t("gui-state-starting"))
            .vexpand(true)
            .build();
        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .halign(gtk::Align::Center)
            .build();
        let open = Self::action_button(&self.t("gui-open-folder"));
        let pause = gtk::Button::builder()
            .label(self.t("gui-pause"))
            .css_classes(["pill"])
            .build();
        let sign_out = gtk::Button::builder()
            .label(self.t("gui-sign-out"))
            .css_classes(["flat"])
            .build();
        buttons.append(&open);
        buttons.append(&pause);
        buttons.append(&sign_out);
        let note = gtk::Label::builder()
            .label(self.t("gui-read-only"))
            .wrap(true)
            .justify(gtk::Justification::Center)
            .css_classes(["dim-label", "caption"])
            .build();
        page.set_child(Some(&buttons));
        content.append(&page);
        content.append(&note);

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
        let this = Rc::downgrade(self);
        sign_out.connect_clicked(move |_| {
            if let Some(this) = this.upgrade() {
                this.sign_out();
            }
        });
        *self.status_view.borrow_mut() = Some(StatusView {
            page: page.clone(),
            pause,
        });
        self.shown.set(None);
        self.nav.replace(&[self.page("ike loud", &content)]);
        self.tick();
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
            view.page.set_icon_name(Some(&badge.icon_name()));
            view.page.set_title(&title);
            view.page.set_description(Some(&detail));
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
