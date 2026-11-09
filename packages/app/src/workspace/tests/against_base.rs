//! The against-base axis: what a lane committed since its base, read off the
//! tracking refresh.

use super::*;
use crate::lane::git::base::{AgainstBase, BaseProblem, BaseTips, RangeFile, ResolvedBase};
use crate::lane::git::{FakeRepo, GitTracking};
use crate::workspace::main_area::file_view_pane::DiffSource;

/// What git answers for a `feature` lane that committed `feature.txt` on top
/// of `main`. How real git computes this is `lane::git::base`'s own tests.
fn feature_tips(head: &str) -> BaseTips {
    BaseTips {
        base: ResolvedBase {
            label: "main".into(),
            sha: "b0".into(),
        },
        head: head.into(),
    }
}

fn feature_repo(repo: FakeRepo, head: &str, files: Vec<RangeFile>) -> FakeRepo {
    // An empty range is the lane after a second commit took the file back out.
    let commits = if files.is_empty() { 2 } else { 1 };
    repo.branch("feature")
        .default_branch("main")
        .base_tips(Ok(feature_tips(head)))
        .changes_since(Ok(AgainstBase {
            tips: feature_tips(head),
            merge_base: "b0".into(),
            files,
            commits,
        }))
        .diff_range(ADDED_DIFF)
}

fn added_feature_file() -> RangeFile {
    RangeFile {
        path: "feature.txt".into(),
        old_path: None,
        status: 'A',
        added: 1,
        removed: 0,
    }
}

/// `git diff -M <merge-base> <head> -- feature.txt` for the added file.
const ADDED_DIFF: &str = "diff --git a/feature.txt b/feature.txt
new file mode 100644
index 0000000..3e75765
--- /dev/null
+++ b/feature.txt
@@ -0,0 +1 @@
+new
";

/// The workspace over a `feature` lane that added `feature.txt` since `main`,
/// with the construction placeholder upgraded to a git lane.
fn open_feature(cx: &mut TestAppContext) -> GitFixture {
    GitFixture::open(cx, &daruda_config::Config::default(), |repo| {
        feature_repo(repo, "h1", vec![added_feature_file()])
    })
}

#[gpui::test]
fn a_tracking_refresh_reads_what_the_lane_committed_since_its_base(cx: &mut TestAppContext) {
    let f = open_feature(cx);
    let (ws, fake, root) = (&f.ws, &f.fake, &f.root);
    let target = ws.update(cx, |ws, cx| {
        ws.projects[0].default_branch = Some("main".to_owned());
        let target = ws.active;
        ws.refresh_git_status(target, cx);
        target
    });
    cx.run_until_parked();
    ws.read_with(cx, |ws, _| {
        let state = &ws.lane_scoped[&target].git;
        let found = state
            .against_base
            .as_deref()
            .expect("the tracking refresh reads the axis")
            .as_ref()
            .expect("the lane has a base");
        assert_eq!(found.tips.base.label, "main");
        assert_eq!(found.commits, 1);
        let paths: Vec<_> = found.files.iter().map(|f| f.path.clone()).collect();
        assert_eq!(paths, vec![std::path::PathBuf::from("feature.txt")]);
    });

    // On its own base branch a lane has nothing to compare: the refresh
    // hands git the branch it just read.
    fake.set_tracking(
        root,
        GitTracking {
            branch: Some("main".into()),
            ..GitTracking::default()
        },
    );
    ws.update(cx, |ws, cx| ws.refresh_git_status(target, cx));
    cx.run_until_parked();
    ws.read_with(cx, |ws, _| {
        assert_eq!(
            ws.lane_scoped[&target].git.against_base.as_deref(),
            Some(&Err(BaseProblem::OnBaseBranch))
        );
    });
}

#[gpui::test]
fn a_lane_with_no_base_anywhere_says_so(cx: &mut TestAppContext) {
    let f = open_feature(cx);
    let ws = &f.ws;
    let target = ws.update(cx, |ws, cx| {
        // Reconcile detected `main`; a repo with no detectable default is
        // what this case is about.
        ws.projects[0].default_branch = None;
        let target = ws.active;
        ws.refresh_git_status(target, cx);
        target
    });
    cx.run_until_parked();
    ws.read_with(cx, |ws, _| {
        assert_eq!(
            ws.lane_scoped[&target].git.against_base.as_deref(),
            Some(&Err(BaseProblem::NoBaseConfigured))
        );
    });
}

#[gpui::test]
fn an_against_base_row_opens_a_diff_pinned_to_its_commits(cx: &mut TestAppContext) {
    use crate::workspace::main_area::file_view_pane::PaneFileContent;
    use crate::workspace::main_area::tab_ops::OpenIntent;

    let f = open_feature(cx);
    let (wh, ws, fake, root) = (f.window, &f.ws, &f.fake, &f.root);
    let target = ws.update(cx, |ws, cx| {
        ws.projects[0].default_branch = Some("main".to_owned());
        let target = ws.active;
        ws.refresh_git_status(target, cx);
        target
    });
    cx.run_until_parked();

    let open = |cx: &mut TestAppContext| {
        cx.update_window(wh.into(), |_, window, cx| {
            ws.update(cx, |ws, cx| {
                ws.open_against_base_file(
                    target,
                    std::path::PathBuf::from("feature.txt"),
                    OpenIntent::Commit,
                    window,
                    cx,
                );
            });
        })
        .unwrap();
        cx.run_until_parked();
    };
    open(cx);
    let tabs = ws.read_with(cx, |ws, _| {
        let found = ws.lane_scoped[&target]
            .git
            .against_base
            .as_deref()
            .and_then(|r| r.as_ref().ok())
            .cloned()
            .expect("the lane has a base");
        let fc = ws
            .focused_file_content()
            .expect("the range pane is focused");
        assert_eq!(
            fc.view.source,
            DiffSource::Range {
                from: found.merge_base.clone(),
                to: found.tips.head.clone(),
                old_path: None,
                status: 'A',
            }
        );
        assert_eq!(fc.view.status(), Some('A'), "status comes from the range");
        match &fc.view.content {
            PaneFileContent::LoadedDiff { added, removed, .. } => {
                assert_eq!((*added, *removed), (1, 0));
            }
            PaneFileContent::Error(e) => panic!("load errored: {e}"),
            _ => panic!("expected a diff"),
        }
        ws.active_runtime().tabs.len()
    });

    // Opening the same row again finds the pane rather than stacking a tab.
    open(cx);
    ws.read_with(cx, |ws, _| assert_eq!(ws.active_runtime().tabs.len(), tabs));

    // The lane moves on and the file leaves the range; the open pane still
    // shows its pinned diff, so it must keep offering it.
    fake.add_repo(feature_repo(FakeRepo::at(root), "h2", Vec::new()));
    ws.update(cx, |ws, cx| ws.refresh_git_status(target, cx));
    cx.run_until_parked();
    ws.read_with(cx, |ws, _| {
        let fc = ws
            .focused_file_content()
            .expect("the range pane is still open");
        assert_eq!(fc.view.status(), Some('A'));
    });
}

#[test]
fn a_persisted_range_restores_ahead_of_the_staged_flag_and_round_trips() {
    use daruda_store::project::{
        SerializedDiffRange, SerializedFileContent, SerializedFileViewMode,
    };
    let mut fc = SerializedFileContent {
        lane_id: 0,
        path: std::path::PathBuf::from("/r/a.rs"),
        staged: true,
        range: None,
        view_mode: SerializedFileViewMode::Changes,
        reference: false,
    };
    assert_eq!(DiffSource::from_serialized(&fc), DiffSource::Index);
    fc.staged = false;
    assert_eq!(DiffSource::from_serialized(&fc), DiffSource::WorkingTree);
    fc.range = Some(SerializedDiffRange {
        from: "m".into(),
        to: "h".into(),
        old_path: None,
        status: 'D',
    });
    fc.staged = true;
    assert_eq!(
        DiffSource::from_serialized(&fc),
        DiffSource::Range {
            from: "m".into(),
            to: "h".into(),
            old_path: None,
            status: 'D',
        }
    );
    // And writing it back yields the record it came from.
    let (staged, range) = DiffSource::from_serialized(&fc).to_serialized();
    assert!(!staged, "a range pane is not staged");
    assert_eq!(range, fc.range);
}

/// A git read that outlives its lane — torn down while the read ran — must
/// drop its result, not bring the lane's state back into the map.
#[gpui::test]
fn a_git_read_that_outlives_its_lane_does_not_revive_its_state(cx: &mut TestAppContext) {
    let f = open_feature(cx);
    let ws = &f.ws;
    let target = ws.update(cx, |ws, cx| {
        let target = ws.active;
        ws.refresh_git_status(target, cx);
        // What lane teardown does, while both reads are still in flight.
        ws.lane_scoped.remove(&target);
        target
    });
    cx.run_until_parked();
    ws.read_with(cx, |ws, _| {
        assert!(
            !ws.lane_scoped.contains_key(&target),
            "a late git read recreated the removed lane's state"
        );
    });
}
