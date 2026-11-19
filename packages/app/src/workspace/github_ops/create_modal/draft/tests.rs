use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{Entity, TestAppContext, WeakEntity, WindowHandle};

use super::super::CreateState;
use super::*;
use crate::agent::text_run::FakeTextRunner;
use crate::lane::git::base::{AgainstBase, BaseTips, RangeFile, ResolvedBase};
use crate::lane::git::{FakeGit, FakeRepo, Git as _, PushTarget};

type Form = (WindowHandle<CreatePrModal>, Entity<CreatePrModal>);

fn text_run() -> TextRun {
    TextRun {
        cli: crate::agent::adapter_cli::AdapterCliRequest {
            adapter: "adapter".into(),
            agent_env: Vec::new(),
            words: vec!["--cli".into()],
        },
        account: daruda_config::AccountEnv::ambient(),
    }
}

/// A branch `feat` with `commits` over `origin/main`, one file changed.
fn branch(commits: u32) -> FakeRepo {
    let tips = BaseTips {
        base: ResolvedBase {
            label: "origin/main".into(),
            sha: "b".into(),
        },
        head: "h".into(),
    };
    FakeRepo::at("/fake/repo")
        .head_message("Fix the sidebar")
        .push_target(PushTarget {
            remote: "origin".into(),
            url: "git@github.com:o/r.git".into(),
            branch: "feat".into(),
            published: true,
            has_upstream: true,
            review: None,
        })
        .base_tips(Ok(tips.clone()))
        .changes_since(Ok(AgainstBase {
            tips,
            merge_base: "m".into(),
            files: vec![RangeFile {
                path: PathBuf::from("src/a.rs"),
                old_path: None,
                status: 'M',
                added: 2,
                removed: 1,
            }],
            commits,
        }))
        .range_subjects(&["Fix the sidebar", "Add the panel"])
        .diff_range("diff --git a/src/a.rs b/src/a.rs\n+new line\n")
}

fn open(
    cx: &mut TestAppContext,
    repo: FakeRepo,
    base: Option<&str>,
    run: Option<TextRun>,
) -> (Form, FakeTextRunner) {
    crate::test_support::init_gpui_component(cx);
    let runner = FakeTextRunner::default();
    cx.update(|cx| crate::text_run_host::TextRunHost::set_global(Arc::new(runner.clone()), cx));
    let handle = FakeGit::new().with(repo).open(Path::new("/fake/repo"));
    let base = base.map(str::to_owned);
    let window = cx.add_window(|window, cx| {
        let lane = daruda_store::project::LaneRef {
            project: 0,
            lane: 0,
        };
        CreatePrModal::new(WeakEntity::new_invalid(), lane, handle, base, window, cx)
            .written_by(run)
    });
    let modal = window.root(cx).unwrap();
    cx.run_until_parked();
    ((window, modal), runner)
}

fn press_write(form: &Form, cx: &mut TestAppContext) {
    form.0
        .update(cx, |modal, window, cx| modal.write_draft(window, cx))
        .unwrap();
}

fn fields(form: &Form, cx: &mut TestAppContext) -> (String, String) {
    form.1.read_with(cx, |modal, cx| modal.fields_for_test(cx))
}

fn failure(form: &Form, cx: &mut TestAppContext) -> Option<String> {
    form.1.read_with(cx, |modal, _| match &modal.written {
        DraftState::Failed(reason) => Some(reason.to_string()),
        _ => None,
    })
}

/// Pressing Write fills the title and the description from the agent's
/// answer, asked with what the branch takes; Put back restores what the
/// user had typed.
#[gpui::test]
fn the_agents_answer_fills_the_fields_and_can_be_put_back(cx: &mut TestAppContext) {
    let (form, runner) = open(cx, branch(2), Some("main"), Some(text_run()));
    form.0
        .update(cx, |modal, window, cx| {
            modal.type_for_test("Mine", "my words", window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    runner.answer(Ok(
        r#"{"title":"Fix the sidebar width","body":"Why\n\n- a"}"#,
    ));

    press_write(&form, cx);
    assert!(
        form.1.read_with(cx, |modal, _| matches!(
            modal.written,
            DraftState::Running { .. }
        )),
        "writing until the agent answers"
    );
    assert_eq!(fields(&form, cx), ("Mine".into(), "my words".into()));
    cx.run_until_parked();
    assert_eq!(
        fields(&form, cx),
        ("Fix the sidebar width".into(), "Why\n\n- a".into())
    );

    let asked = runner.asked();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].0, text_run());
    let prompt = &asked[0].1;
    assert!(prompt.contains("`feat` into `origin/main`"), "{prompt}");
    assert!(
        prompt.contains("- Fix the sidebar\n- Add the panel\n"),
        "{prompt}"
    );
    assert!(prompt.contains("- src/a.rs (+2 -1)"), "{prompt}");
    assert!(prompt.contains("+new line"), "{prompt}");

    form.0
        .update(cx, |modal, window, cx| modal.undo_draft(window, cx))
        .unwrap();
    assert_eq!(fields(&form, cx), ("Mine".into(), "my words".into()));
    assert!(
        form.1
            .read_with(cx, |modal, _| matches!(modal.written, DraftState::Idle))
    );
}

/// What the agent wrote is the user's from then on: typing over it takes
/// the way back away, since putting the old text back would lose it.
#[gpui::test]
fn typing_after_a_draft_keeps_what_was_typed(cx: &mut TestAppContext) {
    let (form, runner) = open(cx, branch(1), Some("main"), Some(text_run()));
    runner.answer(Ok(r#"{"title":"T","body":"B"}"#));
    press_write(&form, cx);
    cx.run_until_parked();
    assert!(form.1.read_with(cx, |modal, _| matches!(
        modal.written,
        DraftState::Filled(_)
    )));
    form.0
        .update(cx, |modal, window, cx| {
            modal.type_for_test("T, edited", "B", window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    form.0
        .update(cx, |modal, window, cx| modal.undo_draft(window, cx))
        .unwrap();
    assert_eq!(fields(&form, cx).0, "T, edited");
}

/// Every failure is a line in the form; the fields stay as they were and
/// the button can be pressed again.
#[gpui::test]
fn a_failure_is_said_in_the_form_and_changes_nothing(cx: &mut TestAppContext) {
    let (form, runner) = open(cx, branch(1), Some("main"), Some(text_run()));
    let before = fields(&form, cx);
    let cases: [(&str, Result<&str, &str>, String); 2] = [
        (
            "the agent failed",
            Err("Not logged in"),
            s::pr_write_failed("Not logged in"),
        ),
        (
            "an answer that is not a title and a description",
            Ok("I cannot do that."),
            s::pr_write_unreadable(),
        ),
    ];
    for (label, answer, said) in cases {
        runner.answer(answer);
        press_write(&form, cx);
        cx.run_until_parked();
        assert_eq!(failure(&form, cx), Some(said), "{label}");
        assert_eq!(fields(&form, cx), before, "{label}");
        assert_eq!(
            form.1.read_with(cx, |modal, cx| modal.draft_offer(cx)),
            DraftOffer::Ready,
            "{label}"
        );
    }
}

/// Stop ends the run: its flag is raised for the process, and an answer
/// that still arrives fills nothing.
#[gpui::test]
fn a_stopped_run_fills_nothing(cx: &mut TestAppContext) {
    let (form, runner) = open(cx, branch(1), Some("main"), Some(text_run()));
    let before = fields(&form, cx);
    runner.answer(Ok(r#"{"title":"T","body":"B"}"#));
    press_write(&form, cx);
    let cancelled = form.1.read_with(cx, |modal, _| match &modal.written {
        DraftState::Running { _run } => _run.cancelled.clone(),
        _ => panic!("running"),
    });
    form.0
        .update(cx, |modal, _, cx| modal.stop_draft(cx))
        .unwrap();
    assert!(
        cancelled.load(Ordering::Relaxed),
        "the process is told to end"
    );
    cx.run_until_parked();
    assert_eq!(fields(&form, cx), before);
    assert!(runner.asked().is_empty(), "the dropped task never ran");
}

/// Closing the form ends a run in flight with it.
#[gpui::test]
fn closing_the_form_ends_the_run(cx: &mut TestAppContext) {
    let (form, _runner) = open(cx, branch(1), Some("main"), Some(text_run()));
    press_write(&form, cx);
    let cancelled = form.1.read_with(cx, |modal, _| match &modal.written {
        DraftState::Running { _run } => _run.cancelled.clone(),
        _ => panic!("running"),
    });
    let (window, modal) = form;
    drop(modal);
    window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    assert!(cancelled.load(Ordering::Relaxed));
}

/// The button is offered only when there is an agent to ask and something
/// to describe; otherwise pressing it asks nobody.
#[gpui::test]
fn the_button_is_offered_only_with_an_agent_and_commits_to_describe(cx: &mut TestAppContext) {
    type Case = (
        &'static str,
        FakeRepo,
        Option<&'static str>,
        Option<TextRun>,
        DraftOffer,
    );
    let cases: [Case; 5] = [
        (
            "an agent and commits over a known base",
            branch(2),
            Some("main"),
            Some(text_run()),
            DraftOffer::Ready,
        ),
        (
            "no agent with a text-only run",
            branch(2),
            Some("main"),
            None,
            DraftOffer::NoAgent,
        ),
        (
            "no commit over the base",
            branch(0),
            Some("main"),
            Some(text_run()),
            DraftOffer::NothingOver,
        ),
        // GitHub's default branch is not known here: nothing to compare.
        (
            "no base typed",
            branch(2),
            None,
            Some(text_run()),
            DraftOffer::NoBase,
        ),
        (
            "a base git does not know",
            FakeRepo::at("/fake/repo"),
            Some("mian"),
            Some(text_run()),
            DraftOffer::NoBase,
        ),
    ];
    for (label, repo, base, run, expected) in cases {
        let (form, runner) = open(cx, repo, base, run);
        assert_eq!(
            form.1.read_with(cx, |modal, cx| modal.draft_offer(cx)),
            expected,
            "{label}"
        );
        if expected != DraftOffer::Ready {
            press_write(&form, cx);
            cx.run_until_parked();
            assert!(runner.asked().is_empty(), "{label}");
        }
    }
}

/// The template the form uses is what the agent is asked to fill in.
#[gpui::test]
fn the_forms_template_is_part_of_what_is_asked(cx: &mut TestAppContext) {
    let (form, runner) = open(cx, branch(1), Some("main"), Some(text_run()));
    form.0
        .update(cx, |modal, _, _| {
            modal.templates.default = Some(super::super::templates::PrTemplate {
                path: ".github/pull_request_template.md".into(),
                body: "## Summary\n".into(),
            });
        })
        .unwrap();
    runner.answer(Ok(r#"{"title":"T","body":"B"}"#));
    press_write(&form, cx);
    cx.run_until_parked();
    assert!(
        runner.asked()[0].1.contains("## Template\n```\n## Summary"),
        "{}",
        runner.asked()[0].1
    );
}

/// An answer never lands on what the user typed meanwhile, nor describes a
/// base they have since changed: either ends the run.
#[gpui::test]
fn typing_or_another_base_ends_a_run_in_flight(cx: &mut TestAppContext) {
    type Edit = fn(&mut CreatePrModal, &mut gpui::Window, &mut gpui::Context<CreatePrModal>);
    let cases: [(&str, Edit); 2] = [
        ("typing in the description", |modal, window, cx| {
            modal.type_for_test("Mine", "typed meanwhile", window, cx)
        }),
        ("naming another base", |modal, window, cx| {
            modal.choose_base("develop".into(), window, cx)
        }),
    ];
    for (label, edit) in cases {
        let (form, runner) = open(cx, branch(1), Some("main"), Some(text_run()));
        runner.answer(Ok(r#"{"title":"T","body":"B"}"#));
        press_write(&form, cx);
        form.0.update(cx, edit).unwrap();
        let typed = fields(&form, cx);
        cx.run_until_parked();
        assert_eq!(fields(&form, cx), typed, "{label}");
        assert!(
            form.1
                .read_with(cx, |modal, _| matches!(modal.written, DraftState::Idle)),
            "{label}"
        );
    }
}

/// Git failing is said with git's own words, not as nothing to describe.
#[gpui::test]
fn a_git_failure_is_said_as_git_said_it(cx: &mut TestAppContext) {
    let (form, runner) = open(cx, branch(1), Some("main"), Some(text_run()));
    form.0
        .update(cx, |modal, _, _| {
            // The patch read fails; the counts were read before.
            let git = FakeGit::new().with(branch(1));
            git.fail(
                "/fake/repo",
                crate::lane::git::GitOp::DiffRange,
                crate::lane::git::FakeFailure::Exit {
                    code: Some(128),
                    stderr: "fatal: bad object".into(),
                },
            );
            modal.repo = git.open(Path::new("/fake/repo"));
        })
        .unwrap();
    press_write(&form, cx);
    cx.run_until_parked();
    let said = failure(&form, cx).expect("a failure line");
    assert!(said.contains("bad object"), "{said}");
    assert_ne!(said, s::pr_write_no_changes());
    assert!(runner.asked().is_empty(), "the agent is not asked");
}

fn run_flag(form: &Form, cx: &mut TestAppContext) -> Arc<AtomicBool> {
    form.1.read_with(cx, |modal, _| match &modal.written {
        DraftState::Running { _run } => _run.cancelled.clone(),
        _ => panic!("running"),
    })
}

/// Sending the form ends a run in flight: what is created is what the
/// fields held when it was sent, not an answer arriving after.
#[gpui::test]
fn sending_the_form_ends_a_run_in_flight(cx: &mut TestAppContext) {
    let (form, runner) = open(cx, branch(1), Some("main"), Some(text_run()));
    let before = fields(&form, cx);
    runner.answer(Ok(r#"{"title":"T","body":"B"}"#));
    press_write(&form, cx);
    let cancelled = run_flag(&form, cx);
    form.0
        .update(cx, |modal, _, cx| modal.begin_creating(cx))
        .unwrap();
    assert!(cancelled.load(Ordering::Relaxed), "the process is ended");
    cx.run_until_parked();
    assert_eq!(fields(&form, cx), before);
    assert!(form.1.read_with(cx, |modal, _| {
        modal.state == CreateState::Creating && matches!(modal.written, DraftState::Idle)
    }));
}

/// Write pressed again right after Stop: the stopped run is ended and
/// fills nothing; only the new one's answer lands.
#[gpui::test]
fn writing_again_after_a_stop_takes_only_the_new_answer(cx: &mut TestAppContext) {
    let (form, runner) = open(cx, branch(1), Some("main"), Some(text_run()));
    runner.answer(Ok(r#"{"title":"Second","body":"B"}"#));
    press_write(&form, cx);
    let first = run_flag(&form, cx);
    form.0
        .update(cx, |modal, _, cx| modal.stop_draft(cx))
        .unwrap();
    press_write(&form, cx);
    cx.run_until_parked();
    assert!(first.load(Ordering::Relaxed), "the stopped run is ended");
    assert_eq!(fields(&form, cx), ("Second".to_owned(), "B".to_owned()));
    assert_eq!(runner.asked().len(), 1, "the stopped run asked nobody");
}
