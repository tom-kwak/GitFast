mod common;

use common::open;
use gf_git::{CancellationToken, LogOrder, LogQuery, Repository};
use gf_test_support::TestRepo;

async fn find(repo: &Repository, scope: &LogQuery, term: &str) -> Vec<gf_core::model::Commit> {
    repo.search_commits(scope, term, CancellationToken::new())
        .await
        .expect("commit search")
}

#[tokio::test]
async fn finds_literal_message_body_and_hashes_only_in_the_requested_history() {
    let fixture = TestRepo::new();
    fixture.write("file", "root");
    let root = fixture.commit_all("first\n\n한글 Retry.*");
    fixture.git(&["tag", "pinned"]);
    fixture.write("file", "second");
    fixture.commit_all("reTRYx is not a literal match");
    fixture.git(&["switch", "-q", "-c", "topic"]);
    fixture.write("file", "topic");
    let tip = fixture.commit_all(&format!("retry.* on topic ({})", &root[..7]));
    fixture.git(&["switch", "-q", "main"]);
    let repo = open(&fixture).await;
    let all = LogQuery {
        all_refs: true,
        order: LogOrder::Topo,
        ..LogQuery::default()
    };
    let pinned = LogQuery {
        all_refs: false,
        revisions: vec!["refs/tags/pinned".into()],
        ..all.clone()
    };
    let messages = find(&repo, &all, " retry.* ").await;
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].id.as_str(), tip);
    assert_eq!(messages[1].id.as_str(), root);
    assert!(messages[1].body.contains("한글"));
    let hash = find(&repo, &all, &root[..7].to_ascii_uppercase()).await;
    assert_eq!(hash.len(), 2);
    assert_eq!(hash[0].id.as_str(), root);
    assert_eq!(hash[1].id.as_str(), tip);
    assert!(find(&repo, &pinned, &tip).await.is_empty());
    let exact = find(&repo, &pinned, &root).await;
    assert_eq!(exact[0].id.as_str(), root);
}

#[tokio::test]
async fn an_unborn_history_is_empty_for_both_search_kinds() {
    let fixture = TestRepo::new();
    let repo = open(&fixture).await;
    for all_refs in [false, true] {
        let query = LogQuery {
            all_refs,
            ..Default::default()
        };
        assert!(
            repo.search_commits(&query, "deadbeef", CancellationToken::new())
                .await
                .expect("empty search")
                .is_empty()
        );
    }
}
