//! Ticket compound-document contract, independent of the Ticket service and business events.
//!
//! The versioned root map includes `ticket-id`, `title`, `initial` and `posts`. `initial` is a map
//! of `author` and `body`; `posts` is an ordered sequence keyed by stable post IDs, each targeting
//! an `author`/`body` map. Order is never sorted for hash stability. All text is complete UTF-8,
//! including comments, Unicode and original line endings. Empty bodies and titles are preserved.
//!
//! Ticket identity deliberately participates in content: equal text on different Tickets yields
//! different roots. It is a business identity, not a content ID. Restoration requires the expected
//! identity and cannot retarget content to a different Ticket. Original authors participate in the
//! snapshot; authenticated history editors and edit times belong to a separate History record.
//!
//! These local types exclude state, assignment, relations, evidence and the existing content_digest.
//! Restoration returns only a document candidate; it cannot rewind business events or replace the
//! existing API's CAS. A caller restoring an old root must append a new history with current parent
//! and authenticated editor facts, and adopt content under its own business authorization/CAS.

use std::collections::BTreeSet;

use content_history::{ContentId, Object, ObjectId, Tree, TreeId};

use crate::{Builder, Error, Snapshot, entry, field, fields, read_tree, text, tree_id};

const FORMAT: &str = "ticket-document/v1";

/// Content-only view. `initial_author` is the original author, not the latest history editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub ticket_id: String,
    pub title: String,
    pub initial_body: String,
    pub initial_author: String,
    pub posts: Vec<Post>,
}

/// Stable business identity and original authorship survive edits and restoration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Post {
    pub id: String,
    pub author: String,
    pub body: String,
}

impl Document {
    fn validate(&self) -> Result<(), Error> {
        if self.ticket_id.is_empty() {
            return Err(Error::Structure("empty Ticket identity"));
        }
        let mut ids = BTreeSet::new();
        for post in &self.posts {
            if post.id.is_empty() || !ids.insert(&post.id) {
                return Err(Error::Structure("empty or duplicate post identity"));
            }
        }
        Ok(())
    }
}

fn message(builder: &mut Builder, author: &str, body: &str) -> Result<TreeId, Error> {
    let author = builder.blob(author);
    let body = builder.blob(body);
    builder.map(vec![entry("author", author), entry("body", body)])
}

/// Prepare a candidate with explicit root components and semantically ordered posts.
pub fn prepare(document: &Document) -> Result<Snapshot, Error> {
    document.validate()?;
    let mut builder = Builder::default();
    let initial = message(
        &mut builder,
        &document.initial_author,
        &document.initial_body,
    )?;
    let mut posts = Vec::new();
    for post in &document.posts {
        let content = message(&mut builder, &post.author, &post.body)?;
        posts.push(entry(&post.id, ContentId::Tree(content)));
    }
    let posts = builder.tree(Tree::sequence(posts)?);
    let format = builder.blob(FORMAT);
    let identity = builder.blob(&document.ticket_id);
    let title = builder.blob(&document.title);
    let root = builder.map(vec![
        entry("format", format),
        entry("ticket-id", identity),
        entry("title", title),
        entry("initial", ContentId::Tree(initial)),
        entry("posts", ContentId::Tree(posts)),
    ])?;
    Ok(builder.finish(root))
}

/// Restore content only and verify the intended business identity before returning any document.
pub fn restore(
    root: TreeId,
    expected_ticket_id: &str,
    mut read: impl FnMut(ObjectId) -> Result<Object, Error>,
) -> Result<Document, Error> {
    let root = read_tree(&mut read, root, false)?;
    fields(&root, &["format", "ticket-id", "title", "initial", "posts"])?;
    if text(&mut read, field(&root, "format")?)? != FORMAT {
        return Err(Error::Format);
    }
    let ticket_id = text(&mut read, field(&root, "ticket-id")?)?;
    if ticket_id != expected_ticket_id {
        return Err(Error::TicketIdentity);
    }
    let title = text(&mut read, field(&root, "title")?)?;
    let (initial_author, initial_body) = restore_message(&mut read, field(&root, "initial")?)?;
    let posts = read_tree(&mut read, tree_id(field(&root, "posts")?)?, true)?;
    let mut restored = Vec::new();
    for post in posts.entries() {
        let (author, body) = restore_message(&mut read, post.target)?;
        restored.push(Post {
            id: post.key.clone(),
            author,
            body,
        });
    }
    let document = Document {
        ticket_id,
        title,
        initial_body,
        initial_author,
        posts: restored,
    };
    document.validate()?;
    Ok(document)
}

fn restore_message(
    read: &mut impl FnMut(ObjectId) -> Result<Object, Error>,
    target: ContentId,
) -> Result<(String, String), Error> {
    let message = read_tree(read, tree_id(target)?, false)?;
    fields(&message, &["author", "body"])?;
    Ok((
        text(read, field(&message, "author")?)?,
        text(read, field(&message, "body")?)?,
    ))
}
