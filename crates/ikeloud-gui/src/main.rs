//! ike loud: the app.
//!
//! First run: sign in (password, then the SMS code), choose a new or empty
//! folder, start the daemon. Afterwards: the status window and the
//! saxophone in the tray. The app signs in itself and hands the session to
//! the daemon through `ikeloud_config::Paths::session`; the password is
//! never stored.
//!
//! Plain GTK 4, no libadwaita: the app follows the desktop's GTK theme
//! (Breeze on KDE Plasma, Adwaita on GNOME), and the folder dialog is the
//! desktop's own through the portal (decision 0022).

mod daemon;
mod icons;
mod tray;

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use gtk::prelude::*;
use gtk::{gio, glib};

use ikeloud_api::{ApiError, Client, HttpsTransport, SavedSession, SignIn, TrustedPhone};
use ikeloud_config::{
    FolderProblem, JournalLine, Paths, PendingChanges, Request, Settings, State, Status,
    check_folder, load_journal, protected_folder,
};
use ikeloud_i18n::{Message, Translations, Value, languages_from_env};
use ikeloud_keyring::SessionStore;

use crate::daemon::{DaemonProcess, EXIT_SIGNED_OUT};
use crate::icons::{APP_ID, Badge};
use crate::tray::{MenuItem, Tray, TrayAction};

type SharedClient = Arc<Mutex<Option<Client<HttpsTransport>>>>;

fn main() -> glib::ExitCode {
    // `--hidden` (autostart after login): only the tray icon, no window.
    let hidden = std::env::args().skip(1).any(|a| a == "--hidden");
    let app = gtk::Application::builder().application_id(APP_ID).build();
    app.connect_activate(move |app| {
        // A second start only shows the window of the first.
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        match Paths::from_env() {
            Ok(paths) => App::start(app, paths, hidden),
            Err(_) => app.quit(),
        }
    });
    // GTK must not see our own option.
    let program: Vec<String> = std::env::args().take(1).collect();
    app.run_with_args(&program)
}

/// Widgets of the status page.
struct StatusView {
    icon: gtk::Image,
    title: gtk::Label,
    detail: gtk::Label,
    pause: gtk::Button,
    /// Read-only or two-way, as the daemon runs now.
    note: gtk::Label,
    /// A mass change waiting for the user: what it does and the button.
    confirm: gtk::Box,
    confirm_list: gtk::Label,
    /// Id of the mass change shown (the confirmation names it).
    confirm_id: RefCell<Option<String>>,
}

/// A page of the window.
struct Page {
    title: String,
    content: gtk::Widget,
    /// The page shows the settings button in the header bar.
    settings: bool,
}

/// Pages in a stack with a back button: the header bar is shared.
struct Nav {
    stack: gtk::Stack,
    title: gtk::Label,
    back: gtk::Button,
    settings: gtk::Button,
    journal: gtk::Button,
    history: RefCell<Vec<(String, String, bool)>>,
    next: Cell<u32>,
}

impl Nav {
    fn show(&self, page: Page, push: bool) {
        let name = format!("page{}", self.next.get());
        self.next.set(self.next.get().wrapping_add(1));
        self.stack.add_named(&page.content, Some(&name));
        self.stack.set_visible_child_name(&name);
        let mut history = self.history.borrow_mut();
        if !push {
            for (old, _, _) in history.drain(..) {
                if let Some(child) = self.stack.child_by_name(&old) {
                    self.stack.remove(&child);
                }
            }
        }
        history.push((name, page.title, page.settings));
        drop(history);
        self.refresh();
    }

    /// Replaces all pages with `page`.
    fn replace(&self, page: Page) {
        self.show(page, false);
    }

    /// Shows `page` on top; Back returns.
    fn push(&self, page: Page) {
        self.show(page, true);
    }

    fn pop(&self) {
        let mut history = self.history.borrow_mut();
        if history.len() < 2 {
            return;
        }
        if let Some((name, _, _)) = history.pop() {
            if let Some((below, _, _)) = history.last() {
                self.stack.set_visible_child_name(below);
            }
            if let Some(child) = self.stack.child_by_name(&name) {
                self.stack.remove(&child);
            }
        }
        drop(history);
        self.refresh();
    }

    fn refresh(&self) {
        let history = self.history.borrow();
        if let Some((_, title, settings)) = history.last() {
            self.title.set_label(title);
            self.settings.set_visible(*settings);
            self.journal.set_visible(*settings);
        }
        self.back.set_visible(history.len() > 1);
    }
}

/// Short messages at the bottom of the window.
struct Toasts {
    revealer: gtk::Revealer,
    label: gtk::Label,
    serial: Cell<u32>,
}

struct App {
    app: gtk::Application,
    window: gtk::ApplicationWindow,
    toasts: Rc<Toasts>,
    nav: Nav,
    paths: Paths,
    /// The signed-in session: the keyring, or the private file.
    sessions: SessionStore,
    texts: Option<Translations>,
    client: SharedClient,
    phone: RefCell<Option<TrustedPhone>>,
    folder: RefCell<PathBuf>,
    daemon: RefCell<DaemonProcess>,
    paused: Cell<bool>,
    tray: RefCell<Option<Tray>>,
    status_view: RefCell<Option<StatusView>>,
    shown: Cell<Option<(Badge, String)>>,
    /// The state a notification was sent for (no repeat while it lasts).
    notified: Cell<Option<State>>,
    /// Newest conflict copy already told about (journal time).
    conflicts_seen: Cell<u64>,
    _hold: gio::ApplicationHoldGuard,
}

impl App {
    fn start(app: &gtk::Application, paths: Paths, hidden: bool) {
        let texts = Translations::new(&languages_from_env()).ok();
        let text = |key: &'static str| match &texts {
            Some(t) => t.text(&Message::new(key)),
            None => key.to_string(),
        };
        // Before the window: it looks its icon up when it is built.
        let icon_dir = paths
            .state_db
            .parent()
            .unwrap_or(Path::new("/tmp"))
            .join("icons");
        let icons_ready = icons::install(&icon_dir).is_ok();
        let _ = icons::install_app_icon(&glib::user_data_dir());
        if icons_ready && let Some(display) = gtk::gdk::Display::default() {
            gtk::IconTheme::for_display(&display).add_search_path(&icon_dir);
        }
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .default_width(460)
            .default_height(520)
            .title("ike loud")
            .icon_name(APP_ID)
            .build();

        let header = gtk::HeaderBar::new();
        let title = gtk::Label::builder()
            .label("ike loud")
            .css_classes(["title"])
            .build();
        header.set_title_widget(Some(&title));
        let back = gtk::Button::builder()
            .icon_name("go-previous-symbolic")
            .tooltip_text(text("gui-back"))
            .visible(false)
            .build();
        let settings = gtk::Button::builder()
            .icon_name("emblem-system-symbolic")
            .tooltip_text(text("gui-settings"))
            .visible(false)
            .build();
        let journal = gtk::Button::builder()
            .icon_name("document-open-recent-symbolic")
            .tooltip_text(text("gui-journal"))
            .visible(false)
            .build();
        journal.update_property(&[gtk::accessible::Property::Label(&text("gui-journal"))]);
        header.pack_start(&back);
        header.pack_end(&settings);
        header.pack_end(&journal);
        window.set_titlebar(Some(&header));

        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::SlideLeftRight)
            .build();
        let toast_label = gtk::Label::builder().wrap(true).build();
        let toast_box = gtk::Box::builder()
            .accessible_role(gtk::AccessibleRole::Alert)
            .css_classes(["app-notification"])
            .margin_bottom(12)
            .build();
        toast_box.append(&toast_label);
        let revealer = gtk::Revealer::builder()
            .child(&toast_box)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::End)
            .transition_type(gtk::RevealerTransitionType::SlideUp)
            .build();
        let overlay = gtk::Overlay::builder().child(&stack).build();
        overlay.add_overlay(&revealer);
        window.set_child(Some(&overlay));
        let toasts = Rc::new(Toasts {
            revealer,
            label: toast_label,
            serial: Cell::new(0),
        });
        let nav = Nav {
            stack,
            title,
            back: back.clone(),
            settings: settings.clone(),
            journal: journal.clone(),
            history: RefCell::new(Vec::new()),
            next: Cell::new(0),
        };

        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let paths_journal = paths.journal.clone();
        let sessions = SessionStore::new(&paths);
        let this = Rc::new(Self {
            app: app.clone(),
            window,
            toasts,
            nav,
            paths,
            sessions,
            texts,
            client: Arc::new(Mutex::new(None)),
            phone: RefCell::new(None),
            folder: RefCell::new(home.join("iCloud")),
            daemon: RefCell::new(DaemonProcess::default()),
            paused: Cell::new(false),
            tray: RefCell::new(None),
            status_view: RefCell::new(None),
            shown: Cell::new(None),
            notified: Cell::new(None),
            conflicts_seen: Cell::new(newest_conflict(&load_journal(&paths_journal))),
            _hold: app.hold(),
        });

        let weak = Rc::downgrade(&this);
        back.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.nav.pop();
            }
        });
        let weak = Rc::downgrade(&this);
        settings.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.nav.push(this.settings_page());
            }
        });
        let weak = Rc::downgrade(&this);
        journal.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.nav.push(this.journal_page());
            }
        });
        // A click on a notification shows the window.
        let show = gio::SimpleAction::new("show", None);
        let weak = Rc::downgrade(&this);
        show.connect_activate(move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.window.present();
            }
        });
        app.add_action(&show);

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
        let session = this
            .sessions
            .load()
            .and_then(|t| SavedSession::from_json(&t));
        match (settings.folder, session) {
            (Some(folder), Some(_)) => {
                *this.folder.borrow_mut() = folder;
                if settings.paused {
                    this.paused.set(true);
                } else {
                    this.start_daemon();
                }
                this.show_status();
            }
            (None, Some(_)) => this.nav.replace(this.folder_page()),
            (_, None) => this.nav.replace(this.signin_page()),
        }

        let weak = Rc::downgrade(&this);
        glib::timeout_add_seconds_local(1, move || match weak.upgrade() {
            Some(app) => {
                app.tick();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        // Hidden only when there is a tray icon to come back through, and
        // nothing waits for the user (setup, sign-in).
        let needs_user = this.status_view.borrow().is_none();
        if !hidden || this.tray.borrow().is_none() || needs_user {
            this.window.present();
        }
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

    /// Shows `text` at the bottom of the window for a few seconds.
    fn toast(&self, text: &str) {
        let toasts = &self.toasts;
        toasts.label.set_label(text);
        toasts.revealer.set_reveal_child(true);
        let serial = toasts.serial.get().wrapping_add(1);
        toasts.serial.set(serial);
        let weak = Rc::downgrade(toasts);
        glib::timeout_add_seconds_local_once(5, move || {
            if let Some(toasts) = weak.upgrade()
                && toasts.serial.get() == serial
            {
                toasts.revealer.set_reveal_child(false);
            }
        });
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

    /// A page: `content` centered at a readable width, scrolling if needed.
    fn page(&self, title: &str, content: &gtk::Box) -> Page {
        self.page_with(title, content, false)
    }

    fn page_with(&self, title: &str, content: &gtk::Box, settings: bool) -> Page {
        content.set_width_request(360);
        content.set_halign(gtk::Align::Center);
        content.set_margin_top(18);
        content.set_margin_bottom(24);
        content.set_margin_start(18);
        content.set_margin_end(18);
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(content)
            .build();
        Page {
            title: title.to_string(),
            content: scroll.upcast(),
            settings,
        }
    }

    /// A heading in large bold type (markup works with any GTK theme).
    fn heading(text: &str, large: bool) -> gtk::Label {
        let size = if large { "x-large" } else { "large" };
        let label = gtk::Label::builder()
            .wrap(true)
            .justify(gtk::Justification::Center)
            .build();
        label.set_markup(&format!(
            "<span size='{size}' weight='bold'>{}</span>",
            glib::markup_escape_text(text)
        ));
        label
    }

    fn intro(title: &str, text: &str) -> gtk::Box {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
        let body = gtk::Label::builder()
            .label(text)
            .wrap(true)
            .justify(gtk::Justification::Center)
            .css_classes(["dim-label"])
            .build();
        content.append(&Self::heading(title, true));
        content.append(&body);
        content
    }

    /// A labelled field: the label above the widget.
    fn field(label: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Box {
        let field = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let caption = gtk::Label::builder().label(label).xalign(0.0).build();
        caption.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(label)));
        caption.set_mnemonic_widget(Some(widget));
        // Screen readers name the field after its label.
        widget
            .upcast_ref::<gtk::Widget>()
            .update_property(&[gtk::accessible::Property::Label(label)]);
        field.append(&caption);
        field.append(widget);
        field
    }

    fn action_button(label: &str) -> gtk::Button {
        gtk::Button::builder()
            .label(label)
            .halign(gtk::Align::Center)
            .css_classes(["suggested-action"])
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
        self.sessions.save(&client.saved().to_json());
    }

    fn signin_page(self: &Rc<Self>) -> Page {
        let content = Self::intro(
            &self.t("gui-signin-title"),
            &self.t("gui-signin-description"),
        );
        let account = gtk::Entry::builder()
            .input_purpose(gtk::InputPurpose::Email)
            .activates_default(false)
            .build();
        let password = gtk::PasswordEntry::builder().show_peek_icon(true).build();
        content.append(&Self::field(&self.t("gui-account"), &account));
        content.append(&Self::field(&self.t("gui-password"), &password));
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
                let previous = this
                    .sessions
                    .load()
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
                                this.nav.push(this.code_page());
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
            move |_: &gtk::PasswordEntry| submit(&button)
        };
        password.connect_activate(on_enter);
        button.connect_clicked(submit);
        self.page(&self.t("gui-signin-title"), &content)
    }

    fn code_page(self: &Rc<Self>) -> Page {
        let phone = self.phone.borrow().clone();
        let ending: String = phone
            .as_ref()
            .map(|p| p.masked.chars().filter(char::is_ascii_digit).collect())
            .unwrap_or_default();
        let description =
            self.message(&Message::new("gui-code-description").arg("phone", Value::Text(ending)));
        let content = Self::intro(&self.t("gui-code-title"), &description);
        let code = gtk::Entry::builder()
            .input_purpose(gtk::InputPurpose::Digits)
            .max_length(8)
            .build();
        content.append(&Self::field(&self.t("gui-code"), &code));
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
            move |_: &gtk::Entry| submit(&verify)
        };
        code.connect_activate(on_enter);
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
                self.start_daemon();
                self.show_status();
            }
            None => self.nav.replace(self.folder_page()),
        }
    }

    fn folder_page(self: &Rc<Self>) -> Page {
        let content = Self::intro(
            &self.t("gui-folder-title"),
            &self.t("gui-folder-description"),
        );
        // The path can be typed or pasted, or picked in the desktop's own
        // folder dialog (KDE's on Plasma, GNOME's on GNOME, via the portal).
        let row = gtk::Entry::builder()
            .text(self.folder.borrow().to_string_lossy().as_ref())
            .hexpand(true)
            .build();
        let choose = gtk::Button::builder()
            .label(self.t("gui-folder-choose"))
            .build();
        row.update_property(&[gtk::accessible::Property::Label(&self.t("gui-folder"))]);
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        line.append(&row);
        line.append(&choose);
        content.append(&Self::field(&self.t("gui-folder"), &line));
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
            let this = this.clone();
            row.connect_changed(move |row| {
                let Some(this) = this.upgrade() else { return };
                *this.folder.borrow_mut() = folder_from_text(&row.text());
                check(&this);
            });
        }
        {
            let row = row.clone();
            choose.connect_clicked(move |_| {
                let Some(this) = this.upgrade() else { return };
                let dialog = gtk::FileDialog::builder()
                    .title(this.t("gui-folder"))
                    .modal(true)
                    .build();
                let row = row.clone();
                dialog.select_folder(Some(&this.window), gio::Cancellable::NONE, move |result| {
                    // Setting the text checks the folder (connect_changed).
                    if let Ok(path) = result.map(|file| file.path()) {
                        row.set_text(&path.unwrap_or_default().to_string_lossy());
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
                    // Only the folder changes; other settings stay.
                    Settings {
                        folder: Some(folder.clone()),
                        ..Settings::load(&this.paths.settings).unwrap_or_default()
                    }
                    .save(&this.paths.settings)
                });
                match saved {
                    Ok(()) => {
                        this.start_daemon();
                        this.show_status();
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
            Err(FolderProblem::NotAbsolute) => Err(self.t("gui-folder-absolute")),
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
    /// destructive)`; `on_answer` gets the id (or "cancel"). The dialog is
    /// GTK's, drawn in the desktop's GTK theme.
    fn confirm(
        self: &Rc<Self>,
        heading: &str,
        body: &str,
        answers: &[(&str, &'static str, bool)],
        on_answer: impl FnOnce(&Rc<Self>, &str) + 'static,
    ) {
        let mut labels = vec![self.t("gui-cancel")];
        let mut ids = vec!["cancel".to_string()];
        for (id, key, _) in answers {
            labels.push(self.t(key));
            ids.push((*id).to_string());
        }
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        let dialog = gtk::AlertDialog::builder()
            .message(heading)
            .detail(body)
            .modal(true)
            .cancel_button(0)
            .default_button(0)
            .build();
        dialog.set_buttons(&labels);
        let weak = Rc::downgrade(self);
        dialog.choose(Some(&self.window), gio::Cancellable::NONE, move |answer| {
            let id = answer
                .ok()
                .and_then(|index| usize::try_from(index).ok())
                .and_then(|index| ids.get(index).cloned())
                .unwrap_or_else(|| "cancel".to_string());
            if let Some(this) = weak.upgrade() {
                on_answer(&this, &id);
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
        let title = Self::heading(&self.t("gui-state-starting"), false);
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
            .halign(gtk::Align::Center)
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

        // Shown only while a mass change waits for the user.
        let confirm = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .visible(false)
            .build();
        let confirm_list = gtk::Label::builder()
            .wrap(true)
            .xalign(0.0)
            .selectable(true)
            .build();
        let confirm_button = gtk::Button::builder()
            .label(self.t("gui-confirm-changes"))
            .halign(gtk::Align::Center)
            .css_classes(["destructive-action"])
            .build();
        confirm.append(&confirm_list);
        confirm.append(&confirm_button);
        content.append(&confirm);
        let this = Rc::downgrade(self);
        confirm_button.connect_clicked(move |_| {
            let Some(this) = this.upgrade() else { return };
            let id = this
                .status_view
                .borrow()
                .as_ref()
                .and_then(|v| v.confirm_id.borrow().clone());
            if let Some(id) = id {
                match Request::Confirm(id).save(&this.paths.request) {
                    Ok(()) => this.toast(&this.t("gui-confirm-sent")),
                    Err(e) => this.toast(&e.to_string()),
                }
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
            note,
            confirm,
            confirm_list,
            confirm_id: RefCell::new(None),
        });
        self.shown.set(None);
        self.nav.replace(self.page_with("ike loud", &content, true));
        self.tick();
    }

    /// Settings: the synced folder and the account.
    fn settings_page(self: &Rc<Self>) -> Page {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 24);

        let folder_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let folder_row = gtk::Entry::builder()
            .text(self.folder.borrow().to_string_lossy().as_ref())
            .hexpand(true)
            .build();
        let apply = gtk::Button::builder().label(self.t("gui-apply")).build();
        let change = gtk::Button::builder()
            .label(self.t("gui-folder-change"))
            .build();
        folder_row.update_property(&[gtk::accessible::Property::Label(&self.t("gui-folder"))]);
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        line.append(&folder_row);
        line.append(&apply);
        line.append(&change);
        folder_box.append(&Self::field(&self.t("gui-folder"), &line));
        folder_box.append(
            &gtk::Label::builder()
                .label(self.t("gui-folder-change-description"))
                .wrap(true)
                .xalign(0.0)
                .css_classes(["dim-label"])
                .build(),
        );
        content.append(&folder_box);

        let account = self
            .sessions
            .load()
            .and_then(|t| SavedSession::from_json(&t))
            .map(|s| s.account_name().to_string())
            .unwrap_or_default();
        let account_label = gtk::Label::builder()
            .label(account)
            .xalign(0.0)
            .hexpand(true)
            .selectable(true)
            .build();
        let sign_out = gtk::Button::builder()
            .label(self.t("gui-sign-out"))
            .css_classes(["destructive-action"])
            .build();
        let account_line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        account_line.append(&account_label);
        account_line.append(&sign_out);
        content.append(&Self::field(&self.t("gui-account"), &account_line));

        // Uploads: off by default while writing is in beta (decision 0023).
        let upload = gtk::Switch::builder()
            .active(
                Settings::load(&self.paths.settings)
                    .map(|s| s.upload)
                    .unwrap_or(false),
            )
            .valign(gtk::Align::Center)
            .build();
        upload.update_property(&[gtk::accessible::Property::Label(
            &self.t("gui-upload-toggle"),
        )]);
        let upload_label = gtk::Label::builder()
            .label(self.t("gui-upload-toggle"))
            .xalign(0.0)
            .hexpand(true)
            .wrap(true)
            .build();
        let upload_line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        upload_line.append(&upload_label);
        upload_line.append(&upload);
        let upload_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        upload_box.append(&upload_line);
        upload_box.append(
            &gtk::Label::builder()
                .label(self.t("gui-upload-description"))
                .wrap(true)
                .xalign(0.0)
                .css_classes(["dim-label"])
                .build(),
        );
        content.append(&upload_box);
        let this = Rc::downgrade(self);
        upload.connect_state_set(move |switch, on| {
            let Some(this) = this.upgrade() else {
                return glib::Propagation::Stop;
            };
            if !on {
                this.set_upload(false);
                switch.set_state(false);
                return glib::Propagation::Stop;
            }
            // Turning uploads on is asked once more, with what it means.
            let switch = switch.clone();
            this.confirm(
                &this.t("gui-upload-toggle"),
                &this.t("gui-upload-confirm"),
                &[("on", "gui-upload-turn-on", false)],
                move |this, answer| {
                    let on = answer == "on";
                    if on {
                        this.set_upload(true);
                    }
                    switch.set_state(on);
                    switch.set_active(on);
                },
            );
            glib::Propagation::Stop
        });

        let this = Rc::downgrade(self);
        let apply_typed = {
            let row = folder_row.clone();
            move || {
                if let Some(this) = this.upgrade() {
                    this.change_folder(&folder_from_text(&row.text()), &row);
                }
            }
        };
        {
            let apply_typed = apply_typed.clone();
            folder_row.connect_activate(move |_| apply_typed());
        }
        apply.connect_clicked(move |_| apply_typed());
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
    fn change_folder(self: &Rc<Self>, folder: &Path, row: &gtk::Entry) {
        let old = self.folder.borrow().clone();
        if folder == old.as_path() {
            return;
        }
        let count = match self.folder_verdict(folder, Some(&old)) {
            Ok(count) => count,
            Err(text) => {
                self.toast(&text);
                row.set_text(&old.to_string_lossy());
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
                } else {
                    row.set_text(&old.to_string_lossy());
                }
            },
        );
    }

    fn apply_folder_change(
        self: &Rc<Self>,
        folder: &Path,
        old: &Path,
        trash_old: bool,
        row: &gtk::Entry,
    ) {
        let status = Status::load(&self.paths.status);
        let was_paused = self.paused.get();
        self.daemon
            .borrow_mut()
            .stop(status.as_ref(), &self.paths.request);
        let resume = |this: &Rc<Self>| {
            if !was_paused {
                this.start_daemon();
            }
            this.tick();
        };
        if let Err(text) = self.empty_into_trash(folder) {
            self.toast(&text);
            row.set_text(&old.to_string_lossy());
            resume(self);
            return;
        }
        let saved = std::fs::create_dir_all(folder).and_then(|()| {
            Settings {
                folder: Some(folder.to_path_buf()),
                ..Settings::load(&self.paths.settings).unwrap_or_default()
            }
            .save(&self.paths.settings)
        });
        if let Err(e) = saved {
            self.toast(&format!("{}: {e}", self.t("gui-folder-error")));
            row.set_text(&old.to_string_lossy());
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
        row.set_text(&folder.to_string_lossy());
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

    /// The first changes of a waiting mass change, one per line.
    fn pending_text(&self, pending: &PendingChanges) -> String {
        const SHOWN: usize = 8;
        let mut lines: Vec<String> = pending
            .files
            .iter()
            .take(SHOWN)
            .map(|change| {
                let what = match change.kind.as_str() {
                    "trash-remote" => self.t("gui-kind-trash-remote"),
                    "overwrite-remote" => self.t("gui-kind-overwrite-remote"),
                    "trash-local" => self.t("gui-kind-trash-local"),
                    "overwrite-local" => self.t("gui-kind-overwrite-local"),
                    "move-remote" => self.t("gui-kind-move-remote"),
                    _ => self.t("gui-kind-move-local"),
                };
                match &change.to {
                    Some(to) => format!("{what}: {} → {to}", change.path),
                    None => format!("{what}: {}", change.path),
                }
            })
            .collect();
        let more = pending.files.len().saturating_sub(SHOWN);
        if more > 0 {
            lines.push(self.message(&Message::new("gui-more-changes").arg(
                "count",
                Value::Number(u64::try_from(more).unwrap_or(u64::MAX)),
            )));
        }
        lines.join("\n")
    }

    /// Notifications only when the user is needed (spec): a mass change to
    /// confirm, an expired sign-in, an error, a new conflict copy. Each once.
    fn notify(&self, state: Option<State>, detail: &str) {
        let wanted = match state {
            Some(s @ (State::NeedsConfirmation | State::SignedOut | State::Error)) => Some(s),
            _ => None,
        };
        if wanted != self.notified.get() {
            self.app.withdraw_notification("state");
            if let Some(state) = wanted {
                let title = match state {
                    State::NeedsConfirmation => self.t("gui-state-needs-confirmation"),
                    State::SignedOut => self.t("gui-state-signed-out"),
                    _ => self.t("gui-state-error"),
                };
                let n = gio::Notification::new(&format!("ike loud: {title}"));
                n.set_body(Some(detail));
                n.set_priority(gio::NotificationPriority::High);
                n.set_default_action("app.show");
                self.app.send_notification(Some("state"), &n);
            }
            self.notified.set(wanted);
        }
        let journal = load_journal(&self.paths.journal);
        let newest = newest_conflict(&journal);
        if newest > self.conflicts_seen.get() {
            let fresh: Vec<&JournalLine> = journal
                .iter()
                .filter(|l| l.kind == "conflict-copy" && l.time > self.conflicts_seen.get())
                .collect();
            if let Some(first) = fresh.first() {
                let n = gio::Notification::new(&self.t("gui-notify-conflict-title"));
                n.set_body(Some(
                    &self.message(
                        &Message::new("gui-notify-conflict")
                            .arg(
                                "name",
                                Value::Text(
                                    first.target.clone().unwrap_or_else(|| first.path.clone()),
                                ),
                            )
                            .arg(
                                "count",
                                Value::Number(u64::try_from(fresh.len()).unwrap_or(u64::MAX)),
                            ),
                    ),
                ));
                n.set_default_action("app.show");
                self.app.send_notification(Some("conflict"), &n);
            }
            self.conflicts_seen.set(newest);
        }
    }

    /// The latest changes the daemon carried out, newest first.
    fn journal_page(self: &Rc<Self>) -> Page {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let lines = load_journal(&self.paths.journal);
        if lines.is_empty() {
            content.append(
                &gtk::Label::builder()
                    .label(self.t("gui-journal-empty"))
                    .css_classes(["dim-label"])
                    .build(),
            );
        }
        for line in &lines {
            let when = glib::DateTime::from_unix_local(i64::try_from(line.time).unwrap_or(0))
                .and_then(|t| t.format("%e. %-m. %H:%M"))
                .map(|t| t.to_string())
                .unwrap_or_default();
            let what = self.journal_kind(&line.kind);
            let file = match &line.target {
                Some(target) => format!("{} → {target}", line.path),
                None => line.path.clone(),
            };
            let row = gtk::Box::new(gtk::Orientation::Vertical, 0);
            row.append(
                &gtk::Label::builder()
                    .label(format!("{when} · {what}"))
                    .xalign(0.0)
                    .css_classes(["dim-label", "caption"])
                    .build(),
            );
            row.append(
                &gtk::Label::builder()
                    .label(file)
                    .xalign(0.0)
                    .wrap(true)
                    .wrap_mode(gtk::pango::WrapMode::WordChar)
                    .selectable(true)
                    .build(),
            );
            row.set_margin_bottom(6);
            content.append(&row);
        }
        self.page(&self.t("gui-journal"), &content)
    }

    fn journal_kind(&self, kind: &str) -> String {
        let key = match kind {
            "uploaded" => "gui-journal-uploaded",
            "downloaded" => "gui-journal-downloaded",
            "trashed-remote" => "gui-journal-trashed-remote",
            "trashed-local" => "gui-journal-trashed-local",
            "moved-remote" => "gui-journal-moved-remote",
            "moved-local" => "gui-journal-moved-local",
            "conflict-copy" => "gui-journal-conflict-copy",
            "folder-created" => "gui-journal-folder-created",
            "overwritten-remote" => "gui-journal-overwritten-remote",
            "overwritten-local" => "gui-journal-overwritten-local",
            "mass-change-confirmed" => "gui-journal-mass-change-confirmed",
            _ => return kind.to_string(),
        };
        self.t(key)
    }

    /// Saves the upload setting; the daemon reads it after its next run.
    fn set_upload(&self, upload: bool) {
        let saved = Settings {
            upload,
            ..Settings::load(&self.paths.settings).unwrap_or_default()
        }
        .save(&self.paths.settings);
        if let Err(e) = saved {
            self.toast(&e.to_string());
        }
        self.shown.set(None);
    }

    fn start_daemon(&self) {
        self.paused.set(false);
        // Starting means syncing again: a remembered pause ends (the daemon
        // would otherwise exit at once).
        let settings = Settings::load(&self.paths.settings).unwrap_or_default();
        if settings.paused {
            let _ = Settings {
                paused: false,
                ..settings
            }
            .save(&self.paths.settings);
        }
        let status = Status::load(&self.paths.status);
        let running = self.daemon.borrow_mut().running(status.as_ref());
        if !running {
            // A status left by an earlier daemon (e.g. "signed out" before
            // this sign-in) must not be read as the new one's.
            let _ = Status::now(State::Starting, "", 0, 0).save(&self.paths.status);
        }
        if let Err(e) = self.daemon.borrow_mut().start(status.as_ref()) {
            self.toast(&e.to_string());
        }
    }

    fn toggle_pause(self: &Rc<Self>) {
        let pause = !self.paused.get();
        // Remembered: a pause lasts across restarts and logins.
        let _ = Settings {
            paused: pause,
            ..Settings::load(&self.paths.settings).unwrap_or_default()
        }
        .save(&self.paths.settings);
        if pause {
            let status = Status::load(&self.paths.status);
            self.daemon
                .borrow_mut()
                .stop(status.as_ref(), &self.paths.request);
            self.paused.set(true);
        } else {
            self.start_daemon();
        }
        self.tick();
    }

    fn sign_out(self: &Rc<Self>) {
        let status = Status::load(&self.paths.status);
        self.daemon
            .borrow_mut()
            .stop(status.as_ref(), &self.paths.request);
        self.sessions.delete();
        let _ = std::fs::remove_file(&self.paths.status);
        if let Ok(mut guard) = self.client.lock() {
            *guard = None;
        }
        *self.status_view.borrow_mut() = None;
        self.paused.set(false);
        self.nav.replace(self.signin_page());
        self.tick();
    }

    /// Opens the synced folder in the desktop's file manager. Asked over
    /// D-Bus (`org.freedesktop.FileManager1`, Dolphin and Nautilus provide
    /// it), so the file manager runs in the session with the user's settings
    /// and theme, not as a child of this app with its environment. Without
    /// that service: GTK's launcher (the portal, or the default handler).
    fn open_folder(&self) {
        let file = gio::File::for_path(&*self.folder.borrow());
        let uri = file.uri().to_string();
        let window = self.window.clone();
        let fallback = move || {
            gtk::FileLauncher::new(Some(&file)).launch(
                Some(&window),
                gio::Cancellable::NONE,
                |_| {},
            );
        };
        let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else {
            fallback();
            return;
        };
        bus.call(
            Some("org.freedesktop.FileManager1"),
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1",
            "ShowFolders",
            Some(&(vec![uri], String::new()).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            5000,
            gio::Cancellable::NONE,
            move |result| {
                if result.is_err() {
                    fallback();
                }
            },
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
        self.notify(state, &detail);

        if let Some(tray) = self.tray.borrow().as_ref() {
            tray.update(badge, &title);
            tray.set_menu(self.menu());
        }
        let upload_on = Settings::load(&self.paths.settings)
            .map(|s| s.upload)
            .unwrap_or(false);
        let note = if status.as_ref().is_some_and(|s| s.writes) {
            "gui-two-way"
        } else if upload_on {
            "gui-upload-waiting"
        } else {
            "gui-read-only"
        };
        let pending_id = PendingChanges::load(&self.paths.pending).map(|p| p.id);
        let key = (badge, format!("{title}\n{detail}\n{note}\n{pending_id:?}"));
        let previous = self.shown.replace(Some(key.clone()));
        if previous.as_ref() == Some(&key) {
            return;
        }
        if let Some(view) = self.status_view.borrow().as_ref() {
            view.icon.set_icon_name(Some(&badge.icon_name()));
            view.title.set_markup(&format!(
                "<span size='large' weight='bold'>{}</span>",
                glib::markup_escape_text(&title)
            ));
            view.detail.set_label(&detail);
            view.detail.set_visible(!detail.is_empty());
            view.pause
                .set_label(&self.t(if paused { "gui-resume" } else { "gui-pause" }));
            view.note.set_label(&self.t(note));
            let pending = (state == Some(State::NeedsConfirmation) && !paused)
                .then(|| PendingChanges::load(&self.paths.pending))
                .flatten();
            view.confirm.set_visible(pending.is_some());
            if let Some(pending) = &pending {
                view.confirm_list.set_label(&self.pending_text(pending));
            }
            *view.confirm_id.borrow_mut() = pending.map(|p| p.id);
        }
        if state == Some(State::SignedOut) && self.status_view.borrow().is_some() {
            *self.status_view.borrow_mut() = None;
            self.nav.replace(self.signin_page());
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
        let pending = status.map(|s| s.local_pending).unwrap_or(0);
        let raw = status.map(|s| s.detail.clone()).unwrap_or_default();
        match state {
            Some(State::Idle) => {
                let mut detail = self.message(
                    &Message::new("gui-state-idle-detail")
                        .arg("files", Value::Number(files))
                        .arg("downloaded", Value::Number(downloaded)),
                );
                if pending > 0 {
                    detail.push_str("\n\n");
                    detail.push_str(&self.message(
                        &Message::new("gui-local-pending").arg("count", Value::Number(pending)),
                    ));
                }
                (self.t("gui-state-idle"), detail)
            }
            Some(State::Syncing) => (
                self.t("gui-state-syncing"),
                self.message(
                    &Message::new("gui-state-syncing-detail")
                        .arg("downloaded", Value::Number(downloaded))
                        .arg(
                            "uploaded",
                            Value::Number(status.map(|s| s.uploaded).unwrap_or(0)),
                        ),
                ),
            ),
            Some(State::NeedsConfirmation) => {
                let detail = PendingChanges::load(&self.paths.pending)
                    .map(|p| {
                        self.message(
                            &Message::new("rejected-mass-change")
                                .arg("changes", Value::Number(p.destructive))
                                .arg("moves", Value::Number(p.moves))
                                .arg("total", Value::Number(p.total)),
                        )
                    })
                    .unwrap_or(raw);
                (self.t("gui-state-needs-confirmation"), detail)
            }
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

/// A folder typed by the user: `~` stands for the home folder.
fn folder_from_text(text: &str) -> PathBuf {
    let text = text.trim();
    match text.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            home().join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(text),
    }
}

/// Time of the newest conflict copy in the journal (0: none).
fn newest_conflict(journal: &[JournalLine]) -> u64 {
    journal
        .iter()
        .filter(|l| l.kind == "conflict-copy")
        .map(|l| l.time)
        .max()
        .unwrap_or(0)
}
