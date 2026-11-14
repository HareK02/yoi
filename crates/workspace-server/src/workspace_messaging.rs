//! Connection-owned messaging tasks. Commit notifications are hints: every delivered page
//! is reread under current Account/Workspace/Subject membership. Subscribe-before-read and
//! saved cursors close the snapshot race; bounded overflow requests explicit resynchronization.
use crate::server::{WorkerOperationContext, WorkspaceApi};
use axum::extract::ws::Message;
use protocol::subscription::{SubscriptionFrame, SubscriptionFramePayload};
use server_api::{
    MessagingCommand, MessagingEvent, MessagingRequest, MessagingResponse, MessagingResult,
    MessagingSelector, MessagingTermination,
};
use std::collections::HashMap;
use tokio::{sync::mpsc, task::JoinHandle};

#[derive(Default)]
pub(crate) struct MessagingLane {
    subscriptions: HashMap<String, JoinHandle<()>>,
    commands: Vec<JoinHandle<()>>,
    next_id: u64,
}
impl Drop for MessagingLane {
    fn drop(&mut self) {
        for task in self.subscriptions.values().chain(self.commands.iter()) {
            task.abort();
        }
    }
}
impl MessagingLane {
    pub(crate) async fn request(
        &mut self,
        api: &WorkspaceApi,
        context: &WorkerOperationContext,
        request: MessagingRequest,
        outbound: &mpsc::Sender<Message>,
        data: &mpsc::Sender<Message>,
    ) {
        self.subscriptions.retain(|_, task| !task.is_finished());
        self.commands.retain(|task| !task.is_finished());
        let MessagingRequest {
            request_id,
            command,
        } = request;
        match command {
            MessagingCommand::Unsubscribe { subscription_id } => {
                if let Some(task) = self.subscriptions.remove(&subscription_id) {
                    task.abort();
                    let _ = task.await;
                }
                let _ = response(
                    outbound,
                    request_id,
                    MessagingResult::Unsubscribed { subscription_id },
                )
                .await;
            }
            MessagingCommand::Subscribe { selector } if self.subscriptions.len() < 64 => {
                self.next_id += 1;
                let subscription_id = format!("messaging-{}", self.next_id);
                // Install the receiver synchronously, before any snapshot or queued command.
                let changes = api.messaging_changes();
                let task = tokio::spawn(subscription(
                    api.clone(),
                    context.clone(),
                    request_id,
                    subscription_id.clone(),
                    selector,
                    changes,
                    outbound.clone(),
                    data.clone(),
                ));
                self.subscriptions.insert(subscription_id, task);
            }
            command
                if self.commands.len() < 32
                    && !matches!(command, MessagingCommand::Subscribe { .. }) =>
            {
                let api = api.clone();
                let context = context.clone();
                let outbound = outbound.clone();
                self.commands.push(tokio::spawn(async move {
                    let result = api.messaging_command(&context, command).await;
                    let _ = response(&outbound, request_id, result).await;
                }));
            }
            _ => {
                let _ = response(
                    outbound,
                    request_id,
                    MessagingResult::Rejected { status: 429 },
                )
                .await;
            }
        }
    }
}
async fn response(
    outbound: &mpsc::Sender<Message>,
    request_id: String,
    result: MessagingResult,
) -> Result<(), ()> {
    send(
        outbound,
        SubscriptionFramePayload::MessagingResponse(MessagingResponse { request_id, result }),
    )
    .await
}
async fn send(
    outbound: &mpsc::Sender<Message>,
    payload: SubscriptionFramePayload,
) -> Result<(), ()> {
    let frame = SubscriptionFrame::new(payload);
    if frame.validate().is_err() {
        return Err(());
    }
    let json = serde_json::to_string(&frame).map_err(|_| ())?;
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        outbound.send(Message::Text(json.into())),
    )
    .await
    .map_err(|_| ())?
    .map_err(|_| ())
}
async fn closed(
    outbound: &mpsc::Sender<Message>,
    subscription_id: String,
    reason: MessagingTermination,
) {
    let _ = send(
        outbound,
        SubscriptionFramePayload::MessagingEvent(MessagingEvent::Closed {
            subscription_id,
            reason,
        }),
    )
    .await;
}
async fn subscription(
    api: WorkspaceApi,
    context: WorkerOperationContext,
    request_id: String,
    subscription_id: String,
    mut selector: MessagingSelector,
    mut changes: tokio::sync::broadcast::Receiver<String>,
    outbound: mpsc::Sender<Message>,
    data: mpsc::Sender<Message>,
) {
    let initial_selector = selector.clone();
    let page = match api.messaging_page(&context, &selector).await {
        Ok(page) => page,
        Err(status) => {
            let _ = response(&outbound, request_id, MessagingResult::Rejected { status }).await;
            return;
        }
    };
    let mut more = page.has_more;
    if let Some(message) = page.messages.last() {
        selector.after_sequence = message.sequence;
    }
    // The response echoes the requested cursor, not the advanced internal cursor.
    if response(
        &outbound,
        request_id,
        MessagingResult::Subscribed {
            subscription_id: subscription_id.clone(),
            selector: initial_selector,
            page,
        },
    )
    .await
    .is_err()
    {
        return;
    }
    let mut reauthorize = tokio::time::interval(std::time::Duration::from_secs(15));
    reauthorize.tick().await;
    loop {
        if !more {
            tokio::select! {
                notice = changes.recv() => match notice {
                    Ok(id) if id != selector.conversation_id => continue,
                    Ok(_) => {},
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => { closed(&outbound, subscription_id, MessagingTermination::Lagged).await; return; }
                    Err(_) => { closed(&outbound, subscription_id, MessagingTermination::ServerShutdown).await; return; }
                },
                // Current credentials and Subject deletion are revalidated even on a quiet conversation.
                _ = reauthorize.tick() => {},
            }
        }
        let page = match api.messaging_page(&context, &selector).await {
            Ok(page) => page,
            Err(_) => {
                closed(
                    &outbound,
                    subscription_id,
                    MessagingTermination::AccessRevoked,
                )
                .await;
                return;
            }
        };
        more = page.has_more;
        if let Some(message) = page.messages.last() {
            selector.after_sequence = message.sequence;
        }
        if page.messages.is_empty() {
            continue;
        }
        let frame = SubscriptionFrame::new(SubscriptionFramePayload::MessagingEvent(
            MessagingEvent::Messages {
                subscription_id: subscription_id.clone(),
                page,
            },
        ));
        if frame.validate().is_err() {
            return;
        }
        let Ok(json) = serde_json::to_string(&frame) else {
            return;
        };
        if data.try_send(Message::Text(json.into())).is_err() {
            closed(&outbound, subscription_id, MessagingTermination::Lagged).await;
            return;
        }
    }
}
