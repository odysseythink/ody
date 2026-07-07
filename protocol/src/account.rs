/// Account state returned by a model provider before it is adapted to an app-facing wire type.
///
/// Deprecated: the system no longer models accounts. The only remaining variant (`ApiKey`)
/// is kept temporarily for the app-server-protocol migration in Part 3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderAccount {
    ApiKey,
}
