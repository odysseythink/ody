use serde::Deserialize;
use serde::Serialize;
use std::borrow::Cow;
use std::cmp::Ordering;
use std::fmt;

/// Joins a namespace and a tool name into a single token.
///
/// Chat-completions tool lists have no namespace field, so a namespaced tool
/// has to travel as one flat name. Some namespaces already carry their own
/// separator — MCP's `mcp__server__` ends in `__`, and MCP's compatibility
/// namespace for builtin-named functions ends in `.` — so the separator is only
/// inserted when the namespace does not already end with one.
const FLAT_NAMESPACE_SEPARATOR: &str = "__";

/// Identifies a callable tool, preserving the namespace split when the model
/// provides one.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ToolName {
    pub name: String,
    pub namespace: Option<String>,
}

impl ToolName {
    pub fn new(namespace: Option<String>, name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            namespace,
        }
    }

    pub fn plain(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            namespace: None,
        }
    }

    pub fn namespaced(namespace: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            namespace: Some(namespace.into()),
        }
    }

    /// The single-token form of this name: `namespace__tool`.
    ///
    /// This is the name a provider without namespace support sees, so it is
    /// also the name it sends back. Keeping the derivation in one place is what
    /// makes the round trip resolvable: [`ToolName::flat`] here, a flat index in
    /// the registry there.
    pub fn flat(&self) -> Cow<'_, str> {
        let Some(namespace) = self.namespace.as_deref() else {
            return Cow::Borrowed(self.name.as_str());
        };
        // A namespace that already ends in a separator (`mcp__server__`,
        // `functions.`) is used as-is: inserting another would make the flat name
        // disagree with the one those tools have always advertised.
        if namespace.ends_with(|c: char| !c.is_alphanumeric()) {
            return Cow::Owned(format!("{namespace}{}", self.name));
        }
        Cow::Owned(format!(
            "{namespace}{FLAT_NAMESPACE_SEPARATOR}{}",
            self.name
        ))
    }
}

impl fmt::Display for ToolName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Display is what shows up in logs and duplicate-registration errors.
        // Concatenating without a separator printed `browsernavigate`, which is
        // neither the flat name a chat provider sees nor a name anyone can
        // search for.
        f.write_str(&self.flat())
    }
}

impl Ord for ToolName {
    fn cmp(&self, other: &Self) -> Ordering {
        let lhs = match &self.namespace {
            Some(namespace) => (namespace.as_str(), Some(self.name.as_str())),
            None => (self.name.as_str(), None),
        };
        let rhs = match &other.namespace {
            Some(namespace) => (namespace.as_str(), Some(other.name.as_str())),
            None => (other.name.as_str(), None),
        };
        lhs.cmp(&rhs)
    }
}

impl PartialOrd for ToolName {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl From<String> for ToolName {
    fn from(name: String) -> Self {
        Self::plain(name)
    }
}

impl From<&str> for ToolName {
    fn from(name: &str) -> Self {
        Self::plain(name)
    }
}

#[cfg(test)]
mod tests {
    use super::ToolName;

    #[test]
    fn flat_form_inserts_the_separator_for_a_bare_namespace() {
        assert_eq!(
            ToolName::namespaced("browser", "screenshot").flat(),
            "browser__screenshot"
        );
    }

    #[test]
    fn flat_form_does_not_double_an_existing_separator() {
        // MCP namespaces are already `mcp__server__`; a second separator would
        // make the flat name differ from the one MCP tools already advertise.
        assert_eq!(
            ToolName::namespaced("mcp__server__", "exec_command").flat(),
            "mcp__server__exec_command"
        );
        // The same holds for the dot-terminated namespace MCP uses when a
        // function would otherwise collide with a builtin.
        assert_eq!(
            ToolName::namespaced("functions.", "echo").flat(),
            "functions.echo"
        );
    }

    #[test]
    fn flat_form_of_a_plain_name_is_the_name() {
        assert_eq!(ToolName::plain("read_file").flat(), "read_file");
    }

    #[test]
    fn display_matches_the_flat_form() {
        assert_eq!(
            ToolName::namespaced("browser", "navigate").to_string(),
            "browser__navigate"
        );
    }
}
