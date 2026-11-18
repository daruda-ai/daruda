//! A pull request's detail on the Pull requests page: what it reads from
//! GitHub, what it drops, where Send to agent goes, and what its merge,
//! close, ready and rerun actions ask GitHub.

use crate::workspace::pages::pull_requests::detail::handoff::{Payload, Prepared, Read};
use daruda_github::fake::{FakeGh, GhCall, GhFailure, GhOp};
use daruda_github::{
    ActionsJob, CheckEntry, CheckKind, CheckRollup, CheckRun, CheckStatus, FailedCheck,
    MergeMethod, Mergeability, PrDetail, PrListItem, PrState, PrSummary, RepoId,
};
use daruda_store::project::LaneRef;
use gpui::{AppContext as _, TestAppContext, VisualTestContext};

use crate::lane::git::{FakeRepo, PushTarget};
use crate::workspace::pages::Page;
use crate::workspace::pages::pull_requests::detail::view::{
    ActionSlot, DetailBody, HandoffBlock, LogSlot, SendSlot,
};
use crate::workspace::pages::pull_requests::detail::{Load, PrDetailId, PrTab};
use crate::workspace::tests::GitFixture;

const JOB: ActionsJob = ActionsJob { run: 11, job: 22 };

pub(in crate::workspace) fn repo() -> RepoId {
    RepoId::new("github.com", "o", "r")
}

fn summary(number: u64, head: &str) -> PrSummary {
    PrSummary {
        number,
        url: format!("https://github.com/o/r/pull/{number}"),
        title: format!("PR {number}"),
        state: PrState::Open,
        head_branch: head.into(),
        cross_repo: false,
        checks: CheckRollup::Failing { failed: Some(1) },
        failed: vec![FailedCheck {
            workflow: Some("CI".into()),
            event: Some("pull_request".into()),
            name: "test".into(),
            details_url: Some("https://github.com/o/r/actions/runs/11/job/22".into()),
            job: Some(JOB),
        }],
        review: None,
        updated_at: "2026-01-01T00:00:00Z".into(),
    }
}

pub(in crate::workspace) fn item(number: u64, head: &str) -> PrListItem {
    PrListItem {
        repo: repo(),
        pr: summary(number, head),
        author: Some("alice".into()),
        head_owner: None,
        base_branch: "main".into(),
        comments: 0,
    }
}

pub(in crate::workspace) fn detail(number: u64, head: &str) -> PrDetail {
    PrDetail {
        summary: summary(number, head),
        body: "Body".into(),
        head_sha: "abc".into(),
        base_branch: "main".into(),
        author: Some("alice".into()),
        mergeability: Mergeability::Ready,
        methods: Vec::new(),
        default_method: None,
        reviewers: Vec::new(),
        checks: vec![CheckEntry {
            run: CheckRun {
                kind: CheckKind::Run,
                workflow: Some("CI".into()),
                event: Some("pull_request".into()),
                name: "test".into(),
                status: CheckStatus::Failure,
                details_url: None,
                started_at: None,
                completed_at: None,
            },
            job: Some(JOB),
            duration: None,
        }],
        checks_truncated: false,
        open_threads: 0,
        open_threads_more: false,
        viewer_can: daruda_github::ViewerCan::default()
            .with(daruda_github::Permission::Close, true)
            .with(daruda_github::Permission::Reopen, true),
        auto_merge: None,
        queue: None,
        node_id: format!("PR_{number}"),
        changed_files: None,
    }
}

/// A window whose one lane pushes `feat` to o/r, linked to PR #7, which
/// GitHub can describe, with one failing Actions job whose log it prints.
pub(in crate::workspace) fn window(cx: &mut TestAppContext) -> (GitFixture, FakeGh, LaneRef) {
    let gh = crate::test_support::fake_gh(cx);
    // Chats focused by a handoff connect; none reaches a real adapter.
    cx.update(|cx| {
        super::Workspace::fake_agent_connections_for_test(cx);
    });
    gh.set_prs(&repo(), vec![summary(7, "feat")]);
    gh.set_pr_detail(&repo(), 7, detail(7, "feat"));
    gh.set_failed_log(
        JOB,
        "job\tstep\t2026-01-01T00:00:00.0000000Z ##[group]Run cargo test\n\
         job\tstep\t2026-01-01T00:00:01.0000000Z test parse ... \u{1b}[31mFAILED\u{1b}[0m\n\
         job\tstep\t2026-01-01T00:00:02.0000000Z ##[error]Process completed with exit code 101.",
    );
    let f = GitFixture::open(cx, &daruda_config::Config::default(), |repo: FakeRepo| {
        repo.push_target(PushTarget {
            remote: "origin".into(),
            url: "git@github.com:o/r.git".into(),
            branch: "feat".into(),
            published: true,
            has_upstream: false,
            review: None,
        })
    });
    let linked = f.ws.read_with(cx, |ws, _| ws.active_ref());
    f.ws.update(cx, |ws, cx| ws.refresh_git_status(linked, cx));
    cx.run_until_parked();
    let (window, ws) = (f.window, f.ws.downgrade());
    cx.update(|cx| crate::window_registry::WindowRegistry::register(window.into(), ws, cx));
    cx.update(crate::github::PrPoller::tick);
    cx.run_until_parked();
    f.ws.update(cx, |ws, cx| ws.show_page(Page::PullRequests, cx));
    (f, gh, linked)
}

/// Click a row, as the page does, without waiting for GitHub.
pub(in crate::workspace) fn open(f: &GitFixture, item: PrListItem, cx: &mut TestAppContext) {
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| ws.open_pr_detail(item, window, cx));
    });
}

fn shown(f: &GitFixture, cx: &mut TestAppContext) -> Option<(PrDetailId, u64, Load<PrDetail>)> {
    f.ws.read_with(cx, |ws, _| {
        let detail = ws.pages.pull_requests.detail.as_ref()?;
        Some((detail.id(), detail.item().pr.number, detail.load().clone()))
    })
}

pub(in crate::workspace) fn id(f: &GitFixture, cx: &mut TestAppContext) -> PrDetailId {
    shown(f, cx).expect("a detail is shown").0
}

fn view(
    f: &GitFixture,
    cx: &mut TestAppContext,
) -> crate::workspace::pages::pull_requests::detail::view::PrDetailView {
    f.ws.update(cx, |ws, cx| {
        ws.stage_pr_page(cx);
        ws.pages.pull_requests.detail_snapshot.clone()
    })
    .expect("the page shows its detail")
}

fn input(f: &GitFixture, cx: &mut TestAppContext) -> String {
    f.ws.read_with(cx, |ws, cx| {
        ws.input_dock.input.read(cx).value().to_string()
    })
}

fn split_chat(f: &GitFixture, cx: &mut TestAppContext) {
    cx.update_window(f.window.into(), |_, window, cx| {
        f.ws.update(cx, |ws, cx| {
            ws.split_focused_pane_kind(
                super::NewPaneKind::AgentChat,
                super::SplitDirection::Horizontal,
                window,
                cx,
            );
        });
    })
    .unwrap();
}

#[gpui::test]
fn opening_a_detail_reads_the_pull_request_once(cx: &mut TestAppContext) {
    let (f, gh, _) = window(cx);
    open(&f, item(7, "feat"), cx);
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::PrDetail), 1);
    assert!(
        matches!(shown(&f, cx), Some((_, 7, Load::Ready(_)))),
        "{:?}",
        shown(&f, cx)
    );
    assert_eq!(
        f.ws.read_with(cx, |ws, _| ws.page_detail_id(Page::PullRequests)),
        Some(crate::workspace::pages::detail::PageDetailId::PullRequest(
            id(&f, cx)
        ))
    );

    // The same pull request again is shown as it is, not read again.
    open(&f, item(7, "feat"), cx);
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::PrDetail), 1);
}

#[gpui::test]
fn pull_request_detail_scrolls_long_checks_and_description(cx: &mut TestAppContext) {
    use gpui::{ScrollDelta, ScrollEvent, ScrollSource, TouchPhase, point, px, size};

    let (f, gh, _) = window(cx);
    let mut long = detail(7, "feat");
    f.ws.update(cx, |ws, _| {
        let target = ws.active_ref();
        ws.lane_for_mut(target).unwrap().name = Some("worktree-with-a-long-review-branch".into());
    });
    long.checks = (0..60)
        .map(|ix| {
            let mut check = long.checks[0].clone();
            check.run.name = format!("check {ix}");
            check
        })
        .collect();
    long.body = (0..60).map(|ix| format!("Paragraph {ix}.\n\n")).collect();
    gh.set_pr_detail(&repo(), 7, long);
    open(&f, item(7, "feat"), cx);
    cx.run_until_parked();
    let id = id(&f, cx);
    let scroll = f.ws.read_with(cx, |ws, _| {
        ws.pages
            .pull_requests
            .detail
            .as_ref()
            .unwrap()
            .scroll()
            .clone()
    });
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    for width in [800., 1280.] {
        vcx.simulate_resize(size(px(width), px(500.)));
        for tab in [PrTab::Checks, PrTab::Description] {
            f.ws.update(&mut vcx, |ws, cx| {
                scroll.set_offset(point(px(0.), px(0.)));
                ws.select_pr_detail_tab(id, tab, cx);
            });
            vcx.run_until_parked();
            let viewport = scroll.bounds();
            let controls = vcx.debug_bounds("pr-detail-controls").unwrap();
            assert!(
                viewport.size.height > px(crate::ui::theme::CONTROL_TARGET_SIZE),
                "{tab:?}: fixed controls leave room for content"
            );
            assert!(
                vcx.debug_bounds("pr-detail-title").unwrap().size.width
                    > px(crate::ui::theme::CONTROL_TARGET_SIZE),
                "{tab:?}: actions do not squeeze the title away"
            );
            assert!(
                vcx.debug_bounds("pr-detail-merge").unwrap().right() <= controls.right(),
                "{tab:?}: merge remains reachable"
            );
            let content = vcx.debug_bounds("pr-detail-tab-content").unwrap();
            let side = vcx.debug_bounds("pr-detail-side").unwrap();
            assert!(
                content.right() <= viewport.right() && side.right() <= viewport.right(),
                "{tab:?}: detail columns fit the viewport"
            );
            assert!(
                controls.bottom() <= viewport.top(),
                "{tab:?}: controls stay above content"
            );
            assert_eq!(
                vcx.debug_bounds("pr-detail-scrollbar").unwrap(),
                viewport,
                "{tab:?}: scrollbar stays within the detail viewport"
            );
            assert!(
                viewport.bottom() <= px(500.),
                "{tab:?}: viewport={viewport:?}"
            );
            assert!(
                scroll.max_offset().y > px(0.),
                "{tab:?}: content overflows: {viewport:?}"
            );
            let before = scroll.offset().y;
            vcx.simulate_event(ScrollEvent {
                source: ScrollSource::Pointer,
                position: viewport.center(),
                delta: ScrollDelta::Pixels(point(px(0.), px(-200.))),
                touch_phase: TouchPhase::Started,
                modifiers: Default::default(),
            });
            vcx.run_until_parked();
            assert!(
                scroll.offset().y < before,
                "{tab:?}: wheel moves the detail"
            );
            assert_eq!(
                vcx.debug_bounds("pr-detail-controls").unwrap(),
                controls,
                "{tab:?}: title, actions and tabs stay fixed"
            );
            let before_click = scroll.offset().y;
            vcx.simulate_click(
                point(
                    viewport.right() - px(crate::ui::theme::SCROLLBAR_MARGIN_R),
                    viewport.bottom() - px(crate::ui::theme::CONTROL_TARGET_SIZE),
                ),
                Default::default(),
            );
            vcx.run_until_parked();
            assert!(
                scroll.offset().y < before_click,
                "{tab:?}: scrollbar track moves the detail"
            );
        }
    }
}

#[gpui::test]
fn a_read_landing_after_its_detail_was_replaced_or_closed_is_dropped(cx: &mut TestAppContext) {
    let (f, gh, _) = window(cx);
    // #7 answers; #8 is unknown to GitHub and fails. Replaced before either
    // lands, the shown #8 must not take #7's answer.
    open(&f, item(7, "feat"), cx);
    let first = id(&f, cx);
    open(&f, item(8, "other"), cx);
    let second = id(&f, cx);
    assert_ne!(first, second, "ids are never reused");
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::PrDetail), 2);
    assert!(
        matches!(shown(&f, cx), Some((id, 8, Load::Failed(_))) if id == second),
        "{:?}",
        shown(&f, cx)
    );

    // Closed before its answer lands: nothing comes back.
    open(&f, item(7, "feat"), cx);
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| f.ws.update(cx, |ws, cx| ws.back_to_pr_list(window, cx)));
    cx.run_until_parked();
    assert_eq!(shown(&f, cx).map(|(_, number, _)| number), None);
}

#[gpui::test]
fn a_threads_read_outdated_by_a_refresh_does_not_land(cx: &mut TestAppContext) {
    let (f, gh, _) = window(cx);
    let id = read_detail(&f, cx);
    let thread = |path: &str| daruda_github::ReviewThreads {
        threads: vec![daruda_github::ReviewThread {
            path: path.into(),
            lines: None,
            outdated: false,
            comments: Vec::new(),
            more_comments: false,
            ..Default::default()
        }],
        truncated: false,
    };
    gh.set_review_threads(&repo(), 7, thread("new.rs"));
    let threads = |f: &GitFixture, cx: &mut TestAppContext| {
        f.ws.read_with(cx, |ws, _| {
            let detail = ws.pages.pull_requests.detail.as_ref().unwrap();
            detail.threads().cloned()
        })
    };
    // The tab's first read, then a refresh's second, neither answered yet.
    f.ws.update(cx, |ws, cx| {
        ws.select_pr_detail_tab(id, PrTab::Threads, cx);
        let detail = ws.pages.pull_requests.detail.as_ref().unwrap();
        let first = detail.threads_ticket().unwrap();
        ws.refresh_pr_detail(id, cx);
        // The first read answers late, into the slot the second waits in.
        ws.land_pr_threads(id, first, Ok(thread("old.rs")), cx);
    });
    assert_eq!(
        threads(&f, cx),
        Some(Load::Loading),
        "the old answer is dropped"
    );
    cx.run_until_parked();
    assert_eq!(threads(&f, cx), Some(Load::Ready(thread("new.rs"))));
}

#[gpui::test]
fn a_failed_check_opens_its_cleaned_log(cx: &mut TestAppContext) {
    let (f, gh, _) = window(cx);
    open(&f, item(7, "feat"), cx);
    cx.run_until_parked();
    let id = id(&f, cx);
    f.ws.update(cx, |ws, cx| ws.toggle_pr_check_log(id, JOB, cx));
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::FailedLog), 1);
    let DetailBody::Ready(ready) = view(&f, cx).body else {
        panic!("not read");
    };
    let LogSlot::Open(_, Load::Ready(log)) = &ready.checks[0].log else {
        panic!("{:?}", ready.checks[0].log);
    };
    assert!(log.contains("test parse ... FAILED"), "{log}");
    assert!(!log.contains('\u{1b}'), "no escapes reach the page: {log}");

    f.ws.update(cx, |ws, cx| ws.toggle_pr_check_log(id, JOB, cx));
    let DetailBody::Ready(ready) = view(&f, cx).body else {
        panic!("not read");
    };
    assert_eq!(ready.checks[0].log, LogSlot::Closed(JOB));
}

#[gpui::test]
fn send_to_agent_goes_to_the_linked_worktrees_chat(cx: &mut TestAppContext) {
    let (f, gh, linked) = window(cx);
    split_chat(&f, cx);
    // Standing on another worktree shows the prompt goes to the linked one.
    let other_dir = f.root.parent().unwrap().join("other");
    std::fs::create_dir_all(&other_dir).unwrap();
    let other = LaneRef {
        project: linked.project,
        lane: 77,
    };
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            let lane = crate::lane::Lane::default_for_project(77, other_dir.clone());
            ws.project_for_mut(linked.project).unwrap().lanes.push(lane);
            ws.activate_lane(other, window, cx);
            ws.show_page(Page::PullRequests, cx);
        })
    });
    open(&f, item(7, "feat"), cx);
    cx.run_until_parked();
    let DetailBody::Ready(ready) = view(&f, cx).body else {
        panic!("not read");
    };
    assert_eq!(ready.checks_send, SendSlot::Ready);
    let id = id(&f, cx);

    // What goes where is prepared and shown first: the page stays.
    f.ws.update(cx, |ws, cx| ws.open_pr_handoff(id, Read::FailedChecks, cx));
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::FailedLog), 1);
    let handoff = view(&f, cx).handoff.expect("the handoff is shown");
    assert_eq!(handoff.chats.len(), 1, "the linked worktree's one chat");
    assert_eq!(handoff.chosen, Some(handoff.chats[0].pane));
    let Prepared::Ready(prompt) = &handoff.prompt else {
        panic!("prepared: {:?}", handoff.prompt);
    };
    assert!(prompt.contains("test parse ... FAILED"), "{prompt}");
    assert_eq!(input(&f, cx), "", "nothing inserted before asked");
    assert_eq!(f.ws.read_with(cx, |ws, _| ws.active_ref()), other);

    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| ws.insert_pr_handoff(id, window, cx));
    });
    cx.run_until_parked();
    f.ws.read_with(cx, |ws, _| {
        assert_eq!(ws.active_ref(), linked, "the linked worktree is shown");
        assert_eq!(ws.active_page(), None, "so its chat input is in view");
        assert!(ws.pr_detail_id().is_some(), "the detail waits on the page");
    });
    let input = input(&f, cx);
    assert!(input.contains("Pull request #7"), "{input}");
    assert!(input.contains("test parse ... FAILED"), "{input}");
}

#[gpui::test]
fn without_a_linked_worktree_send_to_agent_is_off(cx: &mut TestAppContext) {
    let (f, gh, _) = window(cx);
    split_chat(&f, cx);
    f.ws.update(cx, |ws, cx| ws.show_page(Page::PullRequests, cx));
    gh.set_pr_detail(&repo(), 8, detail(8, "other"));
    open(&f, item(8, "other"), cx);
    cx.run_until_parked();
    let DetailBody::Ready(ready) = view(&f, cx).body else {
        panic!("not read");
    };
    assert_eq!(ready.checks_send, SendSlot::NoLane);
    assert_eq!(ready.threads_send, SendSlot::NoLane);
    assert!(!ready.checks_send.enabled());

    let id = id(&f, cx);
    f.ws.update(cx, |ws, cx| ws.open_pr_handoff(id, Read::FailedChecks, cx));
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::FailedLog), 0);
    assert!(
        view(&f, cx).handoff.is_none(),
        "no worktree, nowhere to hand to"
    );
    assert_eq!(input(&f, cx), "");
    assert_eq!(
        f.ws.read_with(cx, |ws, _| ws.active_page()),
        Some(Page::PullRequests)
    );
}

/// What sending said to the user, by title.
fn send_reports(f: &GitFixture, cx: &mut TestAppContext) -> Vec<String> {
    f.ws.read_with(cx, |ws, _| {
        ws.error_history()
            .iter()
            .filter(|r| r.dedup_key.as_deref() == Some("github.send"))
            .map(|r| r.title.clone())
            .collect()
    })
}

/// The detail of #7, read, on the page.
fn read_detail(f: &GitFixture, cx: &mut TestAppContext) -> PrDetailId {
    open(f, item(7, "feat"), cx);
    cx.run_until_parked();
    id(f, cx)
}

#[gpui::test]
fn a_worktree_without_a_chat_is_named_and_nothing_can_be_inserted(cx: &mut TestAppContext) {
    let (f, _, _) = window(cx);
    let id = read_detail(&f, cx);
    f.ws.update(cx, |ws, cx| ws.open_pr_handoff(id, Read::FailedChecks, cx));
    cx.run_until_parked();
    let handoff = view(&f, cx).handoff.expect("shown");
    assert!(handoff.chats.is_empty());
    assert_eq!(handoff.chosen, None, "nothing to insert into");
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| ws.insert_pr_handoff(id, window, cx));
    });
    cx.run_until_parked();
    assert_eq!(input(&f, cx), "");
}

#[gpui::test]
fn inserting_shows_the_worktree_whose_chat_takes_the_prompt(cx: &mut TestAppContext) {
    let (f, _, linked) = window(cx);
    split_chat(&f, cx);
    f.ws.update(cx, |ws, cx| ws.show_page(Page::PullRequests, cx));
    let id = read_detail(&f, cx);
    f.ws.update(cx, |ws, cx| ws.open_pr_handoff(id, Read::FailedChecks, cx));
    cx.run_until_parked();
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| ws.insert_pr_handoff(id, window, cx));
    });
    cx.run_until_parked();
    f.ws.read_with(cx, |ws, _| {
        assert_eq!(ws.active_ref(), linked);
        assert_eq!(ws.active_page(), None, "the page gives way to the chat");
    });
    assert!(input(&f, cx).contains("Pull request #7"));
    assert!(view_handoff_closed(&f, cx), "inserted: the handoff closes");
}

fn view_handoff_closed(f: &GitFixture, cx: &mut TestAppContext) -> bool {
    f.ws.read_with(cx, |ws, _| {
        ws.pages
            .pull_requests
            .detail
            .as_ref()
            .expect("the detail stays")
            .handoff()
            .is_none()
    })
}

fn insert(f: &GitFixture, id: PrDetailId, cx: &mut TestAppContext) {
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| ws.insert_pr_handoff(id, window, cx));
    });
    cx.run_until_parked();
}

fn handoff(f: &GitFixture, id: PrDetailId, cx: &mut TestAppContext) {
    f.ws.update(cx, |ws, cx| ws.open_pr_handoff(id, Read::FailedChecks, cx));
    cx.run_until_parked();
}

/// A linked worktree never shown in this window has no chat: one is
/// opened there and takes the prompt, unsent.
#[gpui::test]
fn a_new_chat_takes_the_prompt_in_a_worktree_never_shown(cx: &mut TestAppContext) {
    let (f, _, linked) = window(cx);
    let other_dir = f.root.parent().unwrap().join("other");
    std::fs::create_dir_all(&other_dir).unwrap();
    let other = LaneRef {
        project: linked.project,
        lane: 77,
    };
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            let lane = crate::lane::Lane::default_for_project(77, other_dir.clone());
            ws.project_for_mut(linked.project).unwrap().lanes.push(lane);
            ws.activate_lane(other, window, cx);
            ws.main_area.runtimes.remove(&linked);
            ws.show_page(Page::PullRequests, cx);
        })
    });
    let id = read_detail(&f, cx);
    handoff(&f, id, cx);
    let shown = view(&f, cx).handoff.expect("shown");
    assert!(shown.chats.is_empty());
    assert!(matches!(shown.prompt, Prepared::Ready(_)));

    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            ws.insert_pr_handoff_in_new_chat(id, window, cx)
        });
    });
    cx.run_until_parked();
    f.ws.read_with(cx, |ws, cx| {
        assert_eq!(ws.active_ref(), linked, "the worktree is shown");
        assert_eq!(ws.active_page(), None);
        assert_eq!(ws.writable_chats(linked, cx).len(), 1, "one chat opened");
    });
    assert_eq!(
        cx.update(super::Workspace::faked_connect_preferences_for_test)
            .len(),
        1,
        "the new chat connects"
    );
    assert!(
        input(&f, cx).contains("Pull request #7"),
        "{}",
        input(&f, cx)
    );
    assert!(view_handoff_closed(&f, cx));
}

/// A worktree whose folder is gone gets no chat, and is not shown.
#[gpui::test]
fn a_missing_worktree_folder_opens_no_chat(cx: &mut TestAppContext) {
    let (f, _, linked) = window(cx);
    let id = read_detail(&f, cx);
    handoff(&f, id, cx);
    let path =
        f.ws.read_with(cx, |ws, _| ws.lane_for(linked).unwrap().path.clone());
    std::fs::remove_dir_all(&path).unwrap();
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            ws.insert_pr_handoff_in_new_chat(id, window, cx)
        });
    });
    cx.run_until_parked();
    f.ws.read_with(cx, |ws, cx| {
        assert!(ws.writable_chats(linked, cx).is_empty());
        assert_eq!(ws.active_page(), Some(Page::PullRequests), "the page stays");
        assert!(
            ws.error_history()
                .iter()
                .any(|r| r.dedup_key.as_deref() == Some("github.handoff.unavailable"))
        );
    });
}

/// The pull request read again while a handoff is open: the failing
/// checks are prepared again with it, the chat chosen kept, so the prompt
/// is never older than the page and nothing goes in while it is read.
#[gpui::test]
fn a_refresh_prepares_an_open_handoff_again(cx: &mut TestAppContext) {
    let (f, gh, _) = window(cx);
    split_chat(&f, cx);
    f.ws.update(cx, |ws, cx| ws.show_page(Page::PullRequests, cx));
    let id = read_detail(&f, cx);
    handoff(&f, id, cx);
    let chosen = view(&f, cx).handoff.expect("shown").chosen;
    assert_eq!(gh.count(GhOp::FailedLog), 1);
    gh.set_failed_log(
        JOB,
        "job\tstep\t2026-01-02T00:00:00.0000000Z test lex ... FAILED",
    );
    f.ws.update(cx, |ws, cx| ws.refresh_pr_detail(id, cx));
    assert!(
        matches!(
            view(&f, cx).handoff.expect("still shown").prompt,
            Prepared::Reading(_)
        ),
        "read again before anything goes in"
    );
    insert(&f, id, cx);
    assert_eq!(input(&f, cx), "", "nothing inserted while it is read");
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::FailedLog), 2, "the log is read again");
    let shown = view(&f, cx).handoff.expect("still shown");
    assert_eq!(shown.chosen, chosen, "the chat chosen stays");
    let Prepared::Ready(prompt) = &shown.prompt else {
        panic!("prepared: {:?}", shown.prompt);
    };
    assert!(prompt.contains("test lex ... FAILED"), "{prompt}");
    assert!(
        !prompt.contains("test parse"),
        "the old log is gone: {prompt}"
    );
}

/// Read closed or merged since: the handoff of its failing checks closes,
/// and none is offered, as nobody is left to fix them.
#[gpui::test]
fn a_pull_request_closed_since_takes_its_handoff_away(cx: &mut TestAppContext) {
    let (f, gh, _) = window(cx);
    split_chat(&f, cx);
    f.ws.update(cx, |ws, cx| ws.show_page(Page::PullRequests, cx));
    let id = read_detail(&f, cx);
    handoff(&f, id, cx);
    assert!(view(&f, cx).handoff.is_some());
    let mut closed = detail(7, "feat");
    closed.summary.state = PrState::Closed;
    gh.set_pr_detail(&repo(), 7, closed);
    f.ws.update(cx, |ws, cx| ws.refresh_pr_detail(id, cx));
    cx.run_until_parked();
    assert!(view_handoff_closed(&f, cx), "closed with it");
    insert(&f, id, cx);
    assert_eq!(input(&f, cx), "");
    let DetailBody::Ready(ready) = view(&f, cx).body else {
        panic!("not read");
    };
    assert_eq!(ready.checks_send, SendSlot::NotOpen);
    assert_eq!(ready.threads_send, SendSlot::NotOpen);
}

/// A chat appearing while the panel was open is chosen among, not
/// joined by another.
#[gpui::test]
fn no_new_chat_is_opened_beside_one(cx: &mut TestAppContext) {
    let (f, _, linked) = window(cx);
    let id = read_detail(&f, cx);
    handoff(&f, id, cx);
    split_chat(&f, cx);
    f.ws.update(cx, |ws, cx| ws.show_page(Page::PullRequests, cx));
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            ws.insert_pr_handoff_in_new_chat(id, window, cx)
        });
    });
    cx.run_until_parked();
    f.ws.read_with(cx, |ws, cx| {
        assert_eq!(ws.writable_chats(linked, cx).len(), 1);
    });
    assert_eq!(input(&f, cx), "");
    assert_eq!(view(&f, cx).handoff.unwrap().chats.len(), 1);
}

#[gpui::test]
fn the_chat_chosen_takes_the_prompt(cx: &mut TestAppContext) {
    let (f, _, _) = window(cx);
    split_chat(&f, cx);
    split_chat(&f, cx);
    f.ws.update(cx, |ws, cx| ws.show_page(Page::PullRequests, cx));
    let id = read_detail(&f, cx);
    handoff(&f, id, cx);
    let shown = view(&f, cx).handoff.expect("shown");
    assert_eq!(shown.chats.len(), 2, "both chats are offered");
    let second = shown.chats[1].pane;
    f.ws.update(cx, |ws, cx| ws.choose_pr_handoff_chat(id, second, cx));
    assert_eq!(view(&f, cx).handoff.unwrap().chosen, Some(second));
    insert(&f, id, cx);
    assert_eq!(
        f.ws.read_with(cx, |ws, _| ws.active_runtime().focused_pane_id),
        second,
        "the chosen chat is the one written into"
    );
    assert!(input(&f, cx).contains("Pull request #7"));
}

#[gpui::test]
fn a_chosen_chat_closed_is_not_swapped_for_another(cx: &mut TestAppContext) {
    let (f, _, _) = window(cx);
    split_chat(&f, cx);
    split_chat(&f, cx);
    f.ws.update(cx, |ws, cx| ws.show_page(Page::PullRequests, cx));
    let id = read_detail(&f, cx);
    handoff(&f, id, cx);
    let chosen = view(&f, cx).handoff.unwrap().chosen.expect("one chosen");
    cx.update_window(f.window.into(), |_, window, cx| {
        f.ws.update(cx, |ws, cx| ws.close_pane_by_id(chosen, window, cx));
    })
    .unwrap();
    f.ws.update(cx, |ws, cx| ws.show_page(Page::PullRequests, cx));
    let shown = view(&f, cx).handoff.expect("still shown");
    assert_eq!(shown.blocked, Some(HandoffBlock::ChatGone), "{shown:?}");
    assert_eq!(shown.chosen, None, "Insert is off until another is chosen");
    insert(&f, id, cx);
    assert_eq!(input(&f, cx), "", "nothing goes to a chat not chosen");
    // The last chat closed too: a new one is offered, not another choice.
    let last = view(&f, cx).handoff.unwrap().chats[0].pane;
    cx.update_window(f.window.into(), |_, window, cx| {
        f.ws.update(cx, |ws, cx| ws.close_pane_by_id(last, window, cx));
    })
    .unwrap();
    f.ws.update(cx, |ws, cx| ws.show_page(Page::PullRequests, cx));
    let shown = view(&f, cx).handoff.expect("still shown");
    assert!(
        shown.chats.is_empty() && shown.blocked.is_none(),
        "{shown:?}"
    );
    assert_eq!(
        f.ws.read_with(cx, |ws, _| ws.active_page()),
        Some(Page::PullRequests)
    );
}

#[gpui::test]
fn a_worktree_unlinked_meanwhile_takes_nothing(cx: &mut TestAppContext) {
    let (f, gh, _) = window(cx);
    split_chat(&f, cx);
    f.ws.update(cx, |ws, cx| ws.show_page(Page::PullRequests, cx));
    let id = read_detail(&f, cx);
    handoff(&f, id, cx);
    // GitHub now has another pull request on the worktree's branch.
    gh.set_prs(&repo(), vec![summary(9, "feat")]);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(31));
    cx.update(crate::github::PrPoller::tick);
    cx.run_until_parked();
    let shown = view(&f, cx).handoff.expect("still shown");
    assert_eq!(shown.blocked, Some(HandoffBlock::Moved), "{shown:?}");
    insert(&f, id, cx);
    assert_eq!(input(&f, cx), "");
}

#[gpui::test]
fn an_answer_for_a_closed_handoff_is_dropped(cx: &mut TestAppContext) {
    let (f, gh, _) = window(cx);
    split_chat(&f, cx);
    f.ws.update(cx, |ws, cx| ws.show_page(Page::PullRequests, cx));
    let id = read_detail(&f, cx);
    f.ws.update(cx, |ws, cx| {
        ws.open_pr_handoff(id, Read::FailedChecks, cx);
        ws.close_pr_handoff(id, cx);
        ws.open_pr_handoff(id, Read::OpenThreads, cx);
    });
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::FailedLog), 1, "the first was asked for");
    let shown = view(&f, cx).handoff.expect("the second is shown");
    assert_eq!(shown.payload, Payload::OpenThreads);
    assert!(
        !matches!(&shown.prompt, Prepared::Ready(p) if p.contains("FAILED")),
        "the checks prompt does not land on the threads handoff: {:?}",
        shown.prompt
    );
}

#[gpui::test]
fn a_terminal_is_not_offered(cx: &mut TestAppContext) {
    let (f, _, _) = window(cx);
    split_chat(&f, cx);
    cx.update_window(f.window.into(), |_, window, cx| {
        f.ws.update(cx, |ws, cx| {
            ws.split_focused_pane_kind(
                super::NewPaneKind::Terminal,
                super::SplitDirection::Horizontal,
                window,
                cx,
            );
        });
    })
    .unwrap();
    f.ws.update(cx, |ws, cx| ws.show_page(Page::PullRequests, cx));
    let id = read_detail(&f, cx);
    handoff(&f, id, cx);
    assert_eq!(
        view(&f, cx).handoff.unwrap().chats.len(),
        1,
        "the chat only"
    );
}

/// The Git panel's Send checks nothing for a worktree with no chat: the
/// worktree is shown so one can be opened there.
#[gpui::test]
fn the_git_panel_names_a_worktree_without_a_chat(cx: &mut TestAppContext) {
    let (f, gh, linked) = window(cx);
    f.ws.update(cx, |ws, cx| ws.show_page(Page::Tasks, cx));
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| ws.send_failed_checks(window, cx));
    });
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::FailedLog), 0, "nothing is read for nowhere");
    assert_eq!(
        send_reports(&f, cx),
        [crate::surface::strings::error::github_no_agent_pane()]
    );
    f.ws.read_with(cx, |ws, _| {
        assert_eq!(ws.active_ref(), linked);
        assert_eq!(ws.active_page(), None, "the worktree is in view");
    });
}

/// A page opened while the Git panel's prompt was read keeps it out of an
/// input out of sight.
#[gpui::test]
fn the_git_panel_does_not_put_a_prompt_behind_a_page(cx: &mut TestAppContext) {
    let (f, gh, _) = window(cx);
    split_chat(&f, cx);
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            ws.send_failed_checks(window, cx);
            // A page before the logs arrive.
            ws.show_page(Page::PullRequests, cx);
        });
    });
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::FailedLog), 1);
    assert_eq!(input(&f, cx), "", "the input is out of sight");
    assert_eq!(
        send_reports(&f, cx),
        [crate::surface::strings::error::github_lane_changed()]
    );
    assert!(
        !f.ws
            .read_with(cx, |ws, _| ws.is_sending_pr(ws.active_ref())),
        "the buttons are free again"
    );
}

/// The Git panel stays in the left dock while a page fills the main area,
/// so its buttons can be pressed there. The prompt is put into the active
/// lane's input unsent, to be read first: the page gives way to the lane
/// rather than hide what was just asked for.
#[gpui::test]
fn the_git_panels_send_closes_a_page_to_show_the_prompt(cx: &mut TestAppContext) {
    let (f, gh, linked) = window(cx);
    split_chat(&f, cx);
    f.ws.update(cx, |ws, cx| ws.show_page(Page::Tasks, cx));
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| ws.send_failed_checks(window, cx));
    });
    cx.run_until_parked();

    assert_eq!(gh.count(GhOp::FailedLog), 1);
    f.ws.read_with(cx, |ws, _| {
        assert_eq!(ws.active_ref(), linked);
        assert_eq!(ws.active_page(), None, "the page gave way to the worktree");
    });
    let input = input(&f, cx);
    assert!(input.contains("Pull request #7"), "{input}");
    assert_eq!(send_reports(&f, cx), Vec::<String>::new());
}

/// A detail GitHub lets merge with a merge commit or a squash, whose failed
/// checks are two jobs of run 11 and one of run 12.
pub(in crate::workspace) fn mergeable(number: u64, head: &str) -> PrDetail {
    let mut detail = detail(number, head);
    detail.methods = vec![MergeMethod::Merge, MergeMethod::Squash];
    let failed = |job: ActionsJob| {
        let mut entry = detail.checks[0].clone();
        entry.job = Some(job);
        entry
    };
    detail.checks = vec![
        failed(JOB),
        failed(ActionsJob { run: 11, job: 23 }),
        failed(ActionsJob { run: 12, job: 30 }),
    ];
    detail
}

/// The detail of #7, read, with `detail` as GitHub's answer.
pub(in crate::workspace) fn opened(
    cx: &mut TestAppContext,
    detail: PrDetail,
) -> (GitFixture, FakeGh, PrDetailId) {
    let (f, gh, _) = window(cx);
    gh.set_pr_detail(&repo(), 7, detail);
    open(&f, item(7, "feat"), cx);
    cx.run_until_parked();
    let id = id(&f, cx);
    (f, gh, id)
}

/// Run `ask` in the window and say whether it raised a dialog.
fn asks(
    f: &GitFixture,
    cx: &mut TestAppContext,
    ask: impl FnOnce(&mut super::Workspace, &mut gpui::Window, &mut gpui::Context<super::Workspace>),
) -> bool {
    use crate::ui::WindowExt as _;
    cx.update_window(f.window.into(), |_, window, cx| {
        f.ws.update(cx, |ws, cx| ask(ws, window, cx));
        window.has_active_dialog(cx)
    })
    .unwrap()
}

/// The dialog's OK.
fn confirm(f: &GitFixture, cx: &mut TestAppContext) {
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
}

fn merges(gh: &FakeGh) -> Vec<GhCall> {
    gh.calls()
        .into_iter()
        .filter(|call| matches!(call, GhCall::MergePr { .. }))
        .collect()
}

fn loaded_state(f: &GitFixture, cx: &mut TestAppContext) -> Option<PrState> {
    match shown(f, cx)?.2 {
        Load::Ready(loaded) => Some(loaded.summary.state),
        Load::Loading | Load::Failed(_) => None,
    }
}

/// What the actions reported; the fixture's own git watching can fail too.
fn error_titles(f: &GitFixture, cx: &mut TestAppContext) -> Vec<String> {
    f.ws.read_with(cx, |ws, _| {
        ws.error_history()
            .iter()
            .filter(|r| {
                r.dedup_key
                    .as_deref()
                    .is_some_and(|key| key.starts_with("github.pr_action"))
            })
            .map(|r| r.title.clone())
            .collect()
    })
}

#[gpui::test]
fn merge_asks_first_then_merges_once_with_the_chosen_method_at_the_shown_head(
    cx: &mut TestAppContext,
) {
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    let reads = gh.count(GhOp::PrDetail);
    assert!(asks(&f, cx, |ws, window, cx| {
        ws.ask_pr_merge(id, MergeMethod::Merge, window, cx)
    }));
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::MergePr), 0, "nothing merges before OK");

    confirm(&f, cx);
    assert_eq!(
        merges(&gh),
        [GhCall::MergePr {
            repo: repo(),
            number: 7,
            method: MergeMethod::Merge,
            head_sha: "abc".into(),
        }]
    );
    assert_eq!(
        gh.count(GhOp::PrDetail),
        reads + 1,
        "the detail is read again"
    );
    assert_eq!(loaded_state(&f, cx), Some(PrState::Merged));
    assert!(error_titles(&f, cx).is_empty());
}

#[gpui::test]
fn a_moved_head_says_so_and_reloads_the_detail(cx: &mut TestAppContext) {
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    // Pushed to after the detail was read.
    let mut moved = mergeable(7, "feat");
    moved.head_sha = "def".into();
    gh.set_pr_detail(&repo(), 7, moved);
    let reads = gh.count(GhOp::PrDetail);

    assert!(asks(&f, cx, |ws, window, cx| {
        ws.ask_pr_merge(id, MergeMethod::Squash, window, cx)
    }));
    confirm(&f, cx);
    assert_eq!(gh.count(GhOp::MergePr), 1);
    assert_eq!(
        error_titles(&f, cx),
        [crate::surface::strings::error::github_pr_head_moved(7)]
    );
    assert_eq!(
        gh.count(GhOp::PrDetail),
        reads + 1,
        "the detail is reloaded"
    );
    let Some((_, _, Load::Ready(loaded))) = shown(&f, cx) else {
        panic!("not read");
    };
    assert_eq!(loaded.head_sha, "def", "it shows the head that moved");
    assert_eq!(loaded.summary.state, PrState::Open);
}

#[gpui::test]
fn every_action_is_off_while_one_runs(cx: &mut TestAppContext) {
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    f.ws.update(cx, |ws, cx| ws.rerun_pr_detail_checks(id, cx));
    let shown = view(&f, cx);
    let actions = shown
        .header
        .actions()
        .cloned()
        .expect("an open pull request has actions");
    assert_eq!(actions.merge.slot, ActionSlot::Waiting);
    assert_eq!(actions.close, ActionSlot::Waiting);
    let DetailBody::Ready(ready) = shown.body else {
        panic!("not read");
    };
    assert_eq!(ready.rerun, Some(ActionSlot::Running));

    // Asked again, or for another action, while it runs: nothing starts.
    f.ws.update(cx, |ws, cx| ws.rerun_pr_detail_checks(id, cx));
    assert!(!asks(&f, cx, |ws, window, cx| ws.ask_pr_close(id, window, cx)));
    assert!(!asks(&f, cx, |ws, window, cx| {
        ws.ask_pr_merge(id, MergeMethod::Merge, window, cx)
    }));
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::ClosePr) + gh.count(GhOp::MergePr), 0);
    let actions = view(&f, cx).header.actions().cloned().expect("still open");
    assert_eq!(actions.merge.slot, ActionSlot::Ready);
    assert_eq!(actions.close, ActionSlot::Ready);
}

#[gpui::test]
fn ready_close_and_rerun_each_ask_github_once(cx: &mut TestAppContext) {
    let mut draft = mergeable(7, "feat");
    draft.summary.state = PrState::Draft;
    let (f, gh, id) = opened(cx, draft);

    f.ws.update(cx, |ws, cx| ws.rerun_pr_detail_checks(id, cx));
    cx.run_until_parked();
    let reruns: Vec<u64> = gh
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            GhCall::RerunFailed { run, .. } => Some(run),
            _ => None,
        })
        .collect();
    assert_eq!(reruns, [11, 12], "two failed jobs of run 11 are one rerun");

    assert!(
        view(&f, cx)
            .header
            .actions()
            .cloned()
            .expect("a draft")
            .ready
            .is_some()
    );
    f.ws.update(cx, |ws, cx| ws.mark_pr_detail_ready(id, cx));
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::SetReady), 1);
    assert_eq!(loaded_state(&f, cx), Some(PrState::Open));
    assert_eq!(
        view(&f, cx).header.actions().cloned().expect("open").ready,
        None
    );

    assert!(asks(&f, cx, |ws, window, cx| ws.ask_pr_close(id, window, cx)));
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::ClosePr), 0, "nothing closes before OK");
    confirm(&f, cx);
    assert_eq!(gh.count(GhOp::ClosePr), 1);
    assert_eq!(loaded_state(&f, cx), Some(PrState::Closed));
    assert_eq!(
        view(&f, cx).header.actions().cloned(),
        None,
        "a closed pull request has none"
    );
    assert_eq!(
        view(&f, cx).header.reopen().cloned(),
        Some(ActionSlot::Ready)
    );
    assert!(error_titles(&f, cx).is_empty());

    // Reopening asks nothing first and brings the actions back.
    f.ws.update(cx, |ws, cx| ws.reopen_pr_detail(id, cx));
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::ReopenPr), 1);
    assert_eq!(loaded_state(&f, cx), Some(PrState::Open));
    assert_eq!(view(&f, cx).header.reopen().cloned(), None);
    assert!(view(&f, cx).header.actions().cloned().is_some());
}

/// What GitHub does not let the viewer do is off, with the reason, and
/// never asked of GitHub.
#[gpui::test]
fn close_and_reopen_follow_the_viewers_permission(cx: &mut TestAppContext) {
    let mut open = mergeable(7, "feat");
    open.viewer_can = open
        .viewer_can
        .with(daruda_github::Permission::Close, false);
    let (f, gh, id) = opened(cx, open.clone());
    assert!(matches!(
        view(&f, cx).header.actions().cloned().expect("open").close,
        ActionSlot::Blocked(_)
    ));
    let _ = asks(&f, cx, |ws, window, cx| ws.ask_pr_close(id, window, cx));
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::ClosePr), 0);

    let mut closed = open;
    closed.summary.state = PrState::Closed;
    closed.viewer_can = closed
        .viewer_can
        .with(daruda_github::Permission::Reopen, false);
    gh.set_pr_detail(&repo(), 7, closed);
    f.ws.update(cx, |ws, cx| ws.refresh_pr_detail(id, cx));
    cx.run_until_parked();
    assert!(matches!(
        view(&f, cx).header.reopen().cloned(),
        Some(ActionSlot::Blocked(_))
    ));
    f.ws.update(cx, |ws, cx| ws.reopen_pr_detail(id, cx));
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::ReopenPr), 0);
}

/// GitHub refusing a reopen (a deleted head branch, say) leaves it closed,
/// Reopen offered again, and says what `gh` said.
#[gpui::test]
fn a_refused_reopen_keeps_it_closed(cx: &mut TestAppContext) {
    let mut closed = mergeable(7, "feat");
    closed.summary.state = PrState::Closed;
    let (f, gh, id) = opened(cx, closed);
    gh.fail(
        GhOp::ReopenPr,
        GhFailure::Exit {
            code: Some(1),
            stderr: "Could not reopen: the head branch was deleted".into(),
        },
    );
    f.ws.update(cx, |ws, cx| ws.reopen_pr_detail(id, cx));
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::ReopenPr), 1);
    assert_eq!(loaded_state(&f, cx), Some(PrState::Closed));
    let shown = view(&f, cx);
    assert_eq!(shown.header.reopen().cloned(), Some(ActionSlot::Ready));
    let error = shown.error().expect("said under the header");
    assert!(error.contains("head branch was deleted"), "{error}");
    assert_eq!(
        error_titles(&f, cx),
        [crate::surface::strings::error::github_pr_reopen_failed(7)]
    );
}

#[gpui::test]
fn a_merged_pull_request_cannot_be_reopened(cx: &mut TestAppContext) {
    let mut merged = mergeable(7, "feat");
    merged.summary.state = PrState::Merged;
    // GitHub can still answer a reason for one no longer open.
    merged.mergeability = Mergeability::Conflicting;
    let (f, _, _) = opened(cx, merged);
    assert_eq!(view(&f, cx).header.reopen().cloned(), None);
    assert_eq!(view(&f, cx).header.actions().cloned(), None);
    let meta = view(&f, cx).meta;
    assert!(
        !meta.contains(&crate::surface::strings::pull_requests::merge_conflicting()),
        "no merge reason on a merged pull request: {meta}"
    );
}

#[gpui::test]
fn a_failed_action_reports_what_gh_said(cx: &mut TestAppContext) {
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    gh.fail(
        GhOp::RerunFailed,
        GhFailure::Exit {
            code: Some(1),
            stderr: "run 11 cannot be rerun".into(),
        },
    );
    let reads = gh.count(GhOp::PrDetail);
    f.ws.update(cx, |ws, cx| ws.rerun_pr_detail_checks(id, cx));
    cx.run_until_parked();
    let report = f.ws.read_with(cx, |ws, _| {
        ws.error_history()
            .iter()
            .find(|r| r.dedup_key.as_deref() == Some("github.pr_action"))
            .cloned()
            .expect("the failure is reported")
    });
    assert_eq!(
        report.title,
        crate::surface::strings::error::github_pr_rerun_failed(7)
    );
    assert!(
        report.message.contains("run 11 cannot be rerun"),
        "{}",
        report.message
    );
    assert_eq!(
        gh.count(GhOp::PrDetail),
        reads + 1,
        "a failure leaves GitHub's state unknown, so the detail is read again"
    );
    let shown = view(&f, cx);
    let error = shown
        .error()
        .clone()
        .expect("the detail keeps what gh said");
    assert!(error.contains("run 11 cannot be rerun"), "{error}");
    let DetailBody::Ready(ready) = shown.body else {
        panic!("not read");
    };
    assert_eq!(
        ready.rerun,
        Some(ActionSlot::Ready),
        "and can be tried again"
    );

    // The next action clears it while it runs; a reload clears it too.
    gh.clear_failure(GhOp::RerunFailed);
    f.ws.update(cx, |ws, cx| ws.rerun_pr_detail_checks(id, cx));
    assert_eq!(view(&f, cx).error(), None);
    cx.run_until_parked();
    assert_eq!(view(&f, cx).error(), None, "it succeeded");
    gh.fail(
        GhOp::RerunFailed,
        GhFailure::Exit {
            code: Some(1),
            stderr: "again".into(),
        },
    );
    f.ws.update(cx, |ws, cx| ws.rerun_pr_detail_checks(id, cx));
    cx.run_until_parked();
    assert!(view(&f, cx).error().is_some());
    f.ws.update(cx, |ws, cx| ws.refresh_pr_detail(id, cx));
    assert_eq!(view(&f, cx).error(), None, "a reload clears it");
}

#[gpui::test]
fn merge_goes_to_github_even_when_it_may_refuse(cx: &mut TestAppContext) {
    let mut blocked = mergeable(7, "feat");
    blocked.mergeability = Mergeability::Blocked(daruda_github::BlockReason::Requirements);
    let (f, gh, id) = opened(cx, blocked);
    let actions = view(&f, cx).header.actions().cloned().expect("open");
    assert_eq!(actions.merge.slot, ActionSlot::Ready);
    gh.fail(
        GhOp::MergePr,
        GhFailure::Exit {
            code: Some(1),
            stderr: "Pull request is not mergeable: the base branch policy prohibits the merge"
                .into(),
        },
    );
    assert!(asks(&f, cx, |ws, window, cx| {
        ws.ask_pr_merge(id, MergeMethod::Merge, window, cx)
    }));
    confirm(&f, cx);
    assert_eq!(gh.count(GhOp::MergePr), 1);
    let error = view(&f, cx)
        .error()
        .expect("GitHub's refusal stays in the detail");
    assert!(error.contains("base branch policy"), "{error}");

    // A conflict is not asked at all.
    let mut conflicting = mergeable(7, "feat");
    conflicting.mergeability = Mergeability::Conflicting;
    gh.set_pr_detail(&repo(), 7, conflicting);
    f.ws.update(cx, |ws, cx| ws.refresh_pr_detail(id, cx));
    cx.run_until_parked();
    assert!(!asks(&f, cx, |ws, window, cx| {
        ws.ask_pr_merge(id, MergeMethod::Merge, window, cx)
    }));
}

#[gpui::test]
fn an_answer_landing_after_its_detail_was_replaced_is_dropped(cx: &mut TestAppContext) {
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    gh.set_pr_detail(&repo(), 8, mergeable(8, "other"));
    f.ws.update(cx, |ws, cx| ws.rerun_pr_detail_checks(id, cx));
    let reads = gh.count(GhOp::PrDetail);
    open(&f, item(8, "other"), cx);
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::RerunFailed), 2, "the rerun still ran");
    assert_eq!(
        gh.count(GhOp::PrDetail),
        reads + 1,
        "only #8's own read: #7's answer reloads nothing"
    );
    let Some((shown_id, 8, Load::Ready(_))) = shown(&f, cx) else {
        panic!("{:?}", shown(&f, cx));
    };
    assert_ne!(shown_id, id);
    let actions = view(&f, cx).header.actions().cloned().expect("#8 is open");
    assert_eq!(actions.merge.slot, ActionSlot::Ready, "#8 runs nothing");
}

/// A description is any author's text: its links reach the system opener
/// only as a web page or an e-mail address, and anything else is swallowed
/// rather than handed to the Markdown view's default opener.
#[gpui::test]
fn a_description_link_opens_only_a_web_page_or_an_address(cx: &mut TestAppContext) {
    use crate::workspace::pages::pull_requests::detail::open_description_link;
    let (f, _, _) = window(cx);
    let open = |url: &str, cx: &mut TestAppContext| {
        cx.update_window(f.window.into(), |_, window, cx| {
            open_description_link(url, window, cx)
        })
        .unwrap()
    };
    // The platform keeps only the last URL opened, so the swallowed links
    // go first, while nothing has been opened.
    for url in [
        "file:///etc/passwd",
        "vscode://file/etc/passwd",
        "javascript:alert(1)",
    ] {
        assert!(open(url, cx), "never left to the default opener: {url}");
        assert_eq!(cx.opened_url(), None, "{url}");
    }
    for url in ["https://example.com/a", "mailto:a@example.com"] {
        assert!(open(url, cx), "{url}");
        assert_eq!(cx.opened_url().as_deref(), Some(url));
    }
}

/// `gh` can fail after GitHub acted: a merge that lands while the request
/// times out. The detail is read again, and keeps what failed under its
/// header after that read lands.
#[gpui::test]
fn a_failed_merge_reloads_the_detail_and_keeps_the_failure(cx: &mut TestAppContext) {
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    let mut merged = mergeable(7, "feat");
    merged.summary.state = PrState::Merged;
    gh.set_pr_detail(&repo(), 7, merged);
    gh.fail(
        GhOp::MergePr,
        GhFailure::Exit {
            code: None,
            stderr: "timed out".into(),
        },
    );
    let reads = gh.count(GhOp::PrDetail);

    assert!(asks(&f, cx, |ws, window, cx| {
        ws.ask_pr_merge(id, MergeMethod::Merge, window, cx)
    }));
    confirm(&f, cx);
    assert_eq!(gh.count(GhOp::MergePr), 1);
    assert_eq!(
        gh.count(GhOp::PrDetail),
        reads + 1,
        "the detail is reloaded"
    );
    assert_eq!(loaded_state(&f, cx), Some(PrState::Merged));
    assert_eq!(
        error_titles(&f, cx),
        [crate::surface::strings::error::github_pr_merge_failed(7)]
    );
    assert!(
        view(&f, cx).error().is_some(),
        "the failure outlives the reload"
    );
}

/// A log closed (or cleared by a refresh) and opened again while its first
/// read is out waits for its own read: the first one, answering late, is
/// dropped rather than filling the new slot.
#[gpui::test]
fn a_reopened_log_does_not_take_an_earlier_reads_answer(cx: &mut TestAppContext) {
    let (f, _, _) = window(cx);
    let id = read_detail(&f, cx);
    let log = |f: &GitFixture, cx: &mut TestAppContext| {
        let DetailBody::Ready(ready) = view(f, cx).body else {
            panic!("not read");
        };
        ready.checks[0].log.clone()
    };
    for reopen in ["closed and opened", "refreshed and opened"] {
        f.ws.update(cx, |ws, cx| {
            ws.toggle_pr_check_log(id, JOB, cx);
            let detail = ws.pages.pull_requests.detail.as_ref().unwrap();
            let first = detail.log_ticket(JOB).unwrap();
            match reopen {
                "closed and opened" => ws.toggle_pr_check_log(id, JOB, cx),
                _ => ws.refresh_pr_detail(id, cx),
            }
            ws.toggle_pr_check_log(id, JOB, cx);
            // The first read's answer, from before the reopen.
            ws.land_pr_check_log(id, JOB, first, Ok("stale".into()), cx);
        });
        assert_eq!(log(&f, cx), LogSlot::Open(JOB, Load::Loading), "{reopen}");
        cx.run_until_parked();
        let LogSlot::Open(_, Load::Ready(text)) = log(&f, cx) else {
            panic!("{reopen}: {:?}", log(&f, cx));
        };
        assert!(text.contains("test parse ... FAILED"), "{reopen}: {text}");
        // Closed for the next round.
        f.ws.update(cx, |ws, cx| ws.toggle_pr_check_log(id, JOB, cx));
    }
}

pub(in crate::workspace) fn set_github_enabled(cx: &mut TestAppContext, enabled: bool) {
    cx.update(|cx| {
        let mut config = daruda_config::Config::default();
        config.github.enabled = enabled;
        cx.global_mut::<crate::settings_store::SettingsStore>()
            .set_user_for_testing(config);
    });
}

/// With `[github] enabled` off the page shows Off; an open detail closes
/// with the list rather than go on reading and acting through `gh`, and
/// none opens until the feature is back.
#[gpui::test]
fn turning_the_feature_off_closes_the_detail(cx: &mut TestAppContext) {
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    set_github_enabled(cx, false);
    cx.update(crate::github::PrPoller::tick);
    cx.run_until_parked();
    assert_eq!(shown(&f, cx).map(|(_, number, _)| number), None);
    let page = f.ws.update(cx, |ws, cx| {
        ws.stage_pr_page(cx);
        let page = &ws.pages.pull_requests;
        (
            page.detail_snapshot.is_some(),
            page.snapshot.as_ref().map(|shot| shot.list.body.clone()),
        )
    });
    assert_eq!(
        page,
        (
            false,
            Some(crate::workspace::pages::pull_requests::list::PageBody::Off)
        )
    );

    // The closed detail's buttons and a click on a row reach nothing.
    let (reads, before) = (gh.count(GhOp::PrDetail), gh.calls().len());
    f.ws.update(cx, |ws, cx| {
        ws.refresh_pr_detail(id, cx);
        ws.rerun_pr_detail_checks(id, cx);
        ws.toggle_pr_check_log(id, JOB, cx);
    });
    open(&f, item(7, "feat"), cx);
    cx.run_until_parked();
    assert_eq!(shown(&f, cx).map(|(_, number, _)| number), None);
    assert_eq!(gh.count(GhOp::PrDetail), reads);
    assert_eq!(gh.calls().len(), before, "nothing reaches gh while off");

    set_github_enabled(cx, true);
    cx.update(crate::github::PrPoller::tick);
    cx.run_until_parked();
    open(&f, item(7, "feat"), cx);
    cx.run_until_parked();
    assert!(matches!(shown(&f, cx), Some((_, 7, Load::Ready(_)))));
}

#[gpui::test]
fn returning_to_a_pull_request_keeps_its_tab_and_scroll(cx: &mut TestAppContext) {
    let (f, _gh, _) = window(cx);
    open(&f, item(7, "feat"), cx);
    f.ws.update(cx, |ws, cx| {
        let id = ws.pr_detail_id().unwrap();
        ws.select_pr_detail_tab(id, PrTab::Threads, cx);
        let offset = gpui::point(gpui::px(0.), gpui::px(-240.));
        let detail = ws.pages.pull_requests.detail.as_ref().unwrap();
        detail.scroll().set_offset(offset);
        ws.close_page(cx);
        ws.show_page(Page::Tasks, cx);
        ws.show_page(Page::PullRequests, cx);
        let detail = ws.pages.pull_requests.detail.as_ref().unwrap();
        assert_eq!(detail.id(), id);
        assert_eq!(detail.tab(), PrTab::Threads);
        assert_eq!(detail.scroll().offset(), offset);
    });
}

/// The merge facts beside the detail, by label.
fn merge_facts(f: &GitFixture, cx: &mut TestAppContext) -> Vec<(String, String)> {
    let DetailBody::Ready(ready) = view(f, cx).body else {
        panic!("not read");
    };
    ready
        .side
        .merge
        .iter()
        .map(|fact| (fact.label.clone(), fact.value.clone()))
        .collect()
}

/// A pull request waiting on its requirements, which the viewer may set
/// to merge on its own.
fn waiting(number: u64) -> PrDetail {
    let mut detail = mergeable(number, "feat");
    detail.mergeability = Mergeability::Blocked(daruda_github::BlockReason::Requirements);
    detail.viewer_can = detail
        .viewer_can
        .with(daruda_github::Permission::EnableAutoMerge, true);
    detail.viewer_can = detail
        .viewer_can
        .with(daruda_github::Permission::DisableAutoMerge, true);
    detail
}

#[gpui::test]
fn auto_merge_is_turned_on_once_asked_and_off_again(cx: &mut TestAppContext) {
    use crate::surface::strings::pull_requests as s;
    let (f, gh, id) = opened(cx, waiting(7));
    let actions = view(&f, cx).header.actions().cloned().expect("open");
    let methods: Vec<_> = actions
        .merge
        .auto
        .iter()
        .map(|(method, _)| *method)
        .collect();
    assert_eq!(
        methods,
        [MergeMethod::Squash, MergeMethod::Merge],
        "the merge menu's order"
    );
    assert_eq!(actions.auto_merge_off, None, "off: nothing to turn off");

    assert!(asks(&f, cx, |ws, window, cx| {
        ws.ask_pr_auto_merge(id, MergeMethod::Squash, None, window, cx)
    }));
    // Polling meanwhile never turns it on.
    cx.update(crate::github::PrPoller::tick);
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::EnableAutoMerge), 0, "nothing before OK");
    confirm(&f, cx);
    assert!(gh.calls().contains(&GhCall::EnableAutoMerge {
        repo: repo(),
        pr: "PR_7".into(),
        method: MergeMethod::Squash,
        head_sha: "abc".into(),
        include_stack: false,
    }));
    assert_eq!(loaded_state(&f, cx), Some(PrState::Open), "not merged");
    let facts = merge_facts(&f, cx);
    assert!(
        facts.contains(&(s::side_auto_merge(), s::auto_merge_on(s::method_squash()))),
        "{facts:?}"
    );
    let actions = view(&f, cx).header.actions().cloned().expect("still open");
    assert!(actions.merge.auto.is_empty(), "already on");
    assert_eq!(actions.auto_merge_off, Some(ActionSlot::Ready));

    f.ws.update(cx, |ws, cx| ws.disable_pr_auto_merge(id, cx));
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::DisableAutoMerge), 1);
    assert!(
        !merge_facts(&f, cx)
            .iter()
            .any(|(label, _)| *label == s::side_auto_merge())
    );
    assert!(error_titles(&f, cx).is_empty());
}

#[gpui::test]
fn auto_merge_on_a_moved_head_is_refused(cx: &mut TestAppContext) {
    let (f, gh, id) = opened(cx, waiting(7));
    assert!(asks(&f, cx, |ws, window, cx| {
        ws.ask_pr_auto_merge(id, MergeMethod::Squash, None, window, cx)
    }));
    // The branch moves before OK.
    let mut moved = waiting(7);
    moved.head_sha = "def".into();
    gh.set_pr_detail(&repo(), 7, moved);
    confirm(&f, cx);
    assert_eq!(gh.count(GhOp::EnableAutoMerge), 1);
    // GitHub's wording for this refusal is not known: told as a failure.
    assert_eq!(
        error_titles(&f, cx),
        [crate::surface::strings::error::github_pr_auto_merge_failed(
            7
        )]
    );
    let DetailBody::Ready(_) = view(&f, cx).body else {
        panic!("read again");
    };
    assert!(
        view(&f, cx)
            .header
            .actions()
            .is_some_and(|actions| actions.auto_merge_off.is_none()),
        "still off"
    );
}

/// In the merge queue: accepted, not merged; the merge button says so,
/// and it can be taken out of the queue.
#[gpui::test]
fn a_queued_pull_request_is_shown_queued_not_merged(cx: &mut TestAppContext) {
    use crate::surface::strings::pull_requests as s;
    let mut queued = waiting(7);
    queued.queue = Some(daruda_github::QueueEntry {
        position: Some(2),
        state: daruda_github::QueueState::AwaitingChecks,
    });
    let (f, gh, id) = opened(cx, queued);
    assert_eq!(view(&f, cx).state, s::state_open());
    let actions = view(&f, cx).header.actions().cloned().expect("open");
    assert_eq!(
        actions.merge.slot,
        ActionSlot::Blocked(s::merge_blocked(s::merge_in_queue()))
    );
    assert!(actions.merge.auto.is_empty());
    let facts = merge_facts(&f, cx);
    assert!(
        facts.contains(&(
            s::side_merge_queue(),
            s::queue_position(2, s::queue_awaiting_checks())
        )),
        "{facts:?}"
    );
    // Queued without auto-merge: nothing to turn off, but it can leave.
    assert_eq!(actions.auto_merge_off, None);
    assert_eq!(actions.dequeue, Some(ActionSlot::Ready));
    assert!(asks(&f, cx, |ws, window, cx| ws.ask_pr_dequeue(id, window, cx)));
    assert_eq!(gh.count(GhOp::Dequeue), 0, "nothing before OK");
    confirm(&f, cx);
    assert!(gh.calls().contains(&GhCall::Dequeue {
        repo: repo(),
        pr: "PR_7".into(),
    }));
    let actions = view(&f, cx).header.actions().cloned().expect("still open");
    assert_eq!(actions.dequeue, None, "out of the queue");
    assert!(
        !matches!(actions.merge.slot, ActionSlot::Blocked(_)),
        "it can be merged again: {:?}",
        actions.merge.slot
    );
    assert!(error_titles(&f, cx).is_empty());
}

/// One failed check's own Rerun asks for its run alone, says it was asked
/// (nothing else shows it until GitHub starts), and links to GitHub.
#[gpui::test]
fn a_failed_check_reruns_its_own_run_and_links_to_github(cx: &mut TestAppContext) {
    use crate::workspace::pages::pull_requests::detail::view::ActionNotice;
    let mut detail = mergeable(7, "feat");
    detail.checks[2].run.details_url = Some("https://github.com/o/r/actions/runs/12/job/30".into());
    detail.checks[1].run.details_url = Some("javascript:alert(1)".into());
    let (f, gh, id) = opened(cx, detail);
    let DetailBody::Ready(ready) = view(&f, cx).body else {
        panic!("not read");
    };
    let runs: Vec<_> = ready
        .checks
        .iter()
        .map(|row| row.rerun.as_ref().map(|(run, _)| *run))
        .collect();
    assert_eq!(runs, [Some(11), Some(11), Some(12)]);
    assert_eq!(
        ready.checks[2].url.as_deref(),
        Some("https://github.com/o/r/actions/runs/12/job/30")
    );
    assert_eq!(ready.checks[1].url, None, "only a web page is opened");

    f.ws.update(cx, |ws, cx| ws.rerun_pr_check_run(id, 12, cx));
    cx.run_until_parked();
    let reruns: Vec<u64> = gh
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            GhCall::RerunFailed { run, .. } => Some(run),
            _ => None,
        })
        .collect();
    assert_eq!(reruns, [12], "that run alone");
    let requested = Some(ActionNotice::Done(
        crate::surface::strings::pull_requests::rerun_requested(),
    ));
    assert_eq!(view(&f, cx).notice, requested);
    // Polling meanwhile reads the lists, not the detail: the line stays.
    cx.update(crate::github::PrPoller::tick);
    cx.run_until_parked();
    assert_eq!(view(&f, cx).notice, requested, "kept across a poll");

    // A run with no failed job is never asked to rerun.
    f.ws.update(cx, |ws, cx| ws.rerun_pr_check_run(id, 99, cx));
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::RerunFailed), 1);

    // A reload clears the line.
    f.ws.update(cx, |ws, cx| ws.refresh_pr_detail(id, cx));
    cx.run_until_parked();
    assert_eq!(view(&f, cx).notice, None);
}

/// In a stack with an open pull request below it: auto-merge can take the
/// stack below too, the range named when asked and checked again when sent.
#[gpui::test]
fn auto_merge_with_the_stack_below_names_and_rechecks_its_range(cx: &mut TestAppContext) {
    use daruda_github::{PrStack, StackEntry, StackRead};
    let stacked = |numbers: &[u64]| {
        StackRead::Stacked(PrStack {
            number: 1,
            base: "main".into(),
            size: numbers.len() as u32,
            entries: numbers
                .iter()
                .enumerate()
                .map(|(ix, number)| StackEntry {
                    position: ix as u32 + 1,
                    number: *number,
                    title: String::new(),
                    state: PrState::Open,
                })
                .collect(),
        })
    };
    let (f, gh, _) = window(cx);
    gh.set_pr_stack(&repo(), 7, stacked(&[6, 7]));
    gh.set_pr_detail(&repo(), 7, waiting(7));
    open(&f, item(7, "feat"), cx);
    cx.run_until_parked();
    let id = id(&f, cx);
    let actions = view(&f, cx).header.actions().cloned().expect("open");
    let (range, methods) = actions.merge.auto_stack.expect("offered with the stack");
    assert_eq!(range, [6, 7]);
    assert!(!methods.is_empty());

    assert!(asks(&f, cx, |ws, window, cx| {
        ws.ask_pr_auto_merge(id, MergeMethod::Squash, Some(vec![6, 7]), window, cx)
    }));
    confirm(&f, cx);
    assert!(gh.calls().contains(&GhCall::EnableAutoMerge {
        repo: repo(),
        pr: "PR_7".into(),
        method: MergeMethod::Squash,
        head_sha: "abc".into(),
        include_stack: true,
    }));
    assert_eq!(
        loaded_state(&f, cx),
        Some(PrState::Open),
        "accepted, not merged"
    );

    // The stack changed between asking and sending: nothing is turned on.
    gh.set_pr_detail(&repo(), 7, waiting(7));
    f.ws.update(cx, |ws, cx| ws.refresh_pr_detail(id, cx));
    cx.run_until_parked();
    assert!(asks(&f, cx, |ws, window, cx| {
        ws.ask_pr_auto_merge(id, MergeMethod::Squash, Some(vec![6, 7]), window, cx)
    }));
    gh.set_pr_stack(&repo(), 7, stacked(&[5, 6, 7]));
    let before = gh.count(GhOp::EnableAutoMerge);
    confirm(&f, cx);
    assert_eq!(gh.count(GhOp::EnableAutoMerge), before, "not sent");
    let changed = crate::surface::strings::error::github_pr_stack_changed(7);
    assert_eq!(error_titles(&f, cx).last(), Some(&changed));
    let severity = f.ws.read_with(cx, |ws, _| {
        ws.error_history()
            .iter()
            .find(|report| report.title == changed)
            .map(|report| report.severity)
    });
    assert_eq!(
        severity,
        Some(daruda_store::observability::error_report::ErrorSeverity::Warning),
        "an out-of-date view, not a failure"
    );
    assert_eq!(
        view(&f, cx).error(),
        Some(changed),
        "said once, without gh's words after"
    );
}

/// A base behind a merge queue: `gh` answers the merge, nothing is merged
/// yet, and the detail read again shows it queued, not merged.
#[gpui::test]
fn a_merge_into_a_queue_shows_it_queued(cx: &mut TestAppContext) {
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    gh.set_merge_queued(&repo(), 7);
    assert!(asks(&f, cx, |ws, window, cx| {
        ws.ask_pr_merge(id, MergeMethod::Squash, window, cx)
    }));
    confirm(&f, cx);
    assert_eq!(gh.count(GhOp::MergePr), 1);
    let Some((_, _, Load::Ready(loaded))) = shown(&f, cx) else {
        panic!("not read");
    };
    assert_eq!(loaded.summary.state, PrState::Open, "not merged");
    assert!(loaded.queue.is_some(), "queued");
    assert!(error_titles(&f, cx).is_empty(), "not a failure");
}
