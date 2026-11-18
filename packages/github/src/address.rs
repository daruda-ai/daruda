//! Which pull request a pasted reference names, and what in it: a web URL
//! (`https://<host>/<owner>/<name>/pull/<n>`, any host, so an Enterprise
//! server keeps its own authority), `owner/name#n` or a bare `#n` / `n`
//! whose repository the context supplies. The URL's tab (`/files`,
//! `/checks`, `/commits`) and a review comment or conversation comment
//! anchor travel with it. Nothing here reads a URL a terminal or text did
//! not explicitly hand over.

use crate::RepoId;

/// What within the pull request a reference points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrTarget {
    /// The changed files.
    Files,
    /// The checks.
    Checks,
    /// The commits.
    Commits,
    /// A review comment (`#discussion_r<id>`).
    ReviewComment(u64),
    /// A comment on the conversation (`#issuecomment-<id>`).
    Comment(u64),
}

/// A pull request a reference names: its repository when the reference
/// says, its number, and what in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrAddress {
    /// `None` for a bare number: the context names the repository.
    pub repo: Option<RepoId>,
    /// Only `owner/name` was written: the host is the context's.
    pub host_unknown: bool,
    pub number: u64,
    pub target: Option<PrTarget>,
}

/// Read `text` as a pull request reference; `None` when it is not one.
pub fn parse_pr_address(text: &str) -> Option<PrAddress> {
    let text = text.trim();
    let lower = text.to_ascii_lowercase();
    for scheme in ["https://", "http://"] {
        if lower.starts_with(scheme) {
            return parse_url(&text[scheme.len()..]);
        }
    }
    if let Some((repo, number)) = text.split_once('#')
        && !repo.is_empty()
    {
        let (owner, name) = repo.split_once('/')?;
        if owner.is_empty()
            || name.is_empty()
            || name.contains('/')
            || !plain(owner)
            || !plain(name)
        {
            return None;
        }
        return Some(PrAddress {
            repo: Some(RepoId::new(crate::GITHUB_HOST, owner, name)),
            host_unknown: true,
            number: number_of(number)?,
            target: None,
        });
    }
    let number = text.strip_prefix('#').unwrap_or(text);
    Some(PrAddress {
        repo: None,
        host_unknown: true,
        number: number_of(number)?,
        target: None,
    })
}

/// `<host>/<owner>/<name>/pull/<n>[/<tab>][#anchor]`, query ignored.
fn parse_url(rest: &str) -> Option<PrAddress> {
    let (path, anchor) = match rest.split_once('#') {
        Some((path, anchor)) => (path, Some(anchor)),
        None => (rest, None),
    };
    let path = path.split('?').next().unwrap_or(path).trim_end_matches('/');
    let mut parts = path.split('/');
    let host = parts.next()?.split(':').next()?;
    // GitHub serves pull requests at `www.` too; `gh` knows the host bare.
    let host = if host.eq_ignore_ascii_case("www.github.com") {
        crate::GITHUB_HOST
    } else {
        host
    };
    let (owner, name) = (parts.next()?, parts.next()?);
    if parts.next()? != "pull" || host.is_empty() || !plain(owner) || !plain(name) {
        return None;
    }
    let number = number_of(parts.next()?)?;
    let tab = match parts.next() {
        None => None,
        Some("files") => Some(PrTarget::Files),
        Some("checks") => Some(PrTarget::Checks),
        Some("commits") => Some(PrTarget::Commits),
        // Another page of the pull request: the pull request itself.
        Some(_) => None,
    };
    let comment = anchor.and_then(|anchor| {
        if let Some(id) = anchor.strip_prefix("discussion_r") {
            id.parse().ok().map(PrTarget::ReviewComment)
        } else if let Some(id) = anchor.strip_prefix("issuecomment-") {
            id.parse().ok().map(PrTarget::Comment)
        } else {
            None
        }
    });
    Some(PrAddress {
        repo: Some(RepoId::new(host, owner, name)),
        host_unknown: false,
        number,
        target: comment.or(tab),
    })
}

/// A pull request number: digits only, not zero.
fn number_of(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok().filter(|number| *number > 0)
}

/// An owner or repository name: no spaces, slashes or control characters.
fn plain(part: &str) -> bool {
    !part.is_empty()
        && part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

#[cfg(test)]
mod tests;
