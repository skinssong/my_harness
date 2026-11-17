//! What the agent may do unasked.
//!
//! One question — given a tool's name, may the loop run it, ask first, or
//! refuse? — and three answers. The table is pure: no registry, no I/O, no
//! model, so the whole security surface is tested the way the projection is.
//!
//! The gate sits in the loop rather than inside `Registry::dispatch`, because
//! "may this run" is a question about the *run*, and `resume` has to answer it
//! from a recorded decision rather than by asking again. Dispatch stays what it
//! was: the one place a call is executed.

/// The default table's two environment overrides for each direction. Listing a
/// tool here does not remove the defaults; it changes what that one tool does,
/// and deny beats ask beats allow when a tool appears in more than one.
pub const ALLOW_VAR: &str = "LLM_TOOLS_ALLOW";
pub const ASK_VAR: &str = "LLM_TOOLS_ASK";
pub const DENY_VAR: &str = "LLM_TOOLS_DENY";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    /// Run it.
    Allow,
    /// Stop the run and wait for a decision.
    Ask,
    /// Refuse it; the model is told and carries on.
    Deny,
}

impl Permission {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Ask => "ask",
            Self::Deny => "deny",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    rules: Vec<(String, Permission)>,
}

impl Default for Policy {
    /// Reading is allowed, writing and running are asked about. A default that
    /// asks about everything is a default nobody leaves on.
    fn default() -> Self {
        Self {
            rules: vec![
                ("read_file".to_string(), Permission::Allow),
                ("list_dir".to_string(), Permission::Allow),
                ("write_file".to_string(), Permission::Ask),
                ("bash".to_string(), Permission::Ask),
            ],
        }
    }
}

impl Policy {
    /// The default table with the three environment variables applied on top:
    /// allow, then ask, then deny, so the most restrictive listing wins.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let mut policy = Self::default();
        for (name, permission) in [
            (ALLOW_VAR, Permission::Allow),
            (ASK_VAR, Permission::Ask),
            (DENY_VAR, Permission::Deny),
        ] {
            let Some(list) = get(name) else { continue };
            for tool in list.split(',') {
                let tool = tool.trim();
                if !tool.is_empty() {
                    policy.set(tool, permission);
                }
            }
        }
        policy
    }

    pub fn set(&mut self, tool: &str, permission: Permission) {
        match self.rules.iter_mut().find(|(name, _)| name == tool) {
            Some((_, slot)) => *slot = permission,
            None => self.rules.push((tool.to_string(), permission)),
        }
    }

    /// Anything unlisted is refused. A policy that has to name every tool in
    /// order to stop it is a policy that stops nothing the day a tool is added.
    pub fn decide(&self, tool: &str) -> Permission {
        self.rules
            .iter()
            .find(|(name, _)| name == tool)
            .map_or(Permission::Deny, |(_, permission)| *permission)
    }

    pub fn rules(&self) -> &[(String, Permission)] {
        &self.rules
    }
}
