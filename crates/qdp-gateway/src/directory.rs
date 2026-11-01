//! Checking users against the organization's directory (Section 13.4): LDAP,
//! Active Directory included, over TLS.
//!
//! The gateway binds with its own account, finds the user with the configured
//! filter, then binds as the user with the password they typed, which the
//! directory checks with all its own rules, such as disabled and locked
//! accounts and expired passwords. Every way a user can fail looks the same
//! to the client.

use std::fs;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use ldap3::{Ldap, LdapConnAsync, LdapConnSettings, Scope, SearchEntry, ldap_escape};
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;
use rustls::{ClientConfig, RootCertStore};

use crate::config::{DirectoryConfig, USER_PLACEHOLDER};

/// The longest user name the gateway passes to the directory, in bytes.
const MAX_USER_NAME_LENGTH: usize = 255;

/// The longest password the gateway passes to the directory, in bytes.
const MAX_PASSWORD_LENGTH: usize = 1024;

/// LDAP's `invalidCredentials` result code (RFC 4511, Section 4.1.9), which
/// Active Directory also sends for locked, disabled and expired accounts.
const INVALID_CREDENTIALS: u32 = 49;

/// A user the directory authenticated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryUser {
    /// The user's name in the directory.
    pub name: String,
    /// The user's full name, or empty.
    pub display_name: String,
    /// The groups the user is in, as the directory names them: distinguished
    /// names, for Active Directory.
    pub groups: Vec<String>,
}

/// Why a user was not authenticated.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DirectoryError {
    /// The directory refused the user: an unknown name, a wrong password, or
    /// an account that may not sign in, which the client is not told apart.
    #[error("the directory refused the user")]
    Refused,
    /// The directory could not be asked.
    #[error("the directory is unavailable: {0}")]
    Unavailable(String),
}

/// A directory that checks users' passwords.
pub trait Directory: Send + Sync + 'static {
    /// Checks `password` for `user`, returning who they are.
    fn authenticate(
        &self,
        user: &str,
        password: &str,
    ) -> impl Future<Output = Result<DirectoryUser, DirectoryError>> + Send;
}

/// An LDAP directory.
pub struct LdapDirectory {
    /// How to reach it and find users in it.
    config: DirectoryConfig,
    /// The password of the gateway's own account.
    bind_password: String,
    /// The TLS configuration its certificate is checked with.
    tls: Arc<ClientConfig>,
}

impl LdapDirectory {
    /// Returns the directory `config` describes, reading the gateway's
    /// password and the certificate authorities it trusts.
    pub fn new(config: &DirectoryConfig) -> Result<Self, DirectoryError> {
        let bind_password = fs::read_to_string(&config.bind_password_file)
            .map_err(|error| {
                DirectoryError::Unavailable(format!(
                    "cannot read {}: {error}",
                    config.bind_password_file.display()
                ))
            })?
            .trim_end_matches(['\r', '\n'])
            .to_owned();
        let mut roots = RootCertStore::empty();
        match &config.ca_certificate {
            Some(path) => {
                let certificates = CertificateDer::pem_file_iter(path)
                    .and_then(Iterator::collect::<Result<Vec<_>, _>>)
                    .map_err(|error| {
                        DirectoryError::Unavailable(format!(
                            "cannot read {}: {error}",
                            path.display()
                        ))
                    })?;
                roots.add_parsable_certificates(certificates);
            }
            None => {
                roots.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
            }
        }
        if roots.is_empty() {
            return Err(DirectoryError::Unavailable(
                "no certificate authority to check the directory's certificate with".to_owned(),
            ));
        }
        let tls =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map_err(|error| DirectoryError::Unavailable(error.to_string()))?
                .with_root_certificates(roots)
                .with_no_client_auth();
        Ok(Self {
            config: config.clone(),
            bind_password,
            tls: Arc::new(tls),
        })
    }

    /// Returns how long to wait for the directory.
    fn timeout(&self) -> Duration {
        Duration::from_secs(self.config.timeout_seconds)
    }

    /// Connects to the directory.
    async fn connect(&self) -> Result<Ldap, DirectoryError> {
        let settings = LdapConnSettings::new()
            .set_conn_timeout(self.timeout())
            .set_starttls(self.config.starttls)
            .set_config(Arc::clone(&self.tls));
        let (connection, mut ldap) = LdapConnAsync::with_settings(settings, &self.config.url)
            .await
            .map_err(unavailable)?;
        ldap3::drive!(connection);
        ldap.with_timeout(self.timeout());
        Ok(ldap)
    }

    /// Checks that the gateway's own account can bind, as when checking the
    /// configuration.
    pub async fn check(&self) -> Result<(), DirectoryError> {
        let mut ldap = self.connect().await?;
        self.bind_gateway(&mut ldap).await?;
        let _ = ldap.unbind().await;
        Ok(())
    }

    /// Binds as the gateway's own account.
    async fn bind_gateway(&self, ldap: &mut Ldap) -> Result<(), DirectoryError> {
        ldap.simple_bind(&self.config.bind_dn, &self.bind_password)
            .await
            .map_err(unavailable)?
            .success()
            .map(drop)
            .map_err(|error| {
                DirectoryError::Unavailable(format!(
                    "the gateway's account {} cannot bind: {error}",
                    self.config.bind_dn
                ))
            })
    }

    /// Finds `user`'s entry, with the attributes the gateway reads.
    async fn find(&self, user: &str) -> Result<SearchEntry, DirectoryError> {
        let mut ldap = self.connect().await?;
        self.bind_gateway(&mut ldap).await?;
        let filter = self
            .config
            .user_filter
            .replace(USER_PLACEHOLDER, &ldap_escape(user));
        let attributes = [
            self.config.name_attribute.as_str(),
            self.config.display_name_attribute.as_str(),
            self.config.group_attribute.as_str(),
        ];
        let (entries, _) = ldap
            .search(&self.config.base_dn, Scope::Subtree, &filter, attributes)
            .await
            .map_err(unavailable)?
            .success()
            .map_err(unavailable)?;
        let _ = ldap.unbind().await;
        let mut entries = entries.into_iter();
        match (entries.next(), entries.next()) {
            (Some(entry), None) => Ok(SearchEntry::construct(entry)),
            (None, _) => Err(DirectoryError::Refused),
            (Some(_), Some(_)) => {
                tracing::warn!(user, "the user filter matches more than one entry");
                Err(DirectoryError::Refused)
            }
        }
    }
}

impl Directory for LdapDirectory {
    async fn authenticate(
        &self,
        user: &str,
        password: &str,
    ) -> Result<DirectoryUser, DirectoryError> {
        // An empty password makes an unauthenticated bind, which succeeds
        // whatever the name (RFC 4513, Section 5.1.2).
        if user.is_empty()
            || password.is_empty()
            || user.len() > MAX_USER_NAME_LENGTH
            || password.len() > MAX_PASSWORD_LENGTH
        {
            return Err(DirectoryError::Refused);
        }
        let entry = self.find(user).await?;
        let mut ldap = self.connect().await?;
        let bound = ldap
            .simple_bind(&entry.dn, password)
            .await
            .map_err(unavailable)?;
        let _ = ldap.unbind().await;
        match bound.rc {
            0 => {}
            INVALID_CREDENTIALS => return Err(DirectoryError::Refused),
            _ => return Err(DirectoryError::Unavailable(bound.to_string())),
        }
        let first = |attribute: &str| {
            entry
                .attrs
                .get(attribute)
                .and_then(|values| values.first())
                .cloned()
        };
        Ok(DirectoryUser {
            name: first(&self.config.name_attribute).unwrap_or_else(|| user.to_owned()),
            display_name: first(&self.config.display_name_attribute).unwrap_or_default(),
            groups: entry
                .attrs
                .get(&self.config.group_attribute)
                .cloned()
                .unwrap_or_default(),
        })
    }
}

/// Returns an LDAP error as the directory being unavailable.
fn unavailable(error: ldap3::LdapError) -> DirectoryError {
    DirectoryError::Unavailable(error.to_string())
}
