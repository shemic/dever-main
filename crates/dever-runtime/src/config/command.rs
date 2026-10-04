use super::{ADAPTER_FIELDS, Settings};
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CommandLimits {
    pub timeout_ms: u64,
    pub output_limit: usize,
}

impl Default for CommandLimits {
    fn default() -> Self {
        Self {
            timeout_ms: 30_000,
            output_limit: 1024 * 1024,
        }
    }
}

impl Settings {
    /// Deployment limits apply only to ordinary command Adapters.
    pub fn command_limits(&self, identity: &str) -> Result<CommandLimits, String> {
        let node = self
            .adapter
            .as_ref()
            .map(|raw| crate::wire::parse(raw.get()))
            .transpose()?;
        let binding = node
            .as_ref()
            .map(|node| node.object())
            .transpose()?
            .and_then(|fields| fields.get(identity));
        let command = binding
            .map(|node| node.fields(ADAPTER_FIELDS))
            .transpose()?
            .and_then(|fields| fields.get("command"));
        let limits = command
            .map(|node| serde_json::from_str::<CommandLimits>(node.raw()))
            .transpose()
            .map_err(|_| "Adapter command limits require timeout_ms and output_limit integers")?
            .unwrap_or_default();
        if !(1..=3_600_000).contains(&limits.timeout_ms) {
            return Err("Adapter command timeout_ms must be between 1 and 3600000".into());
        }
        if !(1..=8 * 1024 * 1024).contains(&limits.output_limit) {
            return Err("Adapter command output_limit must be between 1 and 8388608".into());
        }
        Ok(limits)
    }
}
