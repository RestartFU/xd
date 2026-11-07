//! Automatic deletion: removing sessions that have sat idle for the chosen
//! number of days, together with the worktrees xd generated for them.
//!
//! It is off until the user picks a number of days, since it cannot be undone.
//! It runs in the daemon, on the same schedule as settling; see
//! [`crate::settlement`]. Using a session's terminal counts as activity. A
//! worktree is only removed when xd recorded generating it, it is clean, and
//! its commits are on a branch or tag; otherwise the whole session stays. Git
//! removes it without forcing, and the branch is left in the repository.

use serde_json::{Value, json};

pub(crate) const MIN_AFTER_DAYS: i64 = 1;
pub(crate) const MAX_AFTER_DAYS: i64 = 365;

/// How a host deletes sessions by itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AutoDeleteSettings {
    /// Delete after this many days without activity; `None`, the default,
    /// deletes nothing.
    pub after_days: Option<i64>,
}

impl AutoDeleteSettings {
    /// How the rule is stored: what a client sees, less the reply's `ok`.
    pub(crate) fn encoded(&self) -> String {
        json!({"after_days": self.after_days}).to_string()
    }

    /// Reads the stored rule. Anything unreadable turns deletion off rather
    /// than guessing at a number of days, and the next change replaces it.
    pub(crate) fn decode(stored: &str) -> Self {
        let after_days = serde_json::from_str::<Value>(stored)
            .ok()
            .and_then(|stored| stored.get("after_days")?.as_i64())
            .filter(|days| (MIN_AFTER_DAYS..=MAX_AFTER_DAYS).contains(days));
        Self { after_days }
    }

    pub(crate) fn reply(&self) -> Value {
        json!({"ok": true, "after_days": self.after_days})
    }
}

/// What became of a session a sweep chose to delete.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AutoDeletion {
    /// The session is gone, and so are these worktrees.
    Deleted {
        chats: Vec<String>,
        worktrees: Vec<String>,
        /// Why a worktree Git refused at the last moment is still on disk.
        left: Vec<String>,
    },
    /// Something happened in the session, or to the rule, after the sweep
    /// looked. The next sweep decides again.
    Changed,
    /// The session stays because its worktree could not be removed safely.
    Kept(String),
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::{
        StateStore,
        settlement::{DAY_SECONDS, SettlementCandidate, now_seconds},
    };

    #[test]
    fn the_rule_is_off_unless_it_reads_cleanly() {
        assert_eq!(AutoDeleteSettings::default().after_days, None);
        assert_eq!(
            AutoDeleteSettings::decode(r#"{"after_days":14}"#).after_days,
            Some(14)
        );
        for damaged in [
            "garbage",
            r#"{"after_days":0}"#,
            r#"{"after_days":366}"#,
            r#"{"after_days":"14"}"#,
            "{}",
        ] {
            assert_eq!(AutoDeleteSettings::decode(damaged).after_days, None);
        }
    }

    /// A store with one session of one chat, removed when the test ends.
    struct Fixture {
        root: PathBuf,
        store: StateStore,
        session: String,
        chat: String,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "xd-deletion-test-{}",
                uuid::Uuid::new_v4().simple()
            ));
            let workspaces = root.join("Workspaces");
            std::fs::create_dir_all(&workspaces).unwrap();
            let store = StateStore::open(root.join("chats.db"), &workspaces).unwrap();
            let folder = store.new_folder(&json!({"name": "Project"})).unwrap()["id"].clone();
            let chat = store
                .new_chat(&json!({"folder": folder, "title": "Task"}))
                .unwrap();
            Self {
                root,
                store,
                session: chat["session"].as_str().unwrap().to_owned(),
                chat: chat["id"].as_str().unwrap().to_owned(),
            }
        }

        fn sql(&self, statement: &str) {
            rusqlite::Connection::open(self.root.join("chats.db"))
                .unwrap()
                .execute_batch(statement)
                .unwrap();
        }

        fn age(&self, days: i64) {
            let at = now_seconds() - days * DAY_SECONDS;
            self.sql(&format!(
                "UPDATE chats SET created_at = {at}, last_user_message_at = {micros}; \
                 UPDATE messages SET created_at = {at}; \
                 UPDATE workspace_sessions SET created_at = {at};",
                micros = at * 1_000_000
            ));
        }

        fn enable(&self, days: i64) -> AutoDeleteSettings {
            self.store
                .set_auto_delete_settings(&json!({"after_days": days}))
                .unwrap()
        }

        /// Deletes whatever the rule picks, as the daemon's sweep would.
        fn sweep(&self) -> Vec<AutoDeletion> {
            let settings = self.store.auto_delete_settings().unwrap();
            self.store
                .deletion_candidates(now_seconds(), &settings)
                .unwrap()
                .iter()
                .map(|candidate| self.delete(candidate, &settings))
                .collect()
        }

        fn delete(
            &self,
            candidate: &SettlementCandidate,
            settings: &AutoDeleteSettings,
        ) -> AutoDeletion {
            match self.store.plan_auto_delete(candidate, settings).unwrap() {
                Ok(plan) => self
                    .store
                    .finish_auto_delete(candidate, settings, plan, &|| true)
                    .unwrap(),
                Err(outcome) => outcome,
            }
        }

        fn chats(&self) -> usize {
            self.store.tree().unwrap()["chats"]
                .as_array()
                .unwrap()
                .len()
        }

        fn git(&self, workdir: &Path, arguments: &[&str]) -> String {
            let output = std::process::Command::new("git")
                .args(arguments)
                .current_dir(workdir)
                .env("GIT_AUTHOR_NAME", "x")
                .env("GIT_AUTHOR_EMAIL", "x@example.com")
                .env("GIT_COMMITTER_NAME", "x")
                .env("GIT_COMMITTER_EMAIL", "x@example.com")
                .output()
                .unwrap();
            assert!(output.status.success(), "git {arguments:?}");
            String::from_utf8(output.stdout).unwrap()
        }

        /// Gives the chat a worktree xd generated, with its first message sent.
        fn generated_worktree(&self) -> PathBuf {
            let repository = self.root.join("Workspaces/Project");
            self.git(&repository, &["init", "-q", "-b", "main"]);
            self.git(
                &repository,
                &["commit", "-q", "--allow-empty", "-m", "start"],
            );
            self.store
                .set_option(&json!({"chat": self.chat, "option": "new-worktree", "value": "true"}))
                .unwrap();
            self.store
                .prepare_send(&json!({"chat": self.chat, "text": "build it"}))
                .unwrap();
            self.sql("UPDATE chats SET host_working = 0");
            let chat = self.store.chat(&self.chat).unwrap();
            PathBuf::from(chat["workdir"].as_str().unwrap())
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn the_rule_is_saved_and_checked() {
        let fixture = Fixture::new();
        let store = &fixture.store;
        assert_eq!(
            store.auto_delete_settings().unwrap(),
            AutoDeleteSettings::default()
        );
        for bad in [json!(0), json!(366), json!("3"), json!(1.5)] {
            assert!(
                store
                    .set_auto_delete_settings(&json!({"after_days": bad}))
                    .is_err()
            );
        }
        assert_eq!(fixture.enable(45).after_days, Some(45));
        assert_eq!(
            store
                .set_auto_delete_settings(&json!({}))
                .unwrap()
                .after_days,
            Some(45),
            "a request that names nothing changes nothing"
        );
        let reopened = StateStore::open(
            fixture.root.join("chats.db"),
            fixture.root.join("Workspaces"),
        )
        .unwrap();
        assert_eq!(
            reopened.auto_delete_settings().unwrap().after_days,
            Some(45)
        );
        let off = store
            .set_auto_delete_settings(&json!({"after_days": null}))
            .unwrap();
        assert_eq!(off.after_days, None);
    }

    #[test]
    fn nothing_is_deleted_until_the_rule_is_on_and_the_days_have_passed() {
        let fixture = Fixture::new();
        fixture.age(400);
        assert!(fixture.sweep().is_empty(), "off by default");

        fixture.enable(30);
        fixture.age(29);
        assert!(fixture.sweep().is_empty());

        fixture.age(31);
        assert_eq!(
            fixture.sweep(),
            vec![AutoDeletion::Deleted {
                chats: vec![fixture.chat.clone()],
                worktrees: Vec::new(),
                left: Vec::new(),
            }]
        );
        assert_eq!(fixture.chats(), 0);
    }

    #[test]
    fn busy_kept_or_reclaimed_sessions_are_not_deleted() {
        let fixture = Fixture::new();
        fixture.enable(1);
        fixture.age(10);

        fixture.sql("UPDATE chats SET host_working = 1");
        assert!(fixture.sweep().is_empty(), "a running turn");
        fixture.sql("UPDATE chats SET host_working = 0, queued = '[\"next\"]'");
        assert!(fixture.sweep().is_empty(), "a queued message");
        fixture.sql("UPDATE chats SET queued = NULL");

        let request = json!({"session": fixture.session});
        fixture
            .store
            .set_session_auto_settle(&json!({"session": fixture.session, "enabled": false}))
            .unwrap();
        assert!(
            fixture.sweep().is_empty(),
            "kept out of the automatic rules"
        );
        fixture
            .store
            .set_session_auto_settle(&json!({"session": fixture.session, "enabled": true}))
            .unwrap();

        fixture.store.settle_session(&request).unwrap();
        fixture.store.unsettle_session(&request).unwrap();
        assert!(fixture.sweep().is_empty(), "the user asked for it back");

        fixture.store.settle_session(&request).unwrap();
        assert_eq!(fixture.sweep().len(), 1, "settled sessions are deleted too");
        assert_eq!(fixture.chats(), 0);
    }

    #[test]
    fn activity_or_a_rule_change_after_the_sweep_looked_keeps_the_session() {
        let fixture = Fixture::new();
        let settings = fixture.enable(5);
        fixture.age(10);
        let candidate = fixture
            .store
            .deletion_candidates(now_seconds(), &settings)
            .unwrap()
            .remove(0);

        let longer = fixture.enable(20);
        assert_eq!(fixture.delete(&candidate, &settings), AutoDeletion::Changed);

        // Planned before the message, finished after it.
        let plan = fixture
            .store
            .plan_auto_delete(&candidate, &longer)
            .unwrap()
            .ok()
            .unwrap();
        fixture
            .store
            .prepare_send(&json!({"chat": fixture.chat, "text": "hold on"}))
            .unwrap();
        fixture.sql("UPDATE chats SET host_working = 0");
        assert_eq!(
            fixture
                .store
                .finish_auto_delete(&candidate, &longer, plan, &|| true)
                .unwrap(),
            AutoDeletion::Changed
        );
        assert_eq!(fixture.delete(&candidate, &longer), AutoDeletion::Changed);
        assert_eq!(fixture.chats(), 1);
    }

    #[test]
    fn a_generated_worktree_goes_with_its_session_but_its_branch_stays() {
        let fixture = Fixture::new();
        let worktree = fixture.generated_worktree();
        let repository = fixture.root.join("Workspaces/Project");
        let branch = fixture.git(&worktree, &["branch", "--show-current"]);
        fixture.git(&worktree, &["commit", "-q", "--allow-empty", "-m", "work"]);
        std::fs::write(worktree.join("build.log"), "ignored").unwrap();
        std::fs::write(worktree.join(".gitignore"), "build.log\n.gitignore\n").unwrap();
        fixture.enable(1);
        fixture.age(3);

        let deleted = fixture.sweep();
        assert_eq!(
            deleted,
            vec![AutoDeletion::Deleted {
                chats: vec![fixture.chat.clone()],
                worktrees: vec![worktree.to_string_lossy().into_owned()],
                left: Vec::new(),
            }]
        );
        assert!(!worktree.exists());
        assert!(
            !worktree.parent().unwrap().exists(),
            "the folder xd made around it goes too"
        );
        assert!(
            !fixture
                .git(&repository, &["worktree", "list"])
                .contains("worktrees")
        );
        let log = fixture.git(&repository, &["log", "--format=%s", branch.trim()]);
        assert!(log.starts_with("work"), "the branch keeps its commits");
    }

    #[test]
    fn uncommitted_work_keeps_the_session_and_its_worktree() {
        let fixture = Fixture::new();
        let worktree = fixture.generated_worktree();
        std::fs::write(worktree.join("draft.txt"), "unsaved").unwrap();
        fixture.enable(1);
        fixture.age(3);
        let kept = fixture.sweep();
        assert!(
            matches!(kept.as_slice(), [AutoDeletion::Kept(_)]),
            "{kept:?}"
        );
        assert!(worktree.join("draft.txt").exists());
        assert_eq!(fixture.chats(), 1);

        std::fs::remove_file(worktree.join("draft.txt")).unwrap();
        assert!(matches!(
            fixture.sweep().as_slice(),
            [AutoDeletion::Deleted { .. }]
        ));
        assert!(!worktree.exists());
    }

    #[test]
    fn a_worktree_xd_did_not_make_or_another_session_uses_is_left_alone() {
        let fixture = Fixture::new();
        let repository = fixture.root.join("Workspaces/Project");
        fixture.git(&repository, &["init", "-q", "-b", "main"]);
        fixture.git(
            &repository,
            &["commit", "-q", "--allow-empty", "-m", "start"],
        );
        let own = fixture.root.join("own-worktree");
        fixture.git(
            &repository,
            &["worktree", "add", "-q", "-b", "mine", own.to_str().unwrap()],
        );
        fixture
            .store
            .set_option(&json!({
                "chat": fixture.chat,
                "option": "workspace",
                "value": own.to_str().unwrap(),
            }))
            .unwrap();
        fixture.enable(1);
        fixture.age(3);
        assert!(matches!(
            fixture.sweep().as_slice(),
            [AutoDeletion::Deleted { worktrees, .. }] if worktrees.is_empty()
        ));
        assert!(own.exists(), "the user made this worktree");

        let fixture = Fixture::new();
        let worktree = fixture.generated_worktree();
        let folder = fixture.store.tree().unwrap()["folders"][0]["id"].clone();
        let other = fixture
            .store
            .new_chat(&json!({"folder": folder, "title": "Other"}))
            .unwrap();
        fixture.sql(&format!(
            "UPDATE chats SET workdir = '{}' WHERE id = '{}'",
            worktree.display(),
            other["id"].as_str().unwrap()
        ));
        fixture.enable(1);
        fixture.age(3);
        fixture.sql(&format!(
            "UPDATE chats SET created_at = {} WHERE id = '{}'; \
             UPDATE workspace_sessions SET created_at = {} WHERE id = '{}'",
            now_seconds(),
            other["id"].as_str().unwrap(),
            now_seconds(),
            other["session"].as_str().unwrap()
        ));
        let deleted = fixture.sweep();
        assert!(
            matches!(
                deleted.as_slice(),
                [AutoDeletion::Deleted { worktrees, .. }] if worktrees.is_empty()
            ),
            "{deleted:?}"
        );
        assert!(worktree.exists(), "another session still works in it");
    }

    #[test]
    fn commits_only_a_detached_head_holds_keep_the_session() {
        let fixture = Fixture::new();
        let worktree = fixture.generated_worktree();
        fixture.git(&worktree, &["checkout", "-q", "--detach"]);
        fixture.git(&worktree, &["commit", "-q", "--allow-empty", "-m", "loose"]);
        fixture.enable(1);
        fixture.age(3);
        let kept = fixture.sweep();
        assert!(
            matches!(kept.as_slice(), [AutoDeletion::Kept(_)]),
            "{kept:?}"
        );
        assert!(worktree.exists());

        fixture.git(&worktree, &["branch", "keep-loose"]);
        assert!(matches!(
            fixture.sweep().as_slice(),
            [AutoDeletion::Deleted { worktrees, .. }] if worktrees.len() == 1
        ));
        assert!(!worktree.exists());
    }

    #[test]
    fn a_worktree_the_user_made_in_the_generated_folder_is_left_alone() {
        let fixture = Fixture::new();
        let generated = fixture.generated_worktree();
        let repository = fixture.root.join("Workspaces/Project");
        let own = generated
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("own/Project");
        fixture.git(
            &repository,
            &["worktree", "add", "-q", "-b", "mine", own.to_str().unwrap()],
        );
        let folder = fixture.store.tree().unwrap()["folders"][0]["id"].clone();
        let other = fixture
            .store
            .new_chat(&json!({"folder": folder, "title": "Mine"}))
            .unwrap();
        fixture
            .store
            .set_option(&json!({
                "chat": other["id"],
                "option": "workspace",
                "value": own.to_str().unwrap(),
            }))
            .unwrap();
        fixture.enable(1);
        fixture.age(3);
        let deleted = fixture.sweep();
        assert_eq!(deleted.len(), 2, "{deleted:?}");
        assert!(!generated.exists());
        assert!(own.exists(), "xd did not make this one");
    }

    #[test]
    fn worktrees_from_before_the_record_are_entered_once_by_where_they_sit() {
        let fixture = Fixture::new();
        let worktree = fixture.generated_worktree();
        fixture.sql("DROP TABLE generated_worktrees");
        StateStore::open(
            fixture.root.join("chats.db"),
            fixture.root.join("Workspaces"),
        )
        .unwrap();
        fixture.enable(1);
        fixture.age(3);
        assert!(matches!(
            fixture.sweep().as_slice(),
            [AutoDeletion::Deleted { worktrees, .. }] if worktrees.len() == 1
        ));
        assert!(!worktree.exists());

        let fixture = Fixture::new();
        let worktree = fixture.generated_worktree();
        fixture.git(&worktree, &["checkout", "-q", "-b", "xd/manual"]);
        fixture.sql("DROP TABLE generated_worktrees");
        StateStore::open(
            fixture.root.join("chats.db"),
            fixture.root.join("Workspaces"),
        )
        .unwrap();
        fixture.enable(1);
        fixture.age(3);
        assert!(matches!(
            fixture.sweep().as_slice(),
            [AutoDeletion::Deleted { worktrees, .. }] if worktrees.is_empty()
        ));
        assert!(worktree.exists(), "only xd's branch for the chat proves it");
    }

    #[test]
    fn a_recorded_worktree_on_another_branch_is_left_alone() {
        let fixture = Fixture::new();
        let worktree = fixture.generated_worktree();
        fixture.git(&worktree, &["checkout", "-q", "-b", "elsewhere"]);
        fixture.enable(1);
        fixture.age(3);
        assert!(matches!(
            fixture.sweep().as_slice(),
            [AutoDeletion::Deleted { worktrees, .. }] if worktrees.is_empty()
        ));
        assert!(worktree.exists(), "it may not be the worktree xd made");
    }

    #[test]
    fn work_left_in_the_worktree_after_planning_keeps_the_session() {
        let fixture = Fixture::new();
        let worktree = fixture.generated_worktree();
        fixture.enable(1);
        fixture.age(3);
        let settings = fixture.store.auto_delete_settings().unwrap();
        let candidate = fixture
            .store
            .deletion_candidates(now_seconds(), &settings)
            .unwrap()
            .remove(0);
        let plan = fixture
            .store
            .plan_auto_delete(&candidate, &settings)
            .unwrap()
            .ok()
            .unwrap();
        std::fs::write(worktree.join("late.txt"), "written after the check").unwrap();
        let finished = fixture
            .store
            .finish_auto_delete(&candidate, &settings, plan, &|| true)
            .unwrap();
        assert!(matches!(finished, AutoDeletion::Kept(_)), "{finished:?}");
        assert!(worktree.join("late.txt").exists());
        assert_eq!(
            fixture.chats(),
            1,
            "checked again before anything is deleted"
        );
    }

    #[test]
    fn a_session_working_inside_the_worktree_keeps_it() {
        let fixture = Fixture::new();
        let worktree = fixture.generated_worktree();
        let nested = worktree.join("packages/app");
        std::fs::create_dir_all(&nested).unwrap();
        let folder = fixture.store.tree().unwrap()["folders"][0]["id"].clone();
        let other = fixture
            .store
            .new_chat(&json!({"folder": folder, "title": "Nested"}))
            .unwrap();
        fixture.enable(1);
        fixture.age(3);
        fixture.sql(&format!(
            "UPDATE chats SET workdir = '{}', created_at = {now} WHERE id = '{id}'; \
             UPDATE workspace_sessions SET created_at = {now} WHERE id = '{session}'",
            nested.display(),
            now = now_seconds(),
            id = other["id"].as_str().unwrap(),
            session = other["session"].as_str().unwrap(),
        ));
        let deleted = fixture.sweep();
        assert!(
            matches!(
                deleted.as_slice(),
                [AutoDeletion::Deleted { worktrees, .. }] if worktrees.is_empty()
            ),
            "{deleted:?}"
        );
        assert!(nested.exists());
    }

    #[test]
    fn switching_branch_after_planning_keeps_the_worktree() {
        let fixture = Fixture::new();
        let worktree = fixture.generated_worktree();
        fixture.enable(1);
        fixture.age(3);
        let settings = fixture.store.auto_delete_settings().unwrap();
        let candidate = fixture
            .store
            .deletion_candidates(now_seconds(), &settings)
            .unwrap()
            .remove(0);
        let plan = fixture
            .store
            .plan_auto_delete(&candidate, &settings)
            .unwrap()
            .ok()
            .unwrap();
        fixture.git(&worktree, &["checkout", "-q", "-b", "mine-now"]);
        let finished = fixture
            .store
            .finish_auto_delete(&candidate, &settings, plan, &|| true)
            .unwrap();
        assert!(
            matches!(&finished, AutoDeletion::Deleted { worktrees, .. } if worktrees.is_empty()),
            "{finished:?}"
        );
        assert!(worktree.exists());
    }

    #[test]
    fn a_worktree_made_again_at_the_same_place_is_not_taken_for_xds() {
        let fixture = Fixture::new();
        let worktree = fixture.generated_worktree();
        let repository = fixture.root.join("Workspaces/Project");
        let branch = fixture.git(&worktree, &["branch", "--show-current"]);
        let target = worktree.to_str().unwrap();
        fixture.git(&repository, &["worktree", "remove", target]);
        fixture.git(
            &repository,
            &["worktree", "add", "-q", target, branch.trim()],
        );
        fixture.enable(1);
        fixture.age(3);
        assert!(matches!(
            fixture.sweep().as_slice(),
            [AutoDeletion::Deleted { worktrees, .. }] if worktrees.is_empty()
        ));
        assert!(worktree.exists(), "the user made this one");
    }

    #[test]
    fn activity_reported_at_the_last_moment_keeps_the_session() {
        let fixture = Fixture::new();
        let worktree = fixture.generated_worktree();
        fixture.enable(1);
        fixture.age(3);
        let settings = fixture.store.auto_delete_settings().unwrap();
        let candidate = fixture
            .store
            .deletion_candidates(now_seconds(), &settings)
            .unwrap()
            .remove(0);
        let plan = fixture
            .store
            .plan_auto_delete(&candidate, &settings)
            .unwrap()
            .ok()
            .unwrap();
        assert_eq!(
            fixture
                .store
                .finish_auto_delete(&candidate, &settings, plan, &|| false)
                .unwrap(),
            AutoDeletion::Changed
        );
        assert_eq!(fixture.chats(), 1);
        assert!(worktree.exists());
    }
}
