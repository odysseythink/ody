//! Metadata describing the per-provider configuration fields that the TUI
//! (and other callers) can render as an editable form.

use crate::config::DatabaseProviderName;

/// Kind of value a configuration field accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderConfigFieldKind {
    /// A password or other secret that should be masked while typing.
    Password,
    /// A free-form string value.
    String,
    /// An unsigned 16-bit integer in the inclusive range `[min, max]`.
    U16 { min: u16, max: u16 },
}

/// One editable field for a database provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderConfigField {
    /// Dotted key used in `DatabaseConnectionConfig` / `options`.
    pub key: &'static str,
    /// Human-readable label shown in the form.
    pub label: &'static str,
    /// Short hint describing what the field does and its allowed values.
    pub description: &'static str,
    /// What kind of input the field expects.
    pub kind: ProviderConfigFieldKind,
    /// Whether the field must have a non-empty value before saving.
    pub required: bool,
}

/// Return the ordered list of editable fields for a database provider.
pub fn provider_config_fields(name: DatabaseProviderName) -> Vec<ProviderConfigField> {
    let mut common = vec![
        ProviderConfigField {
            key: "host",
            label: "Host / Path",
            description: "Hostname or IP for Postgres/MySQL; absolute path for SQLite.",
            kind: ProviderConfigFieldKind::String,
            required: true,
        },
        ProviderConfigField {
            key: "port",
            label: "Port",
            description: "Server port (ignored for SQLite).",
            kind: ProviderConfigFieldKind::U16 { min: 1, max: 65535 },
            required: false,
        },
        ProviderConfigField {
            key: "database",
            label: "Database",
            description: "Database/schema name (ignored for SQLite).",
            kind: ProviderConfigFieldKind::String,
            required: false,
        },
        ProviderConfigField {
            key: "username",
            label: "Username",
            description: "Username for authentication (ignored for SQLite).",
            kind: ProviderConfigFieldKind::String,
            required: false,
        },
        ProviderConfigField {
            key: "password",
            label: "Password",
            description: "Password for authentication (ignored for SQLite).",
            kind: ProviderConfigFieldKind::Password,
            required: false,
        },
    ];

    let options = match name {
        DatabaseProviderName::Postgres => vec![
            ProviderConfigField {
                key: "sslmode",
                label: "SSL mode",
                description: "Postgres sslmode such as 'disable', 'require', 'prefer'.",
                kind: ProviderConfigFieldKind::String,
                required: false,
            },
        ],
        DatabaseProviderName::Mysql => vec![
            ProviderConfigField {
                key: "ssl_mode",
                label: "SSL mode",
                description: "MySQL SSL mode such as 'disabled', 'required'.",
                kind: ProviderConfigFieldKind::String,
                required: false,
            },
        ],
        DatabaseProviderName::Sqlite => vec![],
    };

    common.extend(options);
    common
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_provider_has_fields() {
        for name in [
            DatabaseProviderName::Postgres,
            DatabaseProviderName::Mysql,
            DatabaseProviderName::Sqlite,
        ] {
            let fields = provider_config_fields(name);
            assert!(
                !fields.is_empty(),
                "{name} should expose at least one config field"
            );
            let mut keys: Vec<_> = fields.iter().map(|f| f.key).collect();
            keys.sort();
            let deduped: Vec<_> = keys.iter().copied().collect();
            assert_eq!(
                keys, deduped,
                "{name} config field keys must be unique"
            );
        }
    }

    #[test]
    fn host_is_required_for_all_providers() {
        for name in [
            DatabaseProviderName::Postgres,
            DatabaseProviderName::Mysql,
            DatabaseProviderName::Sqlite,
        ] {
            let fields = provider_config_fields(name);
            let host = fields.iter().find(|f| f.key == "host").unwrap();
            assert!(host.required, "{name} host should be required");
        }
    }
}
