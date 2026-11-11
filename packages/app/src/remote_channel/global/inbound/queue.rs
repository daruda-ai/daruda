//! One retained command pump per connection; worker status never waits on I/O.

use super::{ClaimResult, Command, RemoteChannels, Served, live_generation, serve_command};
use crate::remote_channel::{
    runtime::{WorkerEvent, WorkerPayload},
    transport,
};
use daruda_store::inbox::{ChannelKey, InboxId, InboxStore};
use futures::{
    StreamExt,
    channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded},
};
use gpui::App;
use std::{
    collections::{HashMap, VecDeque},
    rc::Rc,
};

enum Work {
    Command(Command),
    Reopened(std::sync::Arc<super::BotSession>),
}

struct ConnectionQueue {
    generation: uuid::Uuid,
    sender: UnboundedSender<Work>,
    /// Retained so replacing/removing a connection cancels its obsolete work.
    _task: gpui::Task<()>,
}

pub(super) fn serve_events<F: Future<Output = ClaimResult> + 'static>(
    mut receiver: UnboundedReceiver<WorkerEvent>,
    claim: impl Fn(&InboxStore, ChannelKey, InboxId) -> F + 'static,
    cx: &mut App,
) {
    let claim = Rc::new(claim);
    cx.spawn(async move |cx| {
        let mut queues: HashMap<String, ConnectionQueue> = HashMap::new();
        while let Some(event) = receiver.next().await {
            let live = cx.update(|cx| {
                queues.retain(|id, queue| live_generation(id, queue.generation, cx));
                live_generation(&event.id, event.generation, cx)
            });
            if !live {
                continue;
            }
            let work = match event.payload {
                WorkerPayload::Status(status) => {
                    cx.update(|cx| {
                        if let Some(connection) = cx
                            .global_mut::<RemoteChannels>()
                            .connections
                            .get_mut(&event.id)
                        {
                            connection.status = status;
                            if !status.holds_claim() {
                                connection.revoke_bot();
                            }
                        }
                    });
                    continue;
                }
                WorkerPayload::Ready(bot) => {
                    cx.update(|cx| {
                        let changed = cx
                            .global_mut::<RemoteChannels>()
                            .connections
                            .get_mut(&event.id)
                            .is_some_and(|connection| connection.publish_bot(bot.clone()));
                        if changed {
                            crate::remote_channel::offer_unseen_cards_again(cx);
                        }
                        super::super::outbound::flush_edits(cx);
                    });
                    Work::Reopened(bot)
                }
                WorkerPayload::Incoming(incoming) => Work::Command(Command::Incoming(incoming)),
                WorkerPayload::Home(home) => {
                    crate::remote_channel::trace::delivery("home.received", || {
                        format!("connection={} event_id={}", event.id, home.event_id)
                    });
                    match home.kind {
                        // A refresh, safe to repeat, so it skips the inbox
                        // claim and the connection's command queue.
                        transport::HomeEventKind::Opened => {
                            cx.update(|cx| {
                                crate::remote_channel::home::driver::opened(
                                    &event.id,
                                    &home.user_id,
                                    &home.team_id,
                                    cx,
                                )
                            });
                            continue;
                        }
                        // Acts on a pane, so it is claimed and kept in order
                        // with the connection's other commands.
                        transport::HomeEventKind::Tap { .. } => {
                            Work::Command(Command::HomeTap(home))
                        }
                    }
                }
            };
            let queue = queues.entry(event.id.clone()).or_insert_with(|| {
                let (sender, receiver) = unbounded();
                let claim = claim.clone();
                let id = event.id.clone();
                let generation = event.generation;
                let task = cx.update(|cx| spawn_connection(id, generation, receiver, claim, cx));
                ConnectionQueue {
                    generation,
                    sender,
                    _task: task,
                }
            });
            if let Err(error) = queue.sender.unbounded_send(work) {
                crate::remote_channel::log_error(
                    "Remote incoming queue closed",
                    &error,
                    "remote.queue.inbound",
                );
            }
        }
    })
    .detach();
}

fn spawn_connection<F: Future<Output = ClaimResult> + 'static>(
    id: String,
    generation: uuid::Uuid,
    mut receiver: UnboundedReceiver<Work>,
    claim: Rc<impl Fn(&InboxStore, ChannelKey, InboxId) -> F + 'static>,
    cx: &mut App,
) -> gpui::Task<()> {
    cx.spawn(async move |cx| {
        let mut held = VecDeque::new();
        let mut waiting_for_inbox = false;
        while let Some(work) = receiver.next().await {
            match work {
                Work::Command(command) => held.push_back(command),
                Work::Reopened(bot) => {
                    // A reconnect may have queued this wake before a claim
                    // failed. Only the writer currently serving the connection
                    // can release commands held for inbox recovery.
                    if !cx.update(|cx| super::live_session(&id, generation, &bot, cx)) {
                        continue;
                    }
                    waiting_for_inbox = false;
                }
            }
            if waiting_for_inbox {
                continue;
            }
            while let Some(command) = held.pop_front() {
                if let Served::Retry(command) =
                    serve_command(&id, generation, command, claim.as_ref(), cx).await
                {
                    held.push_front(command);
                    waiting_for_inbox = true;
                    break;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests;
