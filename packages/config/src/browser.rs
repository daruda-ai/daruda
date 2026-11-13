//! `[browser]` — the managed agent-browser tool for lane agent chats.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct BrowserConfig {
    /// Install agent-browser and offer it to local lane chat sessions.
    pub enabled: bool,
}
