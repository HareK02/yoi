use super::*;

#[test]
fn comments_preserve_text_exact_replay_and_owner_fencing_without_changing_review() {
    let (_dir, store) = fixture();
    open(&store);
    let approval = approve(&store, "source", "comment-review");
    let auth = mr_auth(&store, "MR", "R");
    let before = store.get_by_id("W", "MR").unwrap();
    let event = store
        .comment(
            &auth,
            "MR",
            "comment-1",
            "Conflict resolved.\nValidation passed.",
            at(6),
            |_| Ok(()),
        )
        .unwrap();
    assert_eq!(
        store
            .comment(&auth, "MR", "comment-1", &event.body, at(7), |_| Ok(()))
            .unwrap(),
        event
    );
    assert!(
        store
            .comment(&auth, "MR", "comment-1", "Changed", at(7), |_| Ok(()))
            .is_err()
    );
    assert!(
        store
            .comment(&auth, "MR", "comment-2", "Blocked", at(7), |_| Err(
                MergeRequestError::Unauthorized("revoked".into())
            ))
            .is_err()
    );
    let after = store.get_by_id("W", "MR").unwrap();
    assert_eq!(after.state, before.state);
    assert_eq!(after.thread.len(), before.thread.len() + 1);
    assert_eq!(
        after.effective_review("source").unwrap().event_id,
        approval.event_id
    );
    store
        .set_worker(
            ownership::transfer(&store, "transfer-comment", Some("successor")),
            |_| Ok(()),
        )
        .unwrap();
    assert!(
        store
            .comment(&auth, "MR", "late-comment", "Old owner", at(8), |_| Ok(()))
            .is_err()
    );
    assert!(
        store
            .comment(&auth, "MR", "comment-1", &event.body, at(8), |_| Ok(()))
            .is_err()
    );
}
