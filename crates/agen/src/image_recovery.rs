//! Recovery is driven by provider rejection, never a locally chosen image limit.
use crate::{History, Item, Role, ToolResultDisposition, llm_client::Request, tool::Attachment};

pub(crate) const REJECTION_MESSAGE: &str = "ViewImage image omitted from model input after the provider rejected image size. This is a recovery candidate, not proof that this particular image caused the rejection. Resize or crop the image and retry ViewImage; do not read the unchanged image again.";

/// Keep the most recent response's tool results eligible, including early tool
/// results interleaved with that response's later text/calls. Empty boundaries
/// from older versions' failed requests are not evidence of accepted input.
pub(crate) fn accepted_input_end<'a>(items: impl Iterator<Item = &'a Item>) -> Option<usize> {
    let mut response_start = None;
    let mut accepted_prefix = None;
    for (index, item) in items.enumerate() {
        match item {
            Item::AssistantResponseBoundary { .. } => response_start = Some(index),
            Item::Message {
                role: Role::Assistant,
                ..
            }
            | Item::ToolCall { .. }
            | Item::Reasoning { .. } => {
                accepted_prefix = Some(response_start.unwrap_or(index));
            }
            Item::Message {
                role: Role::User, ..
            } => response_start = None,
            _ => {}
        }
    }
    accepted_prefix
}

/// Select only a ViewImage result actually sent in the rejected request. The
/// tool contract returns exactly one image, so each correction removes one
/// image and preserves the tool call/result pair. Other tools and user uploads
/// are deliberately not rewritten by this recovery policy.
pub(crate) fn largest_candidate<A>(
    history: &History<A>,
    request: &Request,
) -> Option<(usize, usize)> {
    let start = accepted_input_end(history.items()).unwrap_or(0);
    let mut largest = None;
    for (index, entry) in history.entries().iter().enumerate().skip(start) {
        let Item::ToolResult {
            call_id,
            attachments,
            ..
        } = &entry.item
        else {
            continue;
        };
        let [Attachment::Image(image)] = attachments.as_slice() else {
            continue;
        };
        let from_view_image = history.items().any(|item| {
            matches!(
                item,
                Item::ToolCall { call_id: id, name, .. } if id == call_id && name == "ViewImage"
            )
        });
        if !from_view_image {
            continue;
        }
        let sent = request.items.iter().position(|item| {
            matches!(
                item,
                Item::ToolResult { call_id: id, attachments: sent, .. }
                    if id == call_id && sent == attachments
            )
        });
        let Some(request_index) = sent else { continue };
        // Header inspection only; no raster allocation or size threshold.
        // Unknown dimensions sort by bytes as a deterministic fallback.
        let pixels = imagesize::blob_size(image.data())
            .ok()
            .map(|size| (size.width as u64).saturating_mul(size.height as u64))
            .unwrap_or(0);
        let rank = (pixels, image.data().len(), index);
        if largest
            .as_ref()
            .is_none_or(|(previous, _)| rank > *previous)
        {
            largest = Some((rank, (index, request_index)));
        }
    }
    largest.map(|(_, candidate)| candidate)
}

pub(crate) fn rejected_result(item: &Item) -> Item {
    let mut item = item.clone();
    if let Item::ToolResult {
        summary,
        content,
        attachments,
        disposition,
        is_error,
        ..
    } = &mut item
    {
        *summary = REJECTION_MESSAGE.to_owned();
        *content = None;
        attachments.clear();
        *disposition = ToolResultDisposition::Error;
        *is_error = true;
    }
    item
}
