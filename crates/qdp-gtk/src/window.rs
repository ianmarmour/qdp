//! The app's window: the computers to stream from, found on the network or
//! paired with, each a click away; adding one by its address; pairing with
//! the code a host shows; and what went wrong, if anything. Each stream
//! shows in the window `qdp connect` opens.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::command;
use crate::discovery::{self, Change, FoundHost};
use crate::hosts::{AppFiles, PairedHost};
use crate::rows::{self, Authentication, Row, Status, Target};
use crate::session::Session;
use crate::session_events::{CodeRequest, SessionEvent};

/// The app's ID, which names its icon and desktop entry too.
pub const APP_ID: &str = "io.github.ianmarmour.qdp";

/// The app's directory in the user's data directory.
const APP_DIRECTORY: &str = "qdp";

/// The number of digits in a pairing code (Section 4.4.9).
const CODE_DIGITS: usize = 4;

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

/// The response that forgets a host.
const FORGET: &str = "forget";

/// The response that dismisses an alert.
const OK: &str = "ok";

/// Sets up the app's actions as it starts.
pub fn startup(application: &adw::Application) {
    let about = gio::ActionEntry::builder("about")
        .activate(|application: &adw::Application, _, _| {
            let about = adw::AboutDialog::builder()
                .application_name("QDP")
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
    /// The dialog asking for the host's code, while it shows.
    code_dialog: Option<adw::AlertDialog>,
}

/// Whether the network is being searched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Search {
    /// It is.
    Searching,
    /// It cannot be, as when Avahi does not run; searching is tried again
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
        menu.append(Some("About QDP"), Some("app.about"));
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text("Main Menu")
            .menu_model(&menu)
            .primary(true)
            .build();
        header.pack_end(&menu_button);

        let banner =
            adw::Banner::new("QDP can’t search the network for computers. Is Avahi running?");
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
                "QDP finds computers through Avahi, which isn’t running. Start it, or add a computer by its address.",
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
            .title("QDP")
            .default_width(DEFAULT_SIZE.0)
            .default_height(DEFAULT_SIZE.1)
            .content(&toasts)
            .build();

        let files = AppFiles::new(glib::user_data_dir().join(APP_DIRECTORY));
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
            }),
            files,
        });
        let adding = Rc::clone(&this);
        let add_action = gio::ActionEntry::builder("add")
            .activate(move |_: &adw::ApplicationWindow, _, _| adding.add_by_address())
            .build();
        this.window.add_action_entries([add_action]);
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
        let (rows, search, phases) = {
            let state = self.state.borrow();
            let found: Vec<_> = state.found.values().cloned().collect();
            let phases: Vec<_> = state
                .sessions
                .values()
                .map(|running| (running.target.key().to_owned(), running.phase))
                .collect();
            (rows::rows(&found, &state.paired), state.search, phases)
        };
        for group in self.state.borrow_mut().groups.drain(..) {
            self.page.remove(&group);
        }
        self.banner
            .set_revealed(search == Search::Unavailable && !rows.is_empty());
        if rows.is_empty() {
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
            let phase = row.target.as_ref().and_then(|target| {
                phases
                    .iter()
                    .find(|(key, _)| key == target.key())
                    .map(|(_, phase)| *phase)
            });
            let widget = self.row_widget(row.clone(), phase);
            if row.paired.is_some() {
                yours.add(&widget);
                any_yours = true;
            } else {
                nearby.add(&widget);
                any_nearby = true;
            }
        }
        let mut state = self.state.borrow_mut();
        for (group, any) in [(yours, any_yours), (nearby, any_nearby)] {
            if any {
                self.page.add(&group);
                state.groups.push(group);
            }
        }
        self.stack.set_visible_child_name(LIST_PAGE);
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
        let this = Rc::clone(self);
        let started = Session::start(&target, &self.files.identity(), move |event| {
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
            if response == PAIR {
                running.session.send_code(entry.text().trim());
            } else {
                running.session.cancel();
            }
        });
    }

    /// Asks for a host's address, and pairs with it.
    fn add_by_address(self: &Rc<Self>) {
        let entry = gtk::Entry::builder()
            .placeholder_text("192.168.1.5 or exile.local")
            .input_purpose(gtk::InputPurpose::Url)
            .activates_default(true)
            .build();
        let dialog = adw::AlertDialog::builder()
            .heading("Add a Computer")
            .body("Type its name or IP address, with :PORT if it isn’t 4433, and pair with the code it shows.")
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
            enabling.set_response_enabled(PAIR, !entry.text().trim().is_empty());
        });
        let this = Rc::clone(self);
        glib::spawn_future_local(async move {
            if dialog.choose_future(Some(&this.window)).await == PAIR {
                let address = entry.text().trim().to_owned();
                this.stream(Target {
                    name: address.clone(),
                    address,
                    authentication: Authentication::Pair,
                });
            }
        });
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

/// Returns whether `text` is a whole pairing code.
fn is_code(text: &str) -> bool {
    text.trim().len() == CODE_DIGITS && text.trim().bytes().all(|byte| byte.is_ascii_digit())
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
    }
}
