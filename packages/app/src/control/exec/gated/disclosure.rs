//! What an approval card discloses about the chat it would open: the agent,
//! the permission mode it asks for and the start of its first prompt. A
//! delegated chat approves its own tools, so a card naming only a title could
//! hide what the agent is about to be told. The prompt is quoted through
//! `agent_text::prompt_excerpt`, the bound shared with agent-authored titles.

use crate::surface::strings as s;
use crate::workspace::ChatDisclosure;

/// "Claude Code (permission mode: Bypass Permissions)".
pub(super) fn agent_phrase(disclosure: &ChatDisclosure) -> String {
    s::control::approval_agent(&disclosure.agent_name, &disclosure.mode)
}
