//! Notifications through the freedesktop notification service, on the
//! session bus.

use std::collections::HashMap;
use std::time::Duration;

use tracing::debug;
use zbus::Connection;
use zbus::zvariant::Value;

use crate::notification::Urgency;

/// The application name notifications are shown under.
const APP_NAME: &str = "QDP";

/// The icon of notifications: the theme's icon for remote desktops.
const APP_ICON: &str = "preferences-desktop-remote-desktop";

/// The `replaces_id` of a notification that replaces none.
const NEW_NOTIFICATION: u32 = 0;

/// The hint naming a notification's urgency.
const URGENCY_HINT: &str = "urgency";

/// The specification's normal urgency level.
const URGENCY_NORMAL: u8 = 1;

/// The specification's critical urgency level.
const URGENCY_CRITICAL: u8 = 2;

/// The freedesktop notification service.
#[zbus::proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications",
    gen_blocking = false
)]
trait Notifications {
    /// Shows a notification, returning its ID.
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, Value<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;

    /// Takes a notification away.
    fn close_notification(&self, id: u32) -> zbus::Result<()>;
}

/// A notification the service shows.
#[derive(Debug)]
pub(crate) struct Notification {
    /// The service, on the connection that showed the notification.
    service: NotificationsProxy<'static>,
    /// The notification's ID.
    id: u32,
}

impl Notification {
    /// Takes the notification away.
    pub(crate) async fn close(self) {
        // The notification may already be gone: the user closed it, or it
        // expired.
        if let Err(error) = self.service.close_notification(self.id).await {
            debug!(%error, "cannot close the notification");
        }
    }
}

/// Shows a notification titled `summary` saying `body`.
pub(crate) async fn notify(
    summary: &str,
    body: &str,
    lifetime: Duration,
    urgency: Urgency,
) -> Result<Notification, String> {
    let unavailable = |error: zbus::Error| format!("cannot show a notification: {error}");
    let connection = Connection::session().await.map_err(unavailable)?;
    let service = NotificationsProxy::new(&connection)
        .await
        .map_err(unavailable)?;
    let level = match urgency {
        Urgency::Normal => URGENCY_NORMAL,
        Urgency::Critical => URGENCY_CRITICAL,
    };
    let hints = HashMap::from([(URGENCY_HINT, Value::U8(level))]);
    let timeout = i32::try_from(lifetime.as_millis()).unwrap_or(i32::MAX);
    let id = service
        .notify(
            APP_NAME,
            NEW_NOTIFICATION,
            APP_ICON,
            summary,
            &escape(body),
            &[],
            hints,
            timeout,
        )
        .await
        .map_err(unavailable)?;
    Ok(Notification { service, id })
}

/// Returns `text` with the characters the service may read as markup
/// escaped, so that it shows as written.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Markup in a body shows as text.
    #[test]
    fn escapes_markup() {
        assert_eq!(
            escape("<b>Tom & Jerry</b>"),
            "&lt;b&gt;Tom &amp; Jerry&lt;/b&gt;"
        );
    }
}
