use super::*;
use crate::runs::seed::{self, SeedEnd, SeedRun};
use gpui::TestAppContext;

#[gpui::test]
fn malformed_missing_alias_and_valid_siblings_are_independent(cx: &mut TestAppContext) {
    let query = cx.update(|cx| {
        RunsHost::ensure_installed(cx);
        let run = SeedRun::chat("/work");
        let task = seed::task(
            seed::spec(Default::default(), "valid"),
            Some((&run, SeedEnd::Done)),
            cx,
        );
        prepare(
            Some(vec![
                "flow:../bad".into(),
                "task:missing".into(),
                format!("task:{task}"),
            ]),
            cx,
        )
    });
    let result = query.read();
    let content = &result["structuredContent"];
    assert_eq!(
        content["errors"],
        serde_json::json!([{"code":"unknown_handle","handle":"flow:../bad"}])
    );
    assert_eq!(content["runs"][0]["state"]["state"], "gone");
    assert_eq!(content["runs"][1]["state"]["ended_as"], "completed");
    assert!(content["runs"][1]["handle"].as_str().unwrap().contains('/'));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(result["content"][0]["text"].as_str().unwrap())
            .unwrap(),
        *content
    );
    assert_eq!(result["isError"], false);
}

#[gpui::test]
fn status_scope_is_only_registered_starts_and_queries_have_no_reporting_effect(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        RunsHost::ensure_installed(cx);
        let run = SeedRun::chat("/work");
        let task = seed::task(
            seed::spec(Default::default(), "task"),
            Some((&run, SeedEnd::Done)),
            cx,
        );
        assert_eq!(
            prepare(None, cx).read()["structuredContent"]["runs"],
            serde_json::json!([])
        );
        RunsHost::register_run(domain::RunKey::Task(run.exec(&task)), cx);
        let first = prepare(None, cx).read();
        assert_eq!(first, prepare(None, cx).read());
        assert_eq!(
            first["structuredContent"]["runs"].as_array().unwrap().len(),
            1
        );
    });
}

/// A flow that stopped without deciding to can be picked up again; the
/// orchestrator reads that from the result rather than from the end's name.
#[test]
fn only_an_interrupted_or_stalled_end_is_marked_resumable() {
    let end = |ended_as| domain::RunSnapshot {
        key: domain::RunKey::Flow("run".into()),
        what: "Flow".into(),
        why: None,
        state: domain::RunState::Ended {
            ended_as,
            text: None,
            pointers: Default::default(),
        },
    };
    let resumable = |ended_as| match convert(end(ended_as)).state {
        wire::RunState::Ended { resumable, .. } => resumable,
        other => panic!("not an end: {other:?}"),
    };
    assert!(resumable(domain::EndedAs::Crashed));
    assert!(resumable(domain::EndedAs::Stalled));
    for ended_as in [
        domain::EndedAs::Completed,
        domain::EndedAs::Failed,
        domain::EndedAs::Canceled,
        domain::EndedAs::Interrupted,
        domain::EndedAs::Undelivered,
        domain::EndedAs::Error,
        domain::EndedAs::BudgetExhausted,
    ] {
        assert!(!resumable(ended_as), "{ended_as:?}");
    }
    let value = serde_json::to_value(convert(end(domain::EndedAs::Crashed))).unwrap();
    assert_eq!(value["state"]["resumable"], true);
    let value = serde_json::to_value(convert(end(domain::EndedAs::Completed))).unwrap();
    assert!(value["state"].get("resumable").is_none());
}
