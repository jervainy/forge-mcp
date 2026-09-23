mod jwt;
pub mod oauth;

use std::collections::BTreeSet;

pub use oauth::{ALL_OAUTH_SCOPES, OAuthService};

#[derive(Debug, Clone)]
pub struct Principal {
    pub subject: String,
    pub client_id: String,
    pub scopes: BTreeSet<String>,
    pub resource_metadata_url: String,
}

impl Principal {
    pub fn has_scopes(&self, required: &[&str]) -> bool {
        required.iter().all(|scope| self.scopes.contains(*scope))
    }

    pub fn identity_key(&self) -> String {
        format!("{}:{}", self.subject, self.client_id)
    }
}

#[derive(Debug, Clone)]
pub enum AuthContext {
    Unrestricted,
    OAuth(Principal),
}

impl AuthContext {
    pub fn allows(&self, required: &[&str]) -> bool {
        match self {
            Self::Unrestricted => true,
            Self::OAuth(principal) => principal.has_scopes(required),
        }
    }

    pub fn identity_key(&self) -> Option<String> {
        match self {
            Self::Unrestricted => None,
            Self::OAuth(principal) => Some(principal.identity_key()),
        }
    }

    pub fn www_authenticate(&self, required: &[&str]) -> Option<String> {
        let Self::OAuth(principal) = self else {
            return None;
        };

        let scope = required.join(" ");
        Some(format!(
            "Bearer resource_metadata=\"{}\", error=\"insufficient_scope\", error_description=\"Additional permission is required\", scope=\"{}\"",
            principal.resource_metadata_url, scope
        ))
    }
}
