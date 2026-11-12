//! The push-target axis: where a lane's branch pushes, which names the
//! GitHub repository and branch its pull request is looked up under.

use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::observability::system_info::redact_home;
use daruda_store::project::LaneRef;
use gpui::Context;

use crate::workspace::Workspace;

impl Workspace {
    /// Re-read where `target`'s branch pushes. Rides the tracking axis: a
    /// branch switch, a new upstream and a first push all move refs. Skipped
    /// while `[github]` is off: only pull requests read it, and each read
    /// costs several git processes per lane.
    pub(in crate::workspace) fn refresh_push_target(
        &mut self,
        target: LaneRef,
        cx: &mut Context<Self>,
    ) {
        if !crate::settings_store::SettingsStore::global(cx)
            .user()
            .github
            .enabled
        {
            return;
        }
        let Some(path) = self.git_lane_path(target) else {
            return;
        };
        let path_for_report = path.clone();
        let repo = crate::git_host::GitHost::global(cx).open(&path);
        self.run_git_axis(
            super::status::ReadRequest {
                target,
                recovery_origin: None,
            },
            |git| &mut git.push_target_refresh,
            move || repo.push_target(),
            move |ws, result, cx| match result {
                Ok(found) => ws.set_push_target(target, found, cx),
                Err(e) => {
                    let report =
                        ErrorReport::new(crate::surface::strings::error::git_push_target_failed())
                            .severity(ErrorSeverity::Warning)
                            .from_error(&e)
                            .at(file!(), line!())
                            .with_context("path", redact_home(&path_for_report))
                            .dedup("git.push_target")
                            .build();
                    ws.report_error(report, cx);
                    // A failed read says nothing about where the branch
                    // pushes; the last known target and its link stay.
                    false
                }
            },
            Self::refresh_push_target,
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::lane::git::{FakeRepo, GitOp, PushTarget};
    use crate::workspace::tests::GitFixture;
    use gpui::TestAppContext;

    fn target(branch: &str) -> PushTarget {
        PushTarget {
            remote: "origin".into(),
            url: "git@github.com:o/r.git".into(),
            branch: branch.into(),
            published: true,
            has_upstream: false,
        }
    }

    #[gpui::test]
    fn a_tracking_read_also_reads_the_push_target(cx: &mut TestAppContext) {
        let f = GitFixture::open(cx, &daruda_config::Config::default(), |repo: FakeRepo| {
            repo.push_target(target("main"))
        });
        let id = f.ws.read_with(cx, |ws, _| ws.active_ref());
        f.ws.update(cx, |ws, cx| ws.refresh_git_status(id, cx));
        cx.run_until_parked();
        let read = f.ws.read_with(cx, |ws, _| {
            ws.lane_scoped
                .get(&id)
                .and_then(|s| s.git.push_target.clone())
        });
        assert_eq!(read, Some(target("main")));
        assert!(f.fake.reads(GitOp::PushTarget, &f.root) >= 1);
    }
}
