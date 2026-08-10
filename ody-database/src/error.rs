//! Database errors.
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error("database query failed: {0}")]
    Query(String),
    #[error("database connection failed: {0}")]
    Connection(String),
    #[error("unknown database provider: {0}")]
    UnknownProvider(String),
    #[error("connection '{0}' is not configured")]
    ConnectionNotConfigured(String),
}

impl DatabaseError {
    pub fn user_message(&self) -> String {
        format!("{self}")
    }
}
