//! Test-only workspace hooks for the control-surface tests.

use gpui::Context;

use crate::workspace::Workspace;
use crate::workspace::main_area::pane_tree::PaneId;
use daruda_store::project::LaneRef;

/// Scaffolding for the control-surface tests, which need scenarios the pane
/// ops can reach but `crate::test_support` cannot — everything driven below is
/// `pub(in crate::workspace)` at its source.
impl Workspace {
    /// Insert a pane the way [`Self::control_insert_chat`] does — *without*
    /// revealing it.
    ///
    /// Same reason: revealing focuses the pane, focusing starts a real ACP
    /// adapter, and its task outlives the test and then trips gpui's
    /// determinism assert in whichever test runs next. Every control test
    /// addresses panes by id, so none of them needs the focus.
    pub(crate) fn open_agent_chat_pane_for_test(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> PaneId {
        let agent_id = self.open_agent_id(None);
        let cwds = self.active_lane_cwds();
        self.insert_agent_chat_pane(
            agent_id,
            cwds,
            daruda_config::ModeChoice::FollowDefault,
            window,
            cx,
        )
        .expect("pane opened")
    }

    /// Add `count` lanes to the active project, each holding one agent-chat
    /// pane, and return their pane ids. What makes `main_area.runtimes` big
    /// enough for hash iteration order to diverge from the order a listing
    /// must report.
    pub(crate) fn open_agent_chat_panes_in_fresh_lanes_for_test(
        &mut self,
        count: u32,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> Vec<PaneId> {
        let project = self.active.project;
        let root = self
            .project_for(project)
            .map(|p| p.root.clone())
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        (0..count)
            .map(|i| {
                let lane_id = self.alloc_id();
                let mut lane = crate::lane::Lane::default_for_project(lane_id, root.clone());
                lane.tab_order = i + 1;
                let target = LaneRef {
                    project,
                    lane: lane_id,
                };
                self.project_for_mut(project)
                    .expect("active project")
                    .lanes
                    .push(lane);
                self.activate_lane(target, window, cx);
                self.open_agent_chat_pane_for_test(window, cx)
            })
            .collect()
    }

    /// Make the next allocated id at least `next`.
    pub(crate) fn skip_ids_below_for_test(&mut self, next: u64) {
        self.next_id = self.next_id.max(next);
    }

    /// Whether `pane` is an agent-chat pane in any of this workspace's lanes.
    pub(crate) fn has_agent_chat_pane_for_test(&self, pane: PaneId) -> bool {
        self.agent_chat_view(pane).is_some()
    }

    /// Give `pane` the live session `session`, as a connect would.
    pub(crate) fn set_pane_session_for_test(
        &mut self,
        pane: PaneId,
        session: &str,
        cx: &mut Context<Self>,
    ) {
        let view = self.agent_chat_view(pane).expect("pane").clone();
        view.update(cx, |v, _| v.set_session_id_for_test(Some(session.into())));
    }

    /// Drive one pane into each of the three states `/brief` counts. Each
    /// writes the field the sanctioned predicate reads — `activity_state`
    /// folds turn + permissions, and `map_health` reads `status`.
    pub(crate) fn set_pane_working_for_test(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        let view = self.agent_chat_view(pane).expect("pane").clone();
        view.update(cx, |v, _| v.set_turn_in_flight());
    }

    pub(crate) fn set_pane_awaiting_permission_for_test(
        &mut self,
        pane: PaneId,
        cx: &mut Context<Self>,
    ) {
        let view = self.agent_chat_view(pane).expect("pane").clone();
        view.update(cx, |v, _| v.hold_permission_for_shot(1));
    }

    pub(crate) fn set_pane_awaiting_answer_for_test(
        &mut self,
        pane: PaneId,
        cx: &mut Context<Self>,
    ) {
        let view = self.agent_chat_view(pane).expect("pane").clone();
        let question = daruda_acp::QuestionRequest {
            message: "Which?".into(),
            tool_call_id: None,
            delivery: daruda_acp::QuestionDelivery::Blocking,
            fields: vec![daruda_acp::QuestionField {
                key: "question_0".into(),
                title: None,
                description: None,
                required: false,
                kind: daruda_acp::QuestionFieldKind::Text,
                companion_of: None,
                default: None,
            }],
        };
        view.update(cx, |v, cx| {
            v.apply_event(
                daruda_acp::AcpEvent::QuestionAsked { id: 1, question },
                "",
                false,
                cx,
            );
        });
    }

    pub(crate) fn set_pane_errored_for_test(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        let view = self.agent_chat_view(pane).expect("pane").clone();
        view.update(cx, |v, cx| {
            v.set_error("boom".into(), daruda_acp::Remedy::NoneAvailable, cx)
        });
    }
}
