//! Connecting through a gateway (Section 13), for `qdp connect --gateway`:
//! the terminal's sign-in, which asks for the directory password and a code
//! from the user's authenticator app, showing the QR code that adds the
//! gateway to the app the first time, and the session tokens kept in the
//! identity directory, with which later connections sign in without asking.

use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use qdp_app::qr::{QUIET_ZONE, QrModules};
use qdp_runtime::{Credentials, SignInAnswer, SignInPrompt, SignInRequest};

/// The file of session tokens in the identity directory.
const SESSIONS_FILE: &str = "gateway-sessions";

/// The terminal colours a QR code is drawn in: light modules in bright
/// white, dark ones on black, whatever the terminal's own colours.
const QR_COLOURS: &str = "\x1b[97;40m";

/// The terminal's own colours again.
const RESET_COLOURS: &str = "\x1b[0m";

/// The session tokens of the gateways a client signed in to, readable only
/// by the user: a line a gateway, with its label, a tab, when the token
/// expires in seconds since the Unix epoch, a tab, and the token in
/// hexadecimal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewaySessions {
    /// The file.
    path: PathBuf,
}

impl GatewaySessions {
    /// Returns the sessions of the client whose identity is in `directory`.
    pub fn new(directory: &Path) -> Self {
        Self {
            path: directory.join(SESSIONS_FILE),
        }
    }

    /// Returns the token for the gateway `label`, if it has not expired.
    pub fn token(&self, label: &str) -> Option<Vec<u8>> {
        let now = unix_now();
        self.load()
            .into_iter()
            .find(|entry| entry.label == label && entry.expires > now)
            .map(|entry| entry.token)
    }

    /// Keeps `token` for the gateway `label`, valid for `lifetime`, in place
    /// of its last, and drops the tokens that expired.
    pub fn keep(&self, label: &str, token: &[u8], lifetime: Duration) -> io::Result<()> {
        let now = unix_now();
        let mut entries = self.load();
        entries.retain(|entry| entry.label != label && entry.expires > now);
        entries.push(SessionEntry {
            label: label.to_owned(),
            expires: now.saturating_add(lifetime.as_secs()),
            token: token.to_vec(),
        });
        if let Some(directory) = self.path.parent() {
            fs::create_dir_all(directory)?;
        }
        let text: String = entries
            .iter()
            .map(|entry| {
                format!(
                    "{}\t{}\t{}\n",
                    entry.label,
                    entry.expires,
                    hex(&entry.token)
                )
            })
            .collect();
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        options.open(&self.path)?.write_all(text.as_bytes())
    }

    /// Returns the entries, none if there is no file; lines that are not
    /// entries are skipped.
    fn load(&self) -> Vec<SessionEntry> {
        fs::read_to_string(&self.path)
            .map(|text| text.lines().filter_map(SessionEntry::parse).collect())
            .unwrap_or_default()
    }
}

/// A gateway's session token.
#[derive(Clone, PartialEq, Eq)]
struct SessionEntry {
    /// The gateway's label.
    label: String,
    /// When the token expires, in seconds since the Unix epoch.
    expires: u64,
    /// The token.
    token: Vec<u8>,
}

impl SessionEntry {
    /// Reads one line of the file.
    fn parse(line: &str) -> Option<Self> {
        let mut fields = line.split('\t');
        let label = fields.next()?.to_owned();
        let expires = fields.next()?.parse().ok()?;
        let token = unhex(fields.next()?)?;
        Some(Self {
            label,
            expires,
            token,
        })
    }
}

/// Returns `bytes` in hexadecimal.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Reads bytes written in hexadecimal.
fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok())
        .collect()
}

/// Returns the time now, in seconds since the Unix epoch.
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Returns the prompt that signs in on the terminal, as `user` if given,
/// trying first the password in `password`, as from `QDP_PASSWORD`, if any.
pub fn terminal_prompt(user: Option<String>, password: Option<String>) -> SignInPrompt {
    SignInPrompt::new(move |request| {
        let user = user.clone();
        let password = password.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || ask(&request, user, password))
                .await
                .ok()
                .flatten()
        })
    })
}

/// Asks on the terminal what `request` asks for, or returns `None` if the
/// terminal closed.
fn ask(
    request: &SignInRequest,
    user: Option<String>,
    password: Option<String>,
) -> Option<SignInAnswer> {
    match request {
        SignInRequest::Credentials { gateway, retry } => {
            if let Some(reason) = retry {
                eprintln!("{reason}.");
            }
            eprintln!("Sign in to {gateway} with your organization's account.");
            let user = match user {
                Some(user) => user,
                None => read_line("User name: ")?,
            };
            // The password given is tried once; after a failure, the user
            // types it.
            let password = match (retry, password) {
                (None, Some(password)) => password,
                _ => rpassword::prompt_password("Password: ").ok()?,
            };
            Some(SignInAnswer::Credentials(Credentials { user, password }))
        }
        SignInRequest::Code {
            gateway,
            enrollment,
            retry,
        } => {
            if let Some(reason) = retry {
                eprintln!("{reason}.");
            }
            if let Some(uri) = enrollment {
                eprintln!(
                    "{gateway} signs you in with a code from an authenticator app, such as \
                     Google Authenticator, Microsoft Authenticator or Authy. Scan this QR code \
                     with it, or add this address to it:"
                );
                if io::stderr().is_terminal()
                    && let Some(code) = QrModules::encode(uri)
                {
                    eprint!("{}", terminal_qr(&code));
                }
                eprintln!("{uri}");
            }
            read_line("Type the code your authenticator app shows: ").map(SignInAnswer::Code)
        }
    }
}

/// Asks `question` on the terminal and returns the line typed, or `None` if
/// the terminal closed.
fn read_line(question: &str) -> Option<String> {
    eprint!("{question}");
    let _ = io::stderr().flush();
    let mut line = String::new();
    match io::stdin().lock().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line.trim().to_owned()),
    }
}

/// Draws `code` for a terminal, with its quiet zone: two rows of modules a
/// line, in half blocks, in colours of its own that phone cameras read on
/// light and dark terminals alike.
fn terminal_qr(code: &QrModules) -> String {
    let side = code.size() + 2 * QUIET_ZONE;
    let dark = |x: usize, y: usize| {
        x >= QUIET_ZONE && y >= QUIET_ZONE && code.is_dark(x - QUIET_ZONE, y - QUIET_ZONE)
    };
    let mut text = String::new();
    for row in (0..side).step_by(2) {
        text.push_str(QR_COLOURS);
        for x in 0..side {
            // The foreground draws light modules, the black background
            // shows through as dark ones.
            text.push(match (dark(x, row), dark(x, row + 1)) {
                (false, false) => '█',
                (false, true) => '▀',
                (true, false) => '▄',
                (true, true) => ' ',
            });
        }
        text.push_str(RESET_COLOURS);
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_a_token_for_each_gateway_until_it_expires() {
        let directory =
            std::env::temp_dir().join(format!("qdp-gateway-sessions-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        let sessions = GatewaySessions::new(&directory);
        assert_eq!(sessions.token("gw.corp.example.com:443"), None);
        sessions
            .keep(
                "gw.corp.example.com:443",
                &[1, 2, 0xff],
                Duration::from_secs(3600),
            )
            .unwrap();
        sessions.keep("other:443", &[9], Duration::ZERO).unwrap();
        assert_eq!(
            sessions.token("gw.corp.example.com:443"),
            Some(vec![1, 2, 0xff])
        );
        // An expired token is not used.
        assert_eq!(sessions.token("other:443"), None);
        sessions
            .keep("gw.corp.example.com:443", &[7], Duration::from_secs(3600))
            .unwrap();
        assert_eq!(sessions.token("gw.corp.example.com:443"), Some(vec![7]));
        assert_eq!(sessions.load().len(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(directory.join(SESSIONS_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn draws_qr_codes_with_their_quiet_zone() {
        let code =
            QrModules::encode("otpauth://totp/Example:alice?secret=JBSWY3DPEHPK3PXP").unwrap();
        let drawn = terminal_qr(&code);
        let lines: Vec<&str> = drawn.lines().collect();
        let side = code.size() + 2 * QUIET_ZONE;
        assert_eq!(lines.len(), side.div_ceil(2));
        // The quiet zone's rows are light, top and bottom.
        let first = lines[0]
            .trim_start_matches(QR_COLOURS)
            .trim_end_matches(RESET_COLOURS);
        assert_eq!(first, "█".repeat(side));
        // The finder pattern's dark corner starts after the quiet zone.
        let corner = lines[QUIET_ZONE / 2]
            .trim_start_matches(QR_COLOURS)
            .chars()
            .nth(QUIET_ZONE);
        assert_eq!(corner, Some(' '));
    }
}
