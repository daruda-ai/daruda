//! What the Pull requests page lists, built from plain values so every case
//! is testable without a window: each GitHub repository this window pushes
//! to, its pull requests under the shown filter, the worktree working on
//! each, and why a host has nothing to show.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use daruda_core::age::{self, AgeBucket};
use daruda_github::availability::{BackoffReason, GhAvailability};
use daruda_github::{
    CheckRollup, GITHUB_HOST, PrFilter, PrListItem, PrSearch, PrState, RepoId, ReviewDecision,
};
use daruda_store::project::LaneRef;

use crate::surface::strings::{git as git_s, pull_requests as s};
use crate::workspace::github_ops::pr_list::{HostList, PrListState};
use crate::workspace::github_ops::tone::PrTone;

/// What the page needs to know about one lane to match it to a pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct LaneFacts {
    pub lane: LaneRef,
    pub name: String,
    /// The GitHub repository the lane's branch pushes to.
    pub repo: Option<RepoId>,
    /// The branch checked out in the lane.
    pub branch: Option<String>,
    /// The pull request the lane's branch is linked to.
    pub pr: Option<(RepoId, u64)>,
}

/// The branch a worktree made to review fork pull request `number` is on.
pub(in crate::workspace) fn review_branch(number: u64) -> String {
    format!("pr-{number}")
}

/// The lane working on `item`: the one whose branch is linked to it, or, for
/// a pull request from a fork, a lane on its review branch in a project that
/// pushes to the pull request's repository.
pub(in crate::workspace) fn linked_lane<'a>(
    item: &PrListItem,
    lanes: &'a [LaneFacts],
) -> Option<&'a LaneFacts> {
    let linked = lanes.iter().find(|lane| {
        lane.pr
            .as_ref()
            .is_some_and(|(repo, number)| *repo == item.repo && *number == item.pr.number)
    });
    if linked.is_some() || !item.pr.cross_repo {
        return linked;
    }
    let branch = review_branch(item.pr.number);
    let projects: Vec<_> = lanes
        .iter()
        .filter(|lane| lane.repo.as_ref() == Some(&item.repo))
        .map(|lane| lane.lane.project)
        .collect();
    lanes.iter().find(|lane| {
        projects.contains(&lane.lane.project) && lane.branch.as_deref() == Some(branch.as_str())
    })
}

/// Why a host's `gh` cannot be asked at all; the user has to act.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum Blocked {
    Missing,
    LoggedOut,
}

/// Why what a group shows may be out of date; it heals on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum Trouble {
    RateLimited,
    Unreachable,
    SearchFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum GroupBody {
    Loading,
    Blocked(Blocked),
    Listed {
        rows: Vec<PrRow>,
        /// GitHub matched more on this host than one search returns.
        truncated: bool,
        trouble: Option<Trouble>,
    },
}

/// One repository's pull requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct PrGroup {
    pub repo: RepoId,
    /// `owner/name`.
    pub label: String,
    /// The host, and how many rows once there are any to count.
    pub meta: String,
    pub body: GroupBody,
}

/// The worktree a row links to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct RowLane {
    pub lane: LaneRef,
    pub name: String,
}

/// One pull request, ready to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct PrRow {
    /// Unique on the page: `host/owner/name#number`.
    pub key: String,
    /// `#480 Adjust sidebar…`.
    pub title: String,
    /// `head → base · author · 2h ago`.
    pub meta: String,
    pub tone: PrTone,
    pub draft: bool,
    pub fork: bool,
    pub checks: CheckRollup,
    pub review: Option<ReviewDecision>,
    pub comments: u32,
    pub lane: Option<RowLane>,
    /// Only a URL a browser may be handed.
    pub url: Option<String>,
}

/// What the page body shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum PageBody {
    /// `[github] enabled` is off.
    Off,
    /// No lane in the window pushes to a GitHub repository.
    NoRepos,
    Groups(Vec<PrGroup>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct PrPageList {
    pub filter: PrFilter,
    pub body: PageBody,
}

/// Everything the page needs from the workspace, as plain values.
pub(in crate::workspace) struct PageInputs<'a> {
    pub enabled: bool,
    pub filter: PrFilter,
    pub repos: &'a BTreeSet<RepoId>,
    pub list: &'a PrListState,
    pub lanes: &'a [LaneFacts],
    pub now: DateTime<Utc>,
}

/// The page for `inputs`; `availability` says what is known about `gh` on a host.
pub(in crate::workspace) fn page(
    inputs: &PageInputs<'_>,
    availability: impl Fn(&str) -> GhAvailability,
) -> PrPageList {
    let body = if !inputs.enabled {
        PageBody::Off
    } else if inputs.repos.is_empty() {
        PageBody::NoRepos
    } else {
        PageBody::Groups(
            inputs
                .repos
                .iter()
                .filter_map(|repo| {
                    let host = repo.host();
                    let body =
                        group_body(repo, availability(host), inputs.list.host(host), inputs)?;
                    let meta = match &body {
                        GroupBody::Listed { rows, .. } => s::group_count(host, rows.len()),
                        GroupBody::Loading | GroupBody::Blocked(_) => host.to_owned(),
                    };
                    Some(PrGroup {
                        repo: repo.clone(),
                        label: format!("{}/{}", repo.owner(), repo.name()),
                        meta,
                        body,
                    })
                })
                .collect(),
        )
    };
    PrPageList {
        filter: inputs.filter,
        body,
    }
}

/// What `repo`'s group shows, or `None` to leave it out: a host other than
/// github.com without a usable `gh` may not be GitHub at all, as the Git
/// panel's strip also decides.
fn group_body(
    repo: &RepoId,
    availability: GhAvailability,
    list: &HostList,
    inputs: &PageInputs<'_>,
) -> Option<GroupBody> {
    let trouble = match availability {
        GhAvailability::Missing | GhAvailability::LoggedOut if repo.host() != GITHUB_HOST => {
            return None;
        }
        GhAvailability::Missing => return Some(GroupBody::Blocked(Blocked::Missing)),
        GhAvailability::LoggedOut => return Some(GroupBody::Blocked(Blocked::LoggedOut)),
        GhAvailability::Backoff {
            reason: BackoffReason::RateLimited,
            ..
        } => Some(Trouble::RateLimited),
        GhAvailability::Backoff {
            reason: BackoffReason::Transient,
            ..
        } => Some(Trouble::Unreachable),
        GhAvailability::Unknown | GhAvailability::Ready => None,
    };
    let listed = |search: Option<&PrSearch>, trouble| GroupBody::Listed {
        rows: search.map_or_else(Vec::new, |search| rows(repo, search, inputs)),
        truncated: search.is_some_and(|search| search.truncated),
        trouble,
    };
    Some(match list {
        // A paused host is not searched; say why rather than wait forever.
        HostList::Loading if trouble.is_some() => listed(None, trouble),
        HostList::Loading => GroupBody::Loading,
        HostList::Ready(search) => listed(Some(search), trouble),
        HostList::Failed { last } => listed(last.as_ref(), trouble.or(Some(Trouble::SearchFailed))),
    })
}

fn rows(repo: &RepoId, search: &PrSearch, inputs: &PageInputs<'_>) -> Vec<PrRow> {
    search
        .items
        .iter()
        .filter(|item| item.repo == *repo)
        .map(|item| row(item, inputs))
        .collect()
}

fn row(item: &PrListItem, inputs: &PageInputs<'_>) -> PrRow {
    let pr = &item.pr;
    let author = item.author.clone().unwrap_or_else(s::author_deleted);
    let meta = match age_seconds(&pr.updated_at, inputs.now) {
        Some(seconds) => s::row_meta(
            &pr.head_branch,
            &item.base_branch,
            author,
            age_label(seconds),
        ),
        None => s::row_meta_undated(&pr.head_branch, &item.base_branch, author),
    };
    PrRow {
        key: format!("{}#{}", item.repo.qualified(), pr.number),
        title: format!("{} {}", git_s::pr_number(pr.number), pr.title),
        meta,
        tone: PrTone::of(pr),
        draft: pr.state == PrState::Draft,
        fork: pr.cross_repo,
        checks: pr.checks,
        review: pr.review,
        comments: item.comments,
        lane: linked_lane(item, inputs.lanes).map(|lane| RowLane {
            lane: lane.lane,
            name: lane.name.clone(),
        }),
        url: pr.web_url().map(str::to_owned),
    }
}

/// Whole seconds since `updated_at` (RFC 3339), or `None` if GitHub's
/// timestamp does not parse. A clock behind GitHub's reads as now.
fn age_seconds(updated_at: &str, now: DateTime<Utc>) -> Option<u64> {
    let then = DateTime::parse_from_rfc3339(updated_at).ok()?;
    let seconds = (now - then.with_timezone(&Utc)).num_seconds().max(0);
    u64::try_from(seconds).ok()
}

fn age_label(seconds: u64) -> String {
    match age::bucket(seconds) {
        AgeBucket::JustNow => s::updated_now(),
        AgeBucket::Minutes(n) => s::updated_minutes(n),
        AgeBucket::Hours(n) => s::updated_hours(n),
        AgeBucket::Days(n) => s::updated_days(n),
    }
}

/// The word a review decision shows.
pub(in crate::workspace) fn review_label(review: ReviewDecision) -> String {
    match review {
        ReviewDecision::Approved => s::review_approved(),
        ReviewDecision::ChangesRequested => s::review_changes_requested(),
        ReviewDecision::ReviewRequired => s::review_required(),
    }
}

/// The filters the page offers, in the order the strip shows them.
pub(in crate::workspace) const FILTERS: [PrFilter; 3] =
    [PrFilter::Mine, PrFilter::ReviewRequested, PrFilter::AllOpen];

pub(in crate::workspace) fn filter_label(filter: PrFilter) -> String {
    match filter {
        PrFilter::Mine => s::filter_mine(),
        PrFilter::ReviewRequested => s::filter_review_requested(),
        PrFilter::AllOpen => s::filter_all_open(),
    }
}
