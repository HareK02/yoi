use config_source::{ConfigContentType, ConfigEntry, ConfigTreeSnapshot, VirtualPath};
use content_history_adapters::{settings, ticket};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sources = ConfigTreeSnapshot::from_entries([ConfigEntry::new(
        VirtualPath::parse("nested/main.dcdl")?,
        ConfigContentType::Decodal,
        "// preserve this comment\r\n{}\r\n",
    )?])?;
    let config = settings::prepare(&sources)?;
    assert_eq!(
        settings::restore(config.root, |id| config.get(id))?,
        sources
    );

    let document = ticket::Document {
        ticket_id: "T-example".into(),
        title: "History contract".into(),
        initial_author: "user:original".into(),
        initial_body: "Full text\r\n".into(),
        posts: vec![ticket::Post {
            id: "stable-post-id".into(),
            author: "user:other".into(),
            body: "Reply\n".into(),
        }],
    };
    let candidate = ticket::prepare(&document)?;
    assert_eq!(
        ticket::restore(candidate.root, "T-example", |id| candidate.get(id))?,
        document
    );
    // Pass candidate.root and candidate.objects to store preparation in a caller-owned transaction.
    // Do not interpret successful preparation as business adoption or change an authoritative head.
    println!(
        "config candidate: {}; Ticket candidate: {}",
        config.root, candidate.root
    );
    Ok(())
}
