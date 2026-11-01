//! Desktop notifications, which show the host's user what needs them, such
//! as the code a device pairs with (Section 4.4.9): through the freedesktop
//! notification service on Linux, which GNOME, KDE, Xfce and the
//! notification daemons of other desktops, such as mako and dunst, provide.

use std::time::Duration;

/// How much a notification needs the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Urgency {
    /// It can wait, and goes away on its own.
    Normal,
    /// It needs the user now, and stays shown.
    Critical,
}

/// A notification being shown.
#[derive(Debug)]
pub struct Notification {
    /// The platform's notification.
    #[cfg(all(target_os = "linux", feature = "notifications"))]
    inner: crate::linux::notifications::Notification,
}

impl Notification {
    /// Takes the notification away.
    pub async fn close(self) {
        #[cfg(all(target_os = "linux", feature = "notifications"))]
        self.inner.close().await;
    }
}

/// Shows a notification titled `summary` saying `body`, as plain text, for
/// `lifetime` at most.
///
/// # Errors
///
/// Fails if the desktop has no notification service, or on systems this
/// crate does not notify on yet.
pub async fn notify(
    summary: &str,
    body: &str,
    lifetime: Duration,
    urgency: Urgency,
) -> Result<Notification, String> {
    #[cfg(all(target_os = "linux", feature = "notifications"))]
    {
        crate::linux::notifications::notify(summary, body, lifetime, urgency)
            .await
            .map(|inner| Notification { inner })
    }
    #[cfg(not(all(target_os = "linux", feature = "notifications")))]
    {
        let _ = (summary, body, lifetime, urgency);
        Err("this system has no desktop notifications yet".to_owned())
    }
}
