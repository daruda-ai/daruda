use gpui::{TestAppContext, VisualTestContext};

use super::PrDestination;
use crate::workspace::pages::Page;
use crate::workspace::pages::pull_requests::detail::PrTab;
use crate::workspace::tests::pull_request_detail::{repo, window};

#[gpui::test]
fn a_reference_is_placed_in_a_repository(cx: &mut TestAppContext) {
    let (f, _gh, _) = window(cx);
    let placed = |text: &str, cx: &mut TestAppContext| {
        f.ws.read_with(cx, |ws, cx| {
            ws.resolve_pr_address(text, cx)
                .map(|(repo, address)| (repo.qualified(), address.number))
        })
    };
    // The shown worktree's repository for a number.
    assert_eq!(placed("#7", cx), Ok(("github.com/o/r".into(), 7)));
    // `owner/name` takes the host of a worktree on that repository.
    assert_eq!(placed("O/R#9", cx), Ok(("github.com/o/r".into(), 9)));
    // A URL keeps its own host.
    assert_eq!(
        placed("https://ghe.corp/o/r/pull/3", cx),
        Ok(("ghe.corp/o/r".into(), 3))
    );
    assert!(placed("not a pull request", cx).is_err());
}

/// Opened by URL: read once, shown on its tab, no worktree made or tied.
#[gpui::test]
fn a_url_opens_the_pull_request_on_its_tab_without_touching_worktrees(cx: &mut TestAppContext) {
    let (f, gh, _) = window(cx);
    let lanes = f.ws.read_with(cx, |ws, _| ws.projects.lanes().count());
    let reads = gh.count(daruda_github::fake::GhOp::PrDetail);
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            let (repo, address) = ws
                .resolve_pr_address("https://github.com/o/r/pull/7/files", cx)
                .unwrap();
            ws.open_pr_by_address(repo, address, super::PrDestination::Page, window, cx);
        })
    });
    cx.run_until_parked();
    assert_eq!(
        gh.count(daruda_github::fake::GhOp::PrDetail),
        reads + 1,
        "the read that found it is the one shown"
    );
    f.ws.read_with(cx, |ws, _| {
        assert_eq!(ws.active_page(), Some(Page::PullRequests));
        let detail = ws.pages.pull_requests.detail().expect("shown");
        assert_eq!(detail.key(), (repo(), 7));
        assert_eq!(detail.tab(), PrTab::Files);
        assert_eq!(ws.projects.lanes().count(), lanes, "no worktree made");
    });
}

/// A pull request GitHub does not have: said, and nothing opens.
#[gpui::test]
fn a_pull_request_that_cannot_be_read_opens_nothing(cx: &mut TestAppContext) {
    let (f, _gh, _) = window(cx);
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            let (repo, address) = ws.resolve_pr_address("o/r#404", cx).unwrap();
            ws.open_pr_by_address(repo, address, super::PrDestination::Page, window, cx);
        })
    });
    cx.run_until_parked();
    f.ws.read_with(cx, |ws, _| {
        assert!(ws.pages.pull_requests.detail().is_none());
        assert!(
            ws.error_history()
                .iter()
                .any(|report| report.dedup_key.as_deref() == Some("github.open_pr"))
        );
    });
}

/// The same owner/name on two hosts: the shown worktree's host, never a
/// guess between them.
#[gpui::test]
fn owner_and_name_on_two_hosts_are_not_guessed_between(cx: &mut TestAppContext) {
    let (f, _gh, lane) = window(cx);
    let other = daruda_store::project::LaneRef {
        project: lane.project,
        lane: 88,
    };
    f.ws.update(cx, |ws, _| {
        let state = ws.lane_scoped_mut(other);
        state.git.push_target = Some(crate::lane::git::PushTarget {
            remote: "origin".into(),
            url: "git@ghe.corp:o/r.git".into(),
            branch: "feat".into(),
            published: true,
            has_upstream: true,
            review: None,
        });
    });
    let placed = |cx: &mut TestAppContext| {
        f.ws.read_with(cx, |ws, cx| {
            ws.resolve_pr_address("o/r#9", cx)
                .map(|(repo, _)| repo.qualified())
        })
    };
    assert_eq!(
        placed(cx),
        Ok("github.com/o/r".into()),
        "the shown worktree's host"
    );
    // The shown worktree pushes nowhere; two others hold o/r on two hosts.
    let third = daruda_store::project::LaneRef {
        project: lane.project,
        lane: 89,
    };
    f.ws.update(cx, |ws, _| {
        ws.lane_scoped_mut(third).git.push_target = Some(crate::lane::git::PushTarget {
            remote: "origin".into(),
            url: "git@github.com:o/r.git".into(),
            branch: "other".into(),
            published: true,
            has_upstream: true,
            review: None,
        });
        ws.lane_scoped_mut(lane).git.push_target = None;
    });
    assert!(placed(cx).is_err(), "two hosts, none shown: asked");
}

#[gpui::test]
fn nothing_is_asked_of_github_while_it_is_off(cx: &mut TestAppContext) {
    let (f, _gh, _) = window(cx);
    crate::workspace::tests::pull_request_detail::set_github_enabled(cx, false);
    let placed =
        f.ws.read_with(cx, |ws, cx| ws.resolve_pr_address("#7", cx).is_err());
    assert!(placed);
}

/// The Git panel's entries land on the same detail the palette opens, from
/// any page.
#[gpui::test]
fn the_git_panel_opens_the_same_detail(cx: &mut TestAppContext) {
    let (f, _gh, _) = window(cx);
    let shown = |cx: &mut TestAppContext| {
        f.ws.read_with(cx, |ws, _| {
            ws.pages.pull_requests.detail().map(|detail| detail.key())
        })
    };
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            ws.open_pr_at(repo(), 7, PrDestination::Page, window, cx)
        });
    });
    cx.run_until_parked();
    assert_eq!(shown(cx), Some((repo(), 7)));

    f.ws.update(cx, |ws, cx| ws.show_page(Page::Tasks, cx));
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            ws.open_pr_at(repo(), 7, PrDestination::Page, window, cx)
        });
    });
    cx.run_until_parked();
    assert_eq!(shown(cx), Some((repo(), 7)));
    assert_eq!(
        f.ws.read_with(cx, |ws, _| ws.active_page()),
        Some(Page::PullRequests)
    );
}

/// A link into the pull request already shown moves it to the linked tab.
#[gpui::test]
fn a_url_into_the_shown_pull_request_moves_it_to_that_tab(cx: &mut TestAppContext) {
    let (f, _gh, _) = window(cx);
    let open = |url: &'static str, cx: &mut TestAppContext| {
        let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
        vcx.update(|window, cx| {
            f.ws.update(cx, |ws, cx| {
                let (repo, address) = ws.resolve_pr_address(url, cx).unwrap();
                ws.open_pr_by_address(repo, address, super::PrDestination::Page, window, cx);
            })
        });
        cx.run_until_parked();
    };
    let tab = |cx: &mut TestAppContext| {
        f.ws.read_with(cx, |ws, _| ws.pages.pull_requests.detail().map(|d| d.tab()))
    };
    open("https://github.com/o/r/pull/7/files", cx);
    assert_eq!(tab(cx), Some(PrTab::Files));
    open("https://github.com/o/r/pull/7/checks", cx);
    assert_eq!(tab(cx), Some(PrTab::Checks));
}

/// A link to another pull request while one is shown leaves that one
/// first, then opens on the link's tab: the tab goes with the opening, not
/// picked after it, so it holds whenever the leaving lands.
#[gpui::test]
fn a_url_to_another_pull_request_opens_it_on_its_tab(cx: &mut TestAppContext) {
    let (f, gh, _) = window(cx);
    gh.set_pr_detail(
        &repo(),
        8,
        crate::workspace::tests::pull_request_detail::detail(8, "other"),
    );
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    for url in [
        "https://github.com/o/r/pull/7",
        "https://github.com/o/r/pull/8/checks",
    ] {
        vcx.update(|window, cx| {
            f.ws.update(cx, |ws, cx| {
                let (repo, address) = ws.resolve_pr_address(url, cx).unwrap();
                ws.open_pr_by_address(repo, address, super::PrDestination::Page, window, cx);
            })
        });
        vcx.run_until_parked();
    }
    f.ws.read_with(cx, |ws, _| {
        let detail = ws.pages.pull_requests.detail().expect("shown");
        assert_eq!(detail.key(), (repo(), 8), "the other one replaced it");
        assert_eq!(detail.tab(), PrTab::Checks);
    });
}

/// A read made to open a link does not replace what the detail already
/// shown read for itself: started before, it may be the older of the two.
#[gpui::test]
fn a_link_read_leaves_the_shown_details_own_read(cx: &mut TestAppContext) {
    use crate::workspace::pages::pull_requests::detail::Load;
    use crate::workspace::pages::pull_requests::detail::PrOpening;
    let (f, _gh, _) = window(cx);
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            ws.open_pr_at(repo(), 7, PrDestination::Page, window, cx)
        });
    });
    cx.run_until_parked();
    let (item, shown) = f.ws.read_with(cx, |ws, _| {
        let detail = ws.pages.pull_requests.detail().expect("shown");
        let Load::Ready(pr) = detail.load() else {
            panic!("read");
        };
        (detail.item().clone(), pr.clone())
    });
    let mut older = shown.clone();
    older.summary.state = daruda_github::PrState::Closed;
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            let opening = PrOpening {
                tab: None,
                read: Some(Box::new(older)),
            };
            ws.open_pr_detail_as(item, opening, window, cx);
        })
    });
    cx.run_until_parked();
    f.ws.read_with(cx, |ws, _| {
        let detail = ws.pages.pull_requests.detail().expect("shown");
        assert!(matches!(detail.load(), Load::Ready(pr) if *pr == shown));
    });
}
