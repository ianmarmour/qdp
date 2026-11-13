//! The app's window: the computers to stream from, found on the network,
//! paired with, or reached through the organization's gateway, each a click
//! away; adding one by its address; pairing with the code a host shows;
//! signing in to the gateway, with the QR code that adds it to an
//! authenticator app the first time; and what went wrong, if anything. Each
//! stream shows in the window `qdp connect` opens.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::command;
use crate::discovery::{self, Change, FoundHost};
use crate::gateway::{self, Gateway, GatewayHost};
use crate::hosts::{AppFiles, PairedHost, Token};
use crate::key_pin::KeyPin;
use crate::qr;
use crate::rows::{self, Authentication, Row, Status, Target};
use crate::session::Session;
use crate::session_events::{CodeRequest, SessionEvent};
use crate::settings::{Clipboard, Decoder, Microphone, Resolution, Settings, Sound, Touchpad};

/// The app's ID, which names its icon and desktop entry too.
pub const APP_ID: &str = "io.github.ianmarmour.qdp";

/// The app's directory in the user's data directory.
const APP_DIRECTORY: &str = "qdp";

/// The number of digits in a pairing code (Section 4.4.9).
const CODE_DIGITS: usize = 4;

/// The number of digits in a gateway's TOTP code (Section 4.4.10).
const TOTP_DIGITS: usize = 6;

/// How long after the network could not be searched to try again.
const SEARCH_RETRY_SECONDS: u32 = 10;

/// The window's size when it opens.
const DEFAULT_SIZE: (i32, i32) = (480, 640);

/// The stack page shown while no computer is found.
const EMPTY_PAGE: &str = "empty";

/// The stack page shown while no computer is found and the network cannot
/// be searched.
const UNAVAILABLE_PAGE: &str = "unavailable";

/// The stack page of the list of computers.
const LIST_PAGE: &str = "list";

/// The response that cancels a dialog.
const CANCEL: &str = "cancel";

/// The response that pairs.
const PAIR: &str = "pair";

/// The response that adds a host.
const ADD: &str = "add";

/// The response that signs in to a gateway.
const SIGN_IN: &str = "sign-in";

/// The file of the gateway, in the app's configuration directory.
const GATEWAY_FILE: &str = "gateway";

/// The file of the user's settings, in the app's configuration directory.
const SETTINGS_FILE: &str = "settings";

/// The response that forgets a host.
const FORGET: &str = "forget";

/// The response that dismisses an alert.
const OK: &str = "ok";

/// Sets up the app's actions as it starts.
pub fn startup(application: &adw::Application) {
    // The windows take the app's icon, as the desktop entry names it.
    gtk::Window::set_default_icon_name(APP_ID);
    let about = gio::ActionEntry::builder("about")
        .activate(|application: &adw::Application, _, _| {
            let about = adw::AboutDialog::builder()
                .application_name("Quick Desktop")
                .application_icon(APP_ID)
                .developer_name("Ian Armour")
                .version(env!("CARGO_PKG_VERSION"))
                .license_type(gtk::License::Apache20)
                .website(env!("CARGO_PKG_REPOSITORY"))
                .issue_url(concat!(env!("CARGO_PKG_REPOSITORY"), "/issues"))
                .build();
            about.present(application.active_window().as_ref());
        })
        .build();
    let quit = gio::ActionEntry::builder("quit")
        .activate(|application: &adw::Application, _, _| application.quit())
        .build();
    application.add_action_entries([about, quit]);
    application.set_accels_for_action("app.quit", &["<Control>q"]);
    application.set_accels_for_action("win.add", &["<Control>n"]);
    application.set_accels_for_action("win.preferences", &["<Control>comma"]);
}

/// Shows the window, opening it the first time.
pub fn activate(application: &adw::Application) {
    match application.active_window() {
        Some(window) => window.present(),
        None => Window::open(application),
    }
}

/// Tells a session apart from the others the app runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct SessionId(u64);

/// What a running session does, which its host's row says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Connecting to the host.
    Connecting,
    /// Waiting for the code the host shows.
    Pairing,
    /// Signing in to the gateway.
    SigningIn,
    /// Streaming, in `qdp connect`'s window.
    Streaming,
    /// Connecting again after the connection was lost.
    Reconnecting,
}

impl Phase {
    /// Returns what the row says while the session does this.
    fn detail(self) -> &'static str {
        match self {
            Self::Connecting => "Connecting…",
            Self::Pairing => "Pairing…",
            Self::SigningIn => "Signing in…",
            Self::Streaming => "Streaming",
            Self::Reconnecting => "Reconnecting…",
        }
    }
}

/// A session the app runs.
#[derive(Debug)]
struct Running {
    /// What it streams from.
    target: Target,
    /// What it does.
    phase: Phase,
    /// The session.
    session: Session,
    /// The dialog asking for the host's code, or the gateway's sign-in,
    /// while it shows.
    code_dialog: Option<adw::AlertDialog>,
}

/// Whether the network is being searched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Search {
    /// It is.
    Searching,
    /// It cannot be, as when `qdp` cannot run; searching is tried again
    /// after [`SEARCH_RETRY_SECONDS`].
    Unavailable,
}

/// What the window shows, and what it runs.
#[derive(Debug)]
struct State {
    /// The hosts on the network, by name.
    found: BTreeMap<String, FoundHost>,
    /// The hosts paired with.
    paired: Vec<PairedHost>,
    /// Whether the network is being searched.
    search: Search,
    /// The running `qdp discover --watch`, if any, kept to keep its input
    /// open.
    watcher: Option<gio::Subprocess>,
    /// The sessions running.
    sessions: BTreeMap<SessionId, Running>,
    /// The ID of the next session.
    next_session: u64,
    /// The groups of rows the list shows.
    groups: Vec<adw::PreferencesGroup>,
    /// What the user chose for every stream.
    settings: Settings,
    /// The organization's gateway, if the user set one.
    gateway: Option<Gateway>,
    /// The computers reached through it.
    gateway_hosts: Vec<GatewayHost>,
}

/// The app's window.
#[derive(Debug)]
struct Window {
    /// The window.
    window: adw::ApplicationWindow,
    /// Where notices show.
    toasts: adw::ToastOverlay,
    /// The list, or what to do while it is empty.
    stack: gtk::Stack,
    /// The list's page.
    page: adw::PreferencesPage,
    /// Says that the network cannot be searched, over a list of paired
    /// hosts.
    banner: adw::Banner,
    /// The app's files.
    files: AppFiles,
    /// The file of the user's settings.
    settings_path: PathBuf,
    /// The file of the gateway.
    gateway_path: PathBuf,
    /// What it shows and runs.
    state: RefCell<State>,
}

impl Window {
    /// Opens the window, and starts searching the network.
    fn open(application: &adw::Application) {
        let header = adw::HeaderBar::new();
        let add = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("Add by Address")
            .action_name("win.add")
            .build();
        header.pack_start(&add);
        let menu = gio::Menu::new();
        menu.append(Some("Preferences"), Some("win.preferences"));
        menu.append(Some("About QDP"), Some("app.about"));
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text("Main Menu")
            .menu_model(&menu)
            .primary(true)
            .build();
        header.pack_end(&menu_button);

        let banner = adw::Banner::new("QDP can’t search the network for computers right now.");
        let page = adw::PreferencesPage::new();
        let stack = gtk::Stack::new();
        stack.add_named(
            &status_page(
                "computer-symbolic",
                "Looking for Computers",
                "Run qdp serve on a computer on this network, and it appears here.",
            ),
            Some(EMPTY_PAGE),
        );
        stack.add_named(
            &status_page(
                "network-offline-symbolic",
                "Can’t Search the Network",
                "QDP can’t search this network for computers right now. Add a computer by its address.",
            ),
            Some(UNAVAILABLE_PAGE),
        );
        stack.add_named(&page, Some(LIST_PAGE));

        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.add_top_bar(&banner);
        view.set_content(Some(&stack));
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&view));
        let window = adw::ApplicationWindow::builder()
            .application(application)
            .title("Quick Desktop")
            .default_width(DEFAULT_SIZE.0)
            .default_height(DEFAULT_SIZE.1)
            .content(&toasts)
            .build();

        let files = AppFiles::new(glib::user_data_dir().join(APP_DIRECTORY));
        let settings_path = glib::user_config_dir()
            .join(APP_DIRECTORY)
            .join(SETTINGS_FILE);
        let gateway_path = glib::user_config_dir()
            .join(APP_DIRECTORY)
            .join(GATEWAY_FILE);
        let this = Rc::new(Self {
            window,
            toasts,
            stack,
            page,
            banner,
            state: RefCell::new(State {
                found: BTreeMap::new(),
                paired: files.paired_hosts(),
                search: Search::Searching,
                watcher: None,
                sessions: BTreeMap::new(),
                next_session: 0,
                groups: Vec::new(),
                settings: Settings::load(&settings_path),
                gateway: Gateway::load(&gateway_path),
                gateway_hosts: gateway::hosts(&files.gateway_hosts()),
            }),
            files,
            settings_path,
            gateway_path,
        });
        let adding = Rc::clone(&this);
        let add_action = gio::ActionEntry::builder("add")
            .activate(move |_: &adw::ApplicationWindow, _, _| adding.add_by_address())
            .build();
        let preferring = Rc::clone(&this);
        let preferences_action = gio::ActionEntry::builder("preferences")
            .activate(move |_: &adw::ApplicationWindow, _, _| preferring.show_preferences())
            .build();
        this.window
            .add_action_entries([add_action, preferences_action]);
        this.search();
        this.window.present();
    }

    /// Starts searching the network, or notes that it cannot be, and tries
    /// again later.
    fn search(self: &Rc<Self>) {
        let changed = Rc::clone(self);
        let ended = Rc::clone(self);
        let watching = discovery::watch(
            move |change| changed.network_changed(change),
            move || ended.search_ended(),
        );
        match watching {
            Ok(process) => {
                let mut state = self.state.borrow_mut();
                state.watcher = Some(process);
                state.search = Search::Searching;
            }
            Err(_) => self.search_ended(),
        }
        self.refresh();
    }

    /// Notes that the network can no longer be searched, and tries again
    /// later.
    fn search_ended(self: &Rc<Self>) {
        {
            let mut state = self.state.borrow_mut();
            state.watcher = None;
            state.found.clear();
            state.search = Search::Unavailable;
        }
        self.refresh();
        let this = Rc::clone(self);
        glib::timeout_add_seconds_local_once(SEARCH_RETRY_SECONDS, move || this.search());
    }

    /// Notes a host found or gone.
    fn network_changed(self: &Rc<Self>, change: Change) {
        {
            let mut state = self.state.borrow_mut();
            match change {
                Change::Found(host) => {
                    state.found.insert(host.name.clone(), host);
                }
                Change::Lost(name) => {
                    state.found.remove(&name);
                }
            }
        }
        self.refresh();
    }

    /// Shows the rows as they now stand.
    fn refresh(self: &Rc<Self>) {
        let (rows, search, phases, through) = {
            let state = self.state.borrow();
            let found: Vec<_> = state.found.values().cloned().collect();
            let phases: Vec<_> = state
                .sessions
                .values()
                .map(|running| (running.target.key().to_owned(), running.phase))
                .collect();
            // The computers reached through the gateway, while one is set.
            let through = state
                .gateway
                .as_ref()
                .filter(|_| !state.gateway_hosts.is_empty())
                .map(|gateway| (gateway.address.clone(), state.gateway_hosts.clone()));
            (
                rows::rows(&found, &state.paired),
                state.search,
                phases,
                through,
            )
        };
        let phase_of = |key: &str| {
            phases
                .iter()
                .find(|(running, _)| running == key)
                .map(|(_, phase)| *phase)
        };
        for group in self.state.borrow_mut().groups.drain(..) {
            self.page.remove(&group);
        }
        self.banner
            .set_revealed(search == Search::Unavailable && !rows.is_empty());
        if rows.is_empty() && through.is_none() {
            self.stack.set_visible_child_name(match search {
                Search::Searching => EMPTY_PAGE,
                Search::Unavailable => UNAVAILABLE_PAGE,
            });
            return;
        }
        let yours = adw::PreferencesGroup::builder()
            .title("Your Computers")
            .build();
        let nearby = adw::PreferencesGroup::builder().title("Nearby").build();
        let (mut any_yours, mut any_nearby) = (false, false);
        for row in rows {
            let phase = row
                .target
                .as_ref()
                .and_then(|target| phase_of(target.key()));
            let widget = self.row_widget(row.clone(), phase);
            if row.paired.is_some() {
                yours.add(&widget);
                any_yours = true;
            } else {
                nearby.add(&widget);
                any_nearby = true;
            }
        }
        let gateway_group = adw::PreferencesGroup::new();
        if let Some((address, hosts)) = &through {
            gateway_group.set_title(&format!("Through {address}"));
            for host in hosts {
                gateway_group.add(&self.gateway_row(host.clone(), phase_of(&host.target)));
            }
        }
        let mut state = self.state.borrow_mut();
        for (group, any) in [
            (yours, any_yours),
            (gateway_group, through.is_some()),
            (nearby, any_nearby),
        ] {
            if any {
                self.page.add(&group);
                state.groups.push(group);
            }
        }
        self.stack.set_visible_child_name(LIST_PAGE);
    }

    /// Returns the widget of `host`, reached through the gateway, whose
    /// session, if one runs, does `phase`.
    fn gateway_row(self: &Rc<Self>, host: GatewayHost, phase: Option<Phase>) -> adw::ActionRow {
        let widget = adw::ActionRow::builder()
            .title(host.name.as_str())
            .subtitle(phase.map_or_else(|| host.target.clone(), |phase| phase.detail().to_owned()))
            .use_markup(false)
            .activatable(true)
            .build();
        widget.add_prefix(&gtk::Image::from_icon_name("computer-symbolic"));
        if phase.is_some_and(|phase| phase != Phase::Streaming) {
            widget.add_suffix(&gtk::Spinner::builder().spinning(true).build());
        }
        let forget = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text("Forget")
            .valign(gtk::Align::Center)
            .build();
        forget.add_css_class("flat");
        let this = Rc::clone(self);
        let forgotten = host.clone();
        forget.connect_clicked(move |_| this.confirm_forget_gateway_host(forgotten.clone()));
        widget.add_suffix(&forget);
        let this = Rc::clone(self);
        widget.connect_activated(move |_| {
            this.stream(Target {
                name: host.name.clone(),
                address: host.target.clone(),
                authentication: Authentication::Gateway,
            });
        });
        widget
    }

    /// Returns the widget of `row`, whose session, if one runs, does
    /// `phase`.
    fn row_widget(self: &Rc<Self>, row: Row, phase: Option<Phase>) -> adw::ActionRow {
        let subtitle = match phase {
            Some(phase) => phase.detail(),
            None => &row.detail,
        };
        let widget = adw::ActionRow::builder()
            .title(row.name.as_str())
            .subtitle(subtitle)
            .use_markup(false)
            .activatable(true)
            .build();
        let icon = gtk::Image::from_icon_name(match row.status {
            Status::Ready | Status::NotSeen => "computer-symbolic",
            Status::New => "list-add-symbolic",
            Status::PairingOff => "system-lock-screen-symbolic",
            Status::DifferentKey => "dialog-warning-symbolic",
        });
        if row.status == Status::NotSeen {
            icon.add_css_class("dim-label");
        }
        widget.add_prefix(&icon);
        if phase.is_some_and(|phase| phase != Phase::Streaming) {
            widget.add_suffix(&gtk::Spinner::builder().spinning(true).build());
        } else if row.status == Status::New {
            let pair = gtk::Label::new(Some("Pair"));
            pair.add_css_class("accent");
            widget.add_suffix(&pair);
        }
        if let Some(host) = row.paired.clone() {
            let forget = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text("Forget")
                .valign(gtk::Align::Center)
                .build();
            forget.add_css_class("flat");
            let this = Rc::clone(self);
            forget.connect_clicked(move |_| this.confirm_forget(host.clone()));
            widget.add_suffix(&forget);
        }
        let this = Rc::clone(self);
        widget.connect_activated(move |_| match &row.target {
            Some(target) => this.stream(target.clone()),
            None => this.explain(&row),
        });
        widget
    }

    /// Says why choosing `row` does nothing.
    fn explain(&self, row: &Row) {
        let body = match row.status {
            Status::PairingOff => format!(
                "Pairing is off on {}. Start qdp serve there with pairing on to pair.",
                row.name
            ),
            Status::DifferentKey => "This computer uses the name of one you paired with, but not its key, so QDP won’t connect to it automatically.".to_owned(),
            Status::Ready | Status::NotSeen | Status::New => {
                format!("{} can’t be reached yet.", row.name)
            }
        };
        self.alert(&row.name, &body);
    }

    /// Streams from `target`, pairing first if it asks to, unless a
    /// session with it runs already.
    fn stream(self: &Rc<Self>, target: Target) {
        let running = self
            .state
            .borrow()
            .sessions
            .values()
            .any(|running| running.target.key() == target.key());
        if running {
            self.toast(&format!("Already streaming from {}", target.name));
            return;
        }
        let id = {
            let mut state = self.state.borrow_mut();
            state.next_session += 1;
            SessionId(state.next_session)
        };
        let mut options = self.state.borrow().settings.arguments();
        if target.authentication == Authentication::Gateway {
            let Some(gateway) = self.state.borrow().gateway.clone() else {
                self.alert(
                    &format!("Can’t Reach {}", target.name),
                    "It’s reached through your organization’s gateway, which isn’t set. Set it in Preferences.",
                );
                return;
            };
            options.extend(gateway.arguments());
        }
        let this = Rc::clone(self);
        let started = Session::start(&target, &self.files.identity(), &options, move |event| {
            this.session_event(id, event);
        });
        match started {
            Ok(session) => {
                self.state.borrow_mut().sessions.insert(
                    id,
                    Running {
                        target,
                        phase: Phase::Connecting,
                        session,
                        code_dialog: None,
                    },
                );
                self.refresh();
            }
            Err(error) => self.alert(
                "Can’t Start qdp",
                &format!("QDP streams with the qdp command, which can’t be started: {error}. Install QDP, or put qdp on the PATH."),
            ),
        }
    }

    /// Acts on what session `id` reports.
    fn session_event(self: &Rc<Self>, id: SessionId, event: SessionEvent) {
        match event {
            SessionEvent::Connecting => self.set_phase(id, Phase::Connecting),
            SessionEvent::Connected => self.set_phase(id, Phase::Streaming),
            SessionEvent::Reconnecting { .. } => self.set_phase(id, Phase::Reconnecting),
            SessionEvent::PairingCode(request) => {
                self.set_phase(id, Phase::Pairing);
                self.ask_code(id, request);
            }
            SessionEvent::GatewaySignIn { request, gateway } => {
                self.set_phase(id, Phase::SigningIn);
                self.ask_sign_in(id, request, &gateway);
            }
            SessionEvent::GatewayCode {
                request,
                enrollment,
                gateway,
            } => {
                self.set_phase(id, Phase::SigningIn);
                self.ask_totp(id, request, enrollment.as_deref(), &gateway);
            }
            SessionEvent::Paired(pin) => {
                let name = {
                    let mut state = self.state.borrow_mut();
                    let Some(running) = state.sessions.get_mut(&id) else {
                        return;
                    };
                    running.target.authentication = Authentication::Paired(pin.clone());
                    running.target.name.clone()
                };
                if self.files.set_name(&pin, Some(&name)).is_err() {
                    self.toast(&format!("Paired with {name}, but can’t keep its name"));
                } else {
                    self.toast(&format!("Paired with {name}"));
                }
                self.reload_paired();
            }
            SessionEvent::Ended => {
                self.end_session(id);
            }
            SessionEvent::Failed(error) => {
                if let Some(target) = self.end_session(id) {
                    self.alert(
                        &format!("Couldn’t Stream from {}", target.name),
                        &sentence(&error),
                    );
                }
            }
        }
    }

    /// Notes that session `id` does `phase`.
    fn set_phase(self: &Rc<Self>, id: SessionId, phase: Phase) {
        if let Some(running) = self.state.borrow_mut().sessions.get_mut(&id) {
            running.phase = phase;
        }
        self.refresh();
    }

    /// Forgets session `id`, which ended, closing its dialog, and returns
    /// what it streamed from, if it was running.
    fn end_session(self: &Rc<Self>, id: SessionId) -> Option<Target> {
        let running = self.state.borrow_mut().sessions.remove(&id)?;
        if let Some(dialog) = running.code_dialog {
            dialog.force_close();
        }
        self.refresh();
        Some(running.target)
    }

    /// Asks for the code the host of session `id` shows, `request` saying
    /// whether an earlier one did not work, and answers the session.
    fn ask_code(self: &Rc<Self>, id: SessionId, request: CodeRequest) {
        let Some(name) = self
            .state
            .borrow()
            .sessions
            .get(&id)
            .map(|running| running.target.name.clone())
        else {
            return;
        };
        let body = match request {
            CodeRequest::First => format!("Type the four-digit code {name} shows."),
            CodeRequest::Retry => {
                format!("That code didn’t work. Type the new code {name} shows.")
            }
        };
        let entry = gtk::Entry::builder()
            .placeholder_text("Code")
            .input_purpose(gtk::InputPurpose::Digits)
            .max_length(i32::try_from(CODE_DIGITS).unwrap_or(i32::MAX))
            .activates_default(true)
            .xalign(0.5)
            .build();
        let dialog = adw::AlertDialog::builder()
            .heading(format!("Pair with {name}"))
            .body(body)
            .extra_child(&entry)
            .focus_widget(&entry)
            .close_response(CANCEL)
            .default_response(PAIR)
            .build();
        dialog.add_responses(&[(CANCEL, "Cancel"), (PAIR, "Pair")]);
        dialog.set_response_appearance(PAIR, adw::ResponseAppearance::Suggested);
        dialog.set_response_enabled(PAIR, false);
        let enabling = dialog.clone();
        entry.connect_changed(move |entry| {
            enabling.set_response_enabled(PAIR, is_code(&entry.text()));
        });
        self.ask(id, dialog, move |session, response| {
            if response == PAIR {
                session.send_code(entry.text().trim());
            } else {
                session.cancel();
            }
        });
    }

    /// Asks for the user's name and password for the gateway named
    /// `gateway` of session `id`, `request` saying whether an earlier
    /// attempt failed, and answers the session.
    fn ask_sign_in(self: &Rc<Self>, id: SessionId, request: CodeRequest, gateway: &str) {
        let known_user = self
            .state
            .borrow()
            .gateway
            .as_ref()
            .and_then(|gateway| gateway.user.clone())
            .unwrap_or_default();
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        list.add_css_class("boxed-list");
        let user = adw::EntryRow::builder()
            .title("User name")
            .text(known_user.as_str())
            .activates_default(true)
            .build();
        let password = adw::PasswordEntryRow::builder()
            .title("Password")
            .activates_default(true)
            .build();
        list.append(&user);
        list.append(&password);
        let focus: gtk::Widget = if known_user.is_empty() {
            user.clone().upcast()
        } else {
            password.clone().upcast()
        };
        let dialog = adw::AlertDialog::builder()
            .heading(format!("Sign In to {gateway}"))
            .body(match request {
                CodeRequest::First => "Use your organization’s account.",
                CodeRequest::Retry => "That didn’t work. Check your user name and password.",
            })
            .extra_child(&list)
            .focus_widget(&focus)
            .close_response(CANCEL)
            .default_response(SIGN_IN)
            .build();
        dialog.add_responses(&[(CANCEL, "Cancel"), (SIGN_IN, "Sign In")]);
        dialog.set_response_appearance(SIGN_IN, adw::ResponseAppearance::Suggested);
        let update = Rc::new({
            let (dialog, user, password) = (dialog.clone(), user.clone(), password.clone());
            move || {
                let filled = !user.text().trim().is_empty() && !password.text().is_empty();
                dialog.set_response_enabled(SIGN_IN, filled);
            }
        });
        update();
        for entry in [user.upcast_ref::<gtk::Editable>(), password.upcast_ref()] {
            let update = Rc::clone(&update);
            entry.connect_changed(move |_| update());
        }
        self.ask(id, dialog, move |session, response| {
            if response == SIGN_IN {
                session.send_credentials(user.text().trim(), &password.text());
            } else {
                session.cancel();
            }
        });
    }

    /// Asks for a code from the user's authenticator app for the gateway
    /// named `gateway` of session `id`, showing the QR code of
    /// `enrollment` to add it to the app first, if given, and answers the
    /// session.
    fn ask_totp(
        self: &Rc<Self>,
        id: SessionId,
        request: CodeRequest,
        enrollment: Option<&str>,
        gateway: &str,
    ) {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
        if let Some(uri) = enrollment {
            if let Some(texture) = qr::texture(uri) {
                let picture = gtk::Picture::for_paintable(&texture);
                picture.set_can_shrink(false);
                picture.set_halign(gtk::Align::Center);
                picture.set_alternative_text(Some(
                    "QR code that adds the gateway to an authenticator app",
                ));
                content.append(&picture);
            }
            // For an authenticator on this computer, or a phone without a
            // camera.
            let copy = gtk::Button::builder()
                .label("Copy Setup Link")
                .halign(gtk::Align::Center)
                .build();
            copy.add_css_class("flat");
            let (this, link) = (Rc::clone(self), uri.to_owned());
            copy.connect_clicked(move |_| {
                this.window.clipboard().set_text(&link);
                this.toast("Copied the setup link");
            });
            content.append(&copy);
        }
        let entry = gtk::Entry::builder()
            .placeholder_text("Code")
            .input_purpose(gtk::InputPurpose::Digits)
            .max_length(i32::try_from(TOTP_DIGITS).unwrap_or(i32::MAX))
            .activates_default(true)
            .xalign(0.5)
            .build();
        content.append(&entry);
        let body = match (enrollment.is_some(), request) {
            (true, CodeRequest::First) => {
                "Scan this with an authenticator app, such as Google Authenticator, Microsoft Authenticator or Authy, then type its code."
            }
            (true, CodeRequest::Retry) => {
                "That code didn’t work. Scan this with your authenticator app if you haven’t yet, then type its code."
            }
            (false, CodeRequest::First) => "Type the six-digit code your authenticator app shows.",
            (false, CodeRequest::Retry) => {
                "That code didn’t work. Type the code your authenticator app shows now."
            }
        };
        let dialog = adw::AlertDialog::builder()
            .heading(format!("Sign In to {gateway}"))
            .body(body)
            .extra_child(&content)
            .focus_widget(&entry)
            .close_response(CANCEL)
            .default_response(SIGN_IN)
            .build();
        dialog.add_responses(&[(CANCEL, "Cancel"), (SIGN_IN, "Sign In")]);
        dialog.set_response_appearance(SIGN_IN, adw::ResponseAppearance::Suggested);
        dialog.set_response_enabled(SIGN_IN, false);
        let enabling = dialog.clone();
        entry.connect_changed(move |entry| {
            enabling.set_response_enabled(SIGN_IN, is_digits(&entry.text(), TOTP_DIGITS));
        });
        self.ask(id, dialog, move |session, response| {
            if response == SIGN_IN {
                session.send_code(entry.text().trim());
            } else {
                session.cancel();
            }
        });
    }

    /// Shows `dialog` for session `id`, which closes it if it ends first,
    /// and gives the session and the response to `answer` otherwise.
    fn ask(
        self: &Rc<Self>,
        id: SessionId,
        dialog: adw::AlertDialog,
        answer: impl FnOnce(&Session, &str) + 'static,
    ) {
        if let Some(running) = self.state.borrow_mut().sessions.get_mut(&id) {
            running.code_dialog = Some(dialog.clone());
        }
        let this = Rc::clone(self);
        glib::spawn_future_local(async move {
            let response = dialog.choose_future(Some(&this.window)).await;
            let mut state = this.state.borrow_mut();
            // The session may have ended while the dialog showed.
            let Some(running) = state.sessions.get_mut(&id) else {
                return;
            };
            running.code_dialog = None;
            answer(&running.session, response.as_str());
        });
    }

    /// Asks for a host's address, and pairs with it, or adds it with the key
    /// pin and token its administrator gave.
    fn add_by_address(self: &Rc<Self>) {
        let gateway = self.state.borrow().gateway.clone();
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        list.add_css_class("boxed-list");
        let address = adw::EntryRow::builder()
            .title("Name or IP address")
            .input_purpose(gtk::InputPurpose::Url)
            .activates_default(true)
            .build();
        // Through the gateway, while one is set: by its ID there, or its
        // name inside the organization's network.
        let through = adw::SwitchRow::builder()
            .title(
                gateway
                    .as_ref()
                    .map_or_else(String::new, |gateway| format!("Through {}", gateway.address)),
            )
            .subtitle("Your organization’s gateway, by the computer’s ID there or its name inside the network")
            .active(gateway.is_some())
            .visible(gateway.is_some())
            .build();
        let with_token = adw::SwitchRow::builder()
            .title("Use a key pin and token")
            .subtitle("Instead of pairing with a code")
            .build();
        let pin = adw::EntryRow::builder()
            .title("Key pin")
            .visible(false)
            .activates_default(true)
            .build();
        let token = adw::PasswordEntryRow::builder()
            .title("Token")
            .visible(false)
            .activates_default(true)
            .build();
        let name = adw::EntryRow::builder()
            .title("Name (optional)")
            .visible(false)
            .activates_default(true)
            .build();
        list.append(&address);
        list.append(&through);
        list.append(&with_token);
        list.append(&pin);
        list.append(&token);
        list.append(&name);
        let dialog = adw::AlertDialog::builder()
            .heading("Add a Computer")
            .extra_child(&list)
            .focus_widget(&address)
            .close_response(CANCEL)
            .default_response(ADD)
            .build();
        dialog.add_responses(&[(CANCEL, "Cancel"), (ADD, "Pair")]);
        dialog.set_response_appearance(ADD, adw::ResponseAppearance::Suggested);
        dialog.set_response_enabled(ADD, false);
        let update = Rc::new({
            let (dialog, address, through, with_token, pin, token, name) = (
                dialog.clone(),
                address.clone(),
                through.clone(),
                with_token.clone(),
                pin.clone(),
                token.clone(),
                name.clone(),
            );
            move || {
                let gatewayed = through.is_active();
                let tokened = with_token.is_active() && !gatewayed;
                dialog.set_body(if gatewayed {
                    "Type its ID on your organization’s gateway, or its name inside the organization’s network."
                } else {
                    "Type its name or IP address, with :PORT if it isn’t 4433. Pair with the code it shows, or use the key pin and token its administrator gave you."
                });
                with_token.set_visible(!gatewayed);
                for row in [pin.upcast_ref::<gtk::Widget>(), token.upcast_ref()] {
                    row.set_visible(tokened);
                }
                name.set_visible(tokened || gatewayed);
                dialog.set_response_label(ADD, if tokened || gatewayed { "Add" } else { "Pair" });
                let valid = !address.text().trim().is_empty()
                    && (!tokened
                        || (typed_pin(&pin.text()).is_some()
                            && Token::new(&token.text()).is_some()));
                dialog.set_response_enabled(ADD, valid);
            }
        });
        update();
        for entry in [
            address.upcast_ref::<gtk::Editable>(),
            pin.upcast_ref(),
            token.upcast_ref(),
        ] {
            let update = Rc::clone(&update);
            entry.connect_changed(move |_| update());
        }
        for switch in [&with_token, &through] {
            let toggled = Rc::clone(&update);
            switch.connect_active_notify(move |_| toggled());
        }
        let this = Rc::clone(self);
        glib::spawn_future_local(async move {
            if dialog.choose_future(Some(&this.window)).await != ADD {
                return;
            }
            let address = address.text().trim().to_owned();
            if through.is_active() {
                let host = GatewayHost {
                    name: match name.text().trim() {
                        "" => address.clone(),
                        typed => typed.to_owned(),
                    },
                    target: address,
                };
                if let Err(error) = gateway::add_host(&this.files.gateway_hosts(), &host) {
                    this.alert(
                        &format!("Couldn’t Add {}", host.name),
                        &sentence(&error.to_string()),
                    );
                    return;
                }
                this.reload_gateway_hosts();
                this.stream(Target {
                    name: host.name,
                    address: host.target,
                    authentication: Authentication::Gateway,
                });
                return;
            }
            if !with_token.is_active() {
                this.stream(Target {
                    name: address.clone(),
                    address,
                    authentication: Authentication::Pair,
                });
                return;
            }
            let (Some(pin), Some(token)) = (typed_pin(&pin.text()), Token::new(&token.text()))
            else {
                return;
            };
            let name = match name.text().trim() {
                "" => address.clone(),
                typed => typed.to_owned(),
            };
            if let Err(error) = this.files.add_token_host(&pin, &address, &token, &name) {
                this.alert(
                    &format!("Couldn’t Add {name}"),
                    &sentence(&error.to_string()),
                );
                return;
            }
            this.reload_paired();
            this.stream(Target {
                name,
                address,
                authentication: Authentication::Token { pin, token },
            });
        });
    }

    /// Shows the settings of every stream, keeping each as it changes.
    fn show_preferences(self: &Rc<Self>) {
        let settings = self.state.borrow().settings;
        let page = adw::PreferencesPage::new();
        let video = adw::PreferencesGroup::builder()
            .title("Video")
            .description(
                "For streams started from now on. A fixed resolution streams less at a large \
                 or full-screen window; hardware decoding uses the GPU where it decodes AV1.",
            )
            .build();
        let resolutions = Resolution::choices();
        let labels: Vec<String> = resolutions.iter().map(|choice| choice.label()).collect();
        let resolution = combo_row(
            "Resolution",
            &labels.iter().map(String::as_str).collect::<Vec<_>>(),
            resolutions
                .iter()
                .position(|choice| *choice == settings.resolution),
        );
        let this = Rc::clone(self);
        resolution.connect_selected_notify(move |row| {
            if let Some(&choice) = resolutions.get(row.selected() as usize) {
                this.change_settings(|settings| settings.resolution = choice);
            }
        });
        let decoder = combo_row(
            "Decoding",
            &Decoder::CHOICES.map(Decoder::label),
            Decoder::CHOICES
                .iter()
                .position(|choice| *choice == settings.decoder),
        );
        let this = Rc::clone(self);
        decoder.connect_selected_notify(move |row| {
            if let Some(&choice) = Decoder::CHOICES.get(row.selected() as usize) {
                this.change_settings(|settings| settings.decoder = choice);
            }
        });
        video.add(&resolution);
        video.add(&decoder);

        let sound_group = adw::PreferencesGroup::builder().title("Sound").build();
        let sound = adw::SwitchRow::builder()
            .title("Play the computer’s sound")
            .active(settings.sound == Sound::On)
            .build();
        let this = Rc::clone(self);
        sound.connect_active_notify(move |row| {
            let choice = if row.is_active() {
                Sound::On
            } else {
                Sound::Off
            };
            this.change_settings(|settings| settings.sound = choice);
        });
        let microphone = adw::SwitchRow::builder()
            .title("Send the microphone")
            .subtitle("The computer’s apps hear this one’s microphone")
            .active(settings.microphone == Microphone::On)
            .build();
        let this = Rc::clone(self);
        microphone.connect_active_notify(move |row| {
            let choice = if row.is_active() {
                Microphone::On
            } else {
                Microphone::Off
            };
            this.change_settings(|settings| settings.microphone = choice);
        });
        sound_group.add(&sound);
        sound_group.add(&microphone);

        let input = adw::PreferencesGroup::builder()
            .title("Input")
            .description("The clipboard shares copied text with the computer streamed.")
            .build();
        let clipboard = combo_row(
            "Clipboard",
            &Clipboard::CHOICES.map(Clipboard::label),
            Clipboard::CHOICES
                .iter()
                .position(|choice| *choice == settings.clipboard),
        );
        let this = Rc::clone(self);
        clipboard.connect_selected_notify(move |row| {
            if let Some(&choice) = Clipboard::CHOICES.get(row.selected() as usize) {
                this.change_settings(|settings| settings.clipboard = choice);
            }
        });
        let touchpad = adw::SwitchRow::builder()
            .title("Touchpad gestures")
            .subtitle("Send the touchpad’s fingers for the computer’s own gestures; needs the input group")
            .active(settings.touchpad == Touchpad::Gestures)
            .build();
        let this = Rc::clone(self);
        touchpad.connect_active_notify(move |row| {
            let choice = if row.is_active() {
                Touchpad::Gestures
            } else {
                Touchpad::Pointer
            };
            this.change_settings(|settings| settings.touchpad = choice);
        });
        input.add(&clipboard);
        input.add(&touchpad);

        page.add(&video);
        page.add(&sound_group);
        page.add(&input);
        page.add(&self.gateway_preferences());
        let dialog = adw::PreferencesDialog::new();
        dialog.add(&page);
        dialog.present(Some(&self.window));
    }

    /// Returns the preferences of the organization's gateway, each kept as
    /// it is applied.
    fn gateway_preferences(self: &Rc<Self>) -> adw::PreferencesGroup {
        let gateway = self.state.borrow().gateway.clone();
        let group = adw::PreferencesGroup::builder()
            .title("Gateway")
            .description(
                "Your organization’s gateway, to reach its computers from outside its network. \
                 You sign in with your organization’s account and a code from an authenticator \
                 app. Ask its administrator for its address, and its key pin if they give one.",
            )
            .build();
        let entry = |title: &str, text: &str| {
            adw::EntryRow::builder()
                .title(title)
                .text(text)
                .show_apply_button(true)
                .build()
        };
        let address = entry(
            "Address",
            gateway
                .as_ref()
                .map_or("", |gateway| gateway.address.as_str()),
        );
        address.set_input_purpose(gtk::InputPurpose::Url);
        let user = entry(
            "User name",
            gateway
                .as_ref()
                .and_then(|gateway| gateway.user.as_deref())
                .unwrap_or(""),
        );
        let pin = entry(
            "Key pin (optional)",
            gateway
                .as_ref()
                .and_then(|gateway| gateway.pin.as_ref())
                .map_or("", KeyPin::as_str),
        );
        for row in [&address, &user, &pin] {
            let (this, address, user, pin) =
                (Rc::clone(self), address.clone(), user.clone(), pin.clone());
            row.connect_apply(move |_| {
                this.set_gateway(&address.text(), &user.text(), &pin.text());
            });
            group.add(row);
        }
        group
    }

    /// Keeps the gateway at `address`, signing in as `user`, with the key
    /// pin `pin` if given; forgets it if `address` is empty.
    fn set_gateway(self: &Rc<Self>, address: &str, user: &str, pin: &str) {
        let gateway = match (address.trim(), pin.trim()) {
            ("", _) => None,
            (address, "") => Some(Gateway {
                address: address.to_owned(),
                pin: None,
                user: Some(user.trim().to_owned()).filter(|user| !user.is_empty()),
            }),
            (address, pin) => {
                let Some(pin) = typed_pin(pin) else {
                    self.toast("A key pin is 64 hexadecimal digits");
                    return;
                };
                Some(Gateway {
                    address: address.to_owned(),
                    pin: Some(pin),
                    user: Some(user.trim().to_owned()).filter(|user| !user.is_empty()),
                })
            }
        };
        let kept = match &gateway {
            Some(gateway) => gateway.save(&self.gateway_path),
            None => Gateway::remove(&self.gateway_path),
        };
        if let Err(error) = kept {
            self.toast(&format!("Can’t keep the gateway: {error}"));
            return;
        }
        self.state.borrow_mut().gateway = gateway;
        self.refresh();
    }

    /// Changes the settings as `change` does, and keeps them.
    fn change_settings(&self, change: impl FnOnce(&mut Settings)) {
        let settings = {
            let mut state = self.state.borrow_mut();
            change(&mut state.settings);
            state.settings
        };
        if let Err(error) = settings.save(&self.settings_path) {
            self.toast(&format!("Can’t keep the settings: {error}"));
        }
    }

    /// Asks whether to forget `host`, and forgets it if so.
    fn confirm_forget(self: &Rc<Self>, host: PairedHost) {
        let dialog = adw::AlertDialog::new(
            Some(&format!("Forget {}?", host.name)),
            Some("Connecting to it again needs pairing with a new code."),
        );
        dialog.add_responses(&[(CANCEL, "Cancel"), (FORGET, "Forget")]);
        dialog.set_response_appearance(FORGET, adw::ResponseAppearance::Destructive);
        dialog.set_close_response(CANCEL);
        dialog.set_default_response(Some(CANCEL));
        let this = Rc::clone(self);
        glib::spawn_future_local(async move {
            if dialog.choose_future(Some(&this.window)).await != FORGET {
                return;
            }
            if host.token.is_some() {
                match this.files.remove_token_host(&host.pin) {
                    Ok(_) => this.reload_paired(),
                    Err(error) => this.alert(
                        &format!("Couldn’t Forget {}", host.name),
                        &sentence(&error.to_string()),
                    ),
                }
                return;
            }
            let identity = this.files.identity();
            let forgot = command::run(&[
                OsStr::new("hosts"),
                OsStr::new("remove"),
                OsStr::new(host.pin.as_str()),
                OsStr::new("--identity"),
                identity.as_os_str(),
            ])
            .await;
            if !forgot {
                this.alert(
                    &format!("Couldn’t Forget {}", host.name),
                    "qdp couldn’t change the hosts this computer paired with; its log says why.",
                );
                return;
            }
            // A name left behind is only overwritten when pairing again.
            let _ = this.files.set_name(&host.pin, None);
            this.reload_paired();
        });
    }

    /// Asks whether to forget `host`, reached through the gateway, and
    /// forgets it if so.
    fn confirm_forget_gateway_host(self: &Rc<Self>, host: GatewayHost) {
        let dialog = adw::AlertDialog::new(
            Some(&format!("Forget {}?", host.name)),
            Some("You can add it again by its ID or name."),
        );
        dialog.add_responses(&[(CANCEL, "Cancel"), (FORGET, "Forget")]);
        dialog.set_response_appearance(FORGET, adw::ResponseAppearance::Destructive);
        dialog.set_close_response(CANCEL);
        dialog.set_default_response(Some(CANCEL));
        let this = Rc::clone(self);
        glib::spawn_future_local(async move {
            if dialog.choose_future(Some(&this.window)).await != FORGET {
                return;
            }
            match gateway::remove_host(&this.files.gateway_hosts(), &host.target) {
                Ok(()) => this.reload_gateway_hosts(),
                Err(error) => this.alert(
                    &format!("Couldn’t Forget {}", host.name),
                    &sentence(&error.to_string()),
                ),
            }
        });
    }

    /// Reads the computers reached through the gateway again.
    fn reload_gateway_hosts(self: &Rc<Self>) {
        self.state.borrow_mut().gateway_hosts = gateway::hosts(&self.files.gateway_hosts());
        self.refresh();
    }

    /// Reads the paired hosts again, as after pairing or forgetting one.
    fn reload_paired(self: &Rc<Self>) {
        self.state.borrow_mut().paired = self.files.paired_hosts();
        self.refresh();
    }

    /// Shows `text` for a moment.
    fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::new(text));
    }

    /// Shows an alert with `heading` and `body`, until dismissed.
    fn alert(&self, heading: &str, body: &str) {
        let dialog = adw::AlertDialog::new(Some(heading), Some(body));
        dialog.add_response(OK, "OK");
        dialog.set_default_response(Some(OK));
        dialog.set_close_response(OK);
        dialog.present(Some(&self.window));
    }
}

/// Returns a page saying what to do while no computer is listed, with a
/// button to add one by its address.
fn status_page(icon: &str, title: &str, description: &str) -> adw::StatusPage {
    let add = gtk::Button::builder()
        .label("Add by Address")
        .action_name("win.add")
        .halign(gtk::Align::Center)
        .build();
    add.add_css_class("pill");
    add.add_css_class("suggested-action");
    adw::StatusPage::builder()
        .icon_name(icon)
        .title(title)
        .description(description)
        .child(&add)
        .build()
}

/// Returns `error`, as `qdp` words it, as a sentence: capitalized, and with
/// a full stop.
fn sentence(error: &str) -> String {
    let mut characters = error.trim().chars();
    let Some(first) = characters.next() else {
        return String::new();
    };
    let mut sentence: String = first.to_uppercase().chain(characters).collect();
    if !sentence.ends_with(['.', '!', '?']) {
        sentence.push('.');
    }
    sentence
}

/// Returns a combo row titled `title`, offering `choices`, with the one at
/// `selected` chosen, or the first.
fn combo_row(title: &str, choices: &[&str], selected: Option<usize>) -> adw::ComboRow {
    let row = adw::ComboRow::builder()
        .title(title)
        .model(&gtk::StringList::new(choices))
        .build();
    row.set_selected(u32::try_from(selected.unwrap_or(0)).unwrap_or(0));
    row
}

/// Reads a key pin as users paste it: 64 hexadecimal digits, perhaps with
/// colons or spaces between them.
fn typed_pin(text: &str) -> Option<KeyPin> {
    KeyPin::parse(&text.replace([':', ' '], ""))
}

/// Returns whether `text` is a whole pairing code.
fn is_code(text: &str) -> bool {
    is_digits(text, CODE_DIGITS)
}

/// Returns whether `text` is `digits` digits, as a code is.
fn is_digits(text: &str, digits: usize) -> bool {
    text.trim().len() == digits && text.trim().bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_errors_as_sentences() {
        assert_eq!(
            sentence("the host closed the connection: timed out"),
            "The host closed the connection: timed out."
        );
        assert_eq!(sentence("Already one."), "Already one.");
        assert_eq!(sentence(""), "");
    }

    #[test]
    fn knows_whole_codes() {
        assert!(is_code("0420"));
        assert!(is_code(" 0420 "));
        assert!(!is_code("042"));
        assert!(!is_code("04a0"));
        assert!(is_digits("123456", TOTP_DIGITS));
        assert!(!is_digits("12345", TOTP_DIGITS));
    }
}
