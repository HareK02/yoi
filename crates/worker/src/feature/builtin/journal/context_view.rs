//! Model-facing context only. The original snapshot and its digest stay in history metadata.
use serde::Serialize;
use subjektiv::journal::{JournalEntryState, startup::JournalStartupSnapshot};

#[derive(Serialize)]
struct Context<'a> {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    journals: Vec<Journal<'a>>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    omitted: bool,
}

#[derive(Serialize)]
struct Journal<'a> {
    reference: String,
    #[serde(skip_serializing_if = "str::is_empty")]
    current_position: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    entries: Vec<Entry<'a>>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    omitted: bool,
}

#[derive(Serialize)]
struct Entry<'a> {
    reference: String,
    state: JournalEntryState,
    title: &'a str,
    body: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    ongoing: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    corrects: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    omitted: bool,
}

pub(super) fn yaml(snapshot: &JournalStartupSnapshot) -> Result<Option<String>, serde_yaml::Error> {
    let entry_ref =
        |journal: usize, entry: usize| format!("journal {}, entry {}", journal + 1, entry + 1);
    let journals = snapshot
        .journals
        .iter()
        .enumerate()
        .filter(|(_, journal)| {
            !journal.current_position.is_empty() || !journal.entries.is_empty() || journal.omitted
        })
        .map(|(j, journal)| Journal {
            reference: format!("journal {}", j + 1),
            current_position: &journal.current_position,
            entries: journal
                .entries
                .iter()
                .enumerate()
                .map(|(e, entry)| Entry {
                    reference: entry_ref(j, e),
                    state: entry.state,
                    title: &entry.title,
                    body: &entry.body,
                    ongoing: &entry.ongoing,
                    corrects: entry
                        .correction_refs
                        .iter()
                        .map(|id| {
                            snapshot
                                .journals
                                .iter()
                                .enumerate()
                                .find_map(|(j, journal)| {
                                    journal
                                        .entries
                                        .iter()
                                        .position(|entry| &entry.entry_id == id)
                                        .map(|e| entry_ref(j, e))
                                })
                                .unwrap_or_else(|| "entry outside this excerpt".into())
                        })
                        .collect(),
                    omitted: entry.omitted,
                })
                .collect(),
            omitted: journal.omitted,
        })
        .collect::<Vec<_>>();
    if journals.is_empty() && !snapshot.omitted {
        return Ok(None);
    }
    serde_yaml::to_string(&Context {
        journals,
        omitted: snapshot.omitted,
    })
    .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use subjektiv::journal::startup::{StartupEntry, StartupJournal};

    fn snapshot() -> JournalStartupSnapshot {
        JournalStartupSnapshot {
            subject_id: "internal-subject".into(),
            omitted: false,
            journals: vec![StartupJournal {
                journal_id: "internal-journal".into(),
                source_session_id: "internal-session".into(),
                state_token: "internal-token".into(),
                process_order: 17,
                process_byte: 42,
                current_position: String::new(),
                entries: vec![],
                omitted: false,
            }],
        }
    }

    #[test]
    fn empty_journal_is_silent_but_omission_is_not_absence() {
        let mut snapshot = snapshot();
        assert!(yaml(&snapshot).unwrap().is_none());
        snapshot.journals[0].omitted = true;
        let view: serde_yaml::Value =
            serde_yaml::from_str(&yaml(&snapshot).unwrap().unwrap()).unwrap();
        assert_eq!(view["journals"][0]["omitted"].as_bool(), Some(true));
        snapshot.journals.clear();
        snapshot.omitted = true;
        assert_eq!(yaml(&snapshot).unwrap().unwrap().trim(), "omitted: true");
    }

    #[test]
    fn yaml_keeps_text_and_corrections_without_transport_metadata() {
        let mut snapshot = snapshot();
        let text = "heading: value\n---\n```\n  literal content\n";
        snapshot.journals[0].current_position = text.into();
        snapshot.journals[0].entries = vec![StartupEntry {
            entry_id: "internal-entry".into(),
            position: 1,
            state: JournalEntryState::Open,
            title: "yes".into(),
            body: text.into(),
            ongoing: "pending".into(),
            source_end: None,
            correction_refs: vec!["internal-entry".into(), "not-in-excerpt".into()],
            omitted: true,
        }];
        let rendered = yaml(&snapshot).unwrap().unwrap();
        for hidden in [
            "internal-",
            "state_token",
            "process_order",
            "process_byte",
            "source_end",
            "subject_id",
        ] {
            assert!(!rendered.contains(hidden), "leaked {hidden}");
        }
        let view: serde_yaml::Value = serde_yaml::from_str(&rendered).unwrap();
        assert_eq!(view["journals"][0]["current_position"].as_str(), Some(text));
        let entry = &view["journals"][0]["entries"][0];
        assert_eq!(entry["body"].as_str(), Some(text));
        assert_eq!(entry["title"].as_str(), Some("yes"));
        assert_eq!(entry["corrects"][0].as_str(), Some("journal 1, entry 1"));
        assert_eq!(
            entry["corrects"][1].as_str(),
            Some("entry outside this excerpt")
        );
    }
}
