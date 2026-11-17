//! What a GitHub search adds to a tab's audience and state: words, and the
//! facets typed as `key:value`. Read from the search field's text, which
//! stays the one place the search is written; removing a facet rewrites it.
//! GPUI-free.

use daruda_github::{
    PrAudience, PrCommitStatus, PrRequestedReviewer, PrReviewFilter, PrSearchCriteria,
};

/// One facet the page knows how to send as a typed provider qualifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Facet {
    Author(String),
    Assignee(String),
    Label(String),
    Reviewer(String),
    Team(String),
    Review(PrReviewFilter),
    Draft(bool),
    Status(PrCommitStatus),
}

impl Facet {
    /// The `key:value` token this facet is written as.
    pub(crate) fn token(&self) -> String {
        match self {
            Self::Author(value) => format!("author:{value}"),
            Self::Assignee(value) => format!("assignee:{value}"),
            Self::Label(value) => format!("label:{value}"),
            Self::Reviewer(value) => format!("reviewer:{value}"),
            Self::Team(value) => format!("team:{value}"),
            Self::Review(review) => format!(
                "review:{}",
                match review {
                    PrReviewFilter::None => "none",
                    PrReviewFilter::Required => "required",
                    PrReviewFilter::Approved => "approved",
                    PrReviewFilter::ChangesRequested => "changes_requested",
                }
            ),
            Self::Draft(draft) => format!("draft:{draft}"),
            Self::Status(status) => format!(
                "status:{}",
                match status {
                    PrCommitStatus::Pending => "pending",
                    PrCommitStatus::Success => "success",
                    PrCommitStatus::Failure => "failure",
                }
            ),
        }
    }

    fn parse(key: &str, value: &str) -> Option<Self> {
        if value.is_empty() {
            return None;
        }
        let text = || value.to_owned();
        Some(match key {
            "author" => Self::Author(text()),
            "assignee" => Self::Assignee(text()),
            "label" => Self::Label(text()),
            "reviewer" => Self::Reviewer(text()),
            "team" => Self::Team(text()),
            "review" => Self::Review(match value {
                "none" => PrReviewFilter::None,
                "required" => PrReviewFilter::Required,
                "approved" => PrReviewFilter::Approved,
                "changes_requested" => PrReviewFilter::ChangesRequested,
                _ => return None,
            }),
            "draft" => Self::Draft(match value {
                "true" => true,
                "false" => false,
                _ => return None,
            }),
            "status" => Self::Status(match value {
                "pending" => PrCommitStatus::Pending,
                "success" => PrCommitStatus::Success,
                "failure" => PrCommitStatus::Failure,
                _ => return None,
            }),
            _ => return None,
        })
    }

    /// Two facets that send one qualifier cannot both be sent; the later
    /// replaces. A requested user and team share one.
    fn kind(&self) -> u8 {
        match self {
            Self::Author(_) => 0,
            Self::Assignee(_) => 1,
            Self::Label(_) => 2,
            Self::Reviewer(_) | Self::Team(_) => 3,
            Self::Review(_) => 4,
            Self::Draft(_) => 5,
            Self::Status(_) => 6,
        }
    }
}

/// Words and facets, in the order typed. Empty refines nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub(crate) struct Refine {
    pub terms: Vec<String>,
    pub facets: Vec<Facet>,
}

/// The empty refinement, for slots and updates that name the bare tab.
pub(crate) static NO_REFINE: Refine = Refine {
    terms: Vec::new(),
    facets: Vec::new(),
};

/// Why a refinement cannot be sent with a tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefineProblem {
    /// `author:` beside Mine, or `reviewer:`/`team:` beside Review
    /// requested: repeating the tab's own identity qualifier would widen
    /// GitHub's search rather than narrow it.
    NeedsAllTab,
    /// Quotes, backslashes or control characters, which a search sends as
    /// literal text only and this page does not guess how to escape.
    Unsupported,
}

impl Refine {
    /// Read `text`: each `key:value` the page knows is a facet, everything
    /// else is a word.
    pub(crate) fn parse(text: &str) -> Self {
        let mut refine = Self::default();
        for token in text.split_whitespace() {
            let facet = token
                .split_once(':')
                .and_then(|(key, value)| Facet::parse(&key.to_ascii_lowercase(), value));
            match facet {
                Some(facet) => {
                    refine.facets.retain(|held| held.kind() != facet.kind());
                    refine.facets.push(facet);
                }
                None => refine.terms.push(token.to_owned()),
            }
        }
        refine
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.terms.is_empty() && self.facets.is_empty()
    }

    /// The text that reads back as this refinement.
    pub(crate) fn text(&self) -> String {
        self.terms
            .iter()
            .cloned()
            .chain(self.facets.iter().map(Facet::token))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// This refinement without the facet at `index`.
    pub(crate) fn without_facet(&self, index: usize) -> Self {
        let mut refine = self.clone();
        if index < refine.facets.len() {
            refine.facets.remove(index);
        }
        refine
    }

    /// What keeps it from being sent with `audience`, if anything.
    pub(crate) fn problem(&self, audience: PrAudience) -> Option<RefineProblem> {
        let identity = self.facets.iter().any(|facet| match facet {
            Facet::Author(_) => audience == PrAudience::Mine,
            Facet::Reviewer(_) | Facet::Team(_) => audience == PrAudience::ReviewRequested,
            _ => false,
        });
        if identity {
            return Some(RefineProblem::NeedsAllTab);
        }
        let unsupported = |value: &str| {
            value
                .chars()
                .any(|c| c == '"' || c == '\\' || c.is_control())
        };
        let values = self.facets.iter().filter_map(|facet| match facet {
            Facet::Author(v)
            | Facet::Assignee(v)
            | Facet::Label(v)
            | Facet::Reviewer(v)
            | Facet::Team(v) => Some(v.as_str()),
            Facet::Review(_) | Facet::Draft(_) | Facet::Status(_) => None,
        });
        if self
            .terms
            .iter()
            .map(String::as_str)
            .chain(values)
            .any(unsupported)
        {
            return Some(RefineProblem::Unsupported);
        }
        None
    }

    /// Add this refinement to `criteria`, whose audience and state are set.
    pub(crate) fn apply(&self, criteria: &mut PrSearchCriteria) {
        criteria.terms = self.terms.clone();
        for facet in &self.facets {
            match facet {
                Facet::Author(v) => criteria.author = Some(v.clone()),
                Facet::Assignee(v) => criteria.assignee = Some(v.clone()),
                Facet::Label(v) => criteria.label = Some(v.clone()),
                Facet::Reviewer(v) => {
                    criteria.requested_reviewer = Some(PrRequestedReviewer::User(v.clone()));
                }
                Facet::Team(v) => {
                    criteria.requested_reviewer = Some(PrRequestedReviewer::Team(v.clone()));
                }
                Facet::Review(review) => criteria.review = Some(*review),
                Facet::Draft(draft) => criteria.draft = Some(*draft),
                Facet::Status(status) => criteria.commit_status = Some(*status),
            }
        }
    }
}

#[cfg(test)]
mod tests;
