//! A switch divider. After a restart it also carries the record of the
//! conversation before the switch, shown the way a compaction shows its
//! retained summary: one folded card that discloses the text on demand.

use super::blocks::pane_markdown;
use super::card_frame::{CardFrame, CardTone};
use super::fold_header::{FoldHeader, FoldRow, edge_row};
use super::*;
use daruda_acp::handoff::{HandoffItem, HandoffParty};

pub(super) fn handoff_row(
    ix: usize,
    divider: &HandoffItem,
    key: FoldKey,
    expanded: bool,
    markdown: MarkdownRender<'_>,
    cx: &mut Context<AgentChatView>,
) -> AnyElement {
    let dim = markdown.dim;
    let edge = edge_row(
        s::agent_chat::handoff(party_label(&divider.mark.to)),
        dim,
        cx,
    );
    let Some(record) = divider.record.as_ref() else {
        return edge;
    };
    let muted = theme::dim_toward_gray(theme::agent_chat_fg_muted(cx), dim);
    let leading = div()
        .text_size(px(theme::agent_chat_font_size(cx)))
        .text_color(muted)
        .child(SharedString::from(s::agent_chat::handoff_record()));
    let header = FoldHeader::with_title(
        div()
            .text_size(px(theme::agent_chat_font_size(cx)))
            .text_color(muted)
            .child(SharedString::from(party_label(&divider.mark.from)))
            .into_any_element(),
    )
    .leading(leading.into_any_element());
    // The body runs only when the card is open, so a folded record (the usual
    // state, and up to the recap's whole budget) is never copied per frame.
    let card = FoldRow::card(
        ("agent-chat-handoff-record", ix),
        key,
        expanded,
        header,
        |cx| {
            div()
                .id(("agent-chat-handoff-record-body", ix))
                .debug_selector(move || format!("agent-chat-handoff-record-{ix}"))
                .w_full()
                .min_w_0()
                .text_size(px(theme::agent_chat_font_size(cx)))
                .text_color(muted)
                // No diagram hook: the mermaid cache rasterizes only fences the
                // live transcript holds, so a record's fences render as code.
                .child(
                    pane_markdown(
                        ("agent-chat-md-handoff-record", ix),
                        record.clone(),
                        muted,
                        markdown.states,
                        cx,
                    )
                    .link_click_handler(markdown.links.handler()),
                )
                .into_any_element()
        },
    )
    .render(dim, cx);
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(theme::GAP_SM))
        .child(
            CardFrame::new(CardTone::Standard)
                .render(dim, cx)
                .flex()
                .flex_col()
                .child(card),
        )
        .child(edge)
        .into_any_element()
}

/// The agent, and the account when the divider names one.
fn party_label(party: &HandoffParty) -> String {
    match &party.account {
        Some(account) => s::agent_chat::handoff_party(&party.agent, account),
        None => party.agent.clone(),
    }
}
