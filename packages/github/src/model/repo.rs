//! Which repository, and which links may be opened.

/// The public GitHub host. `gh` trouble on it is worth telling the user
/// about unasked; another host may not be GitHub at all.
pub const GITHUB_HOST: &str = "github.com";

/// A GitHub repository. Every part is lowercased by [`RepoId::new`]: GitHub
/// compares them case-insensitively, and so must every lookup keyed by this.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RepoId {
    host: String,
    owner: String,
    name: String,
}

impl RepoId {
    pub fn new(host: &str, owner: &str, name: &str) -> Self {
        Self {
            host: host.to_ascii_lowercase(),
            owner: owner.to_ascii_lowercase(),
            name: name.to_ascii_lowercase(),
        }
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether this is on the public GitHub host, the only one known to be
    /// GitHub without asking `gh`. Any other host may be a GitHub Enterprise
    /// Server or another forge altogether; a URL cannot tell which.
    pub fn on_github(&self) -> bool {
        self.host == GITHUB_HOST
    }

    /// `host/owner/name`, the form `gh --repo` accepts for any host.
    pub fn qualified(&self) -> String {
        format!("{}/{}/{}", self.host, self.owner, self.name)
    }
}

/// Whether `url` is a web page a browser may be handed. A URL from GitHub
/// may name any scheme; `file:`, a script or another app's scheme must not
/// reach the system opener.
pub fn is_web_url(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

/// Whether a link written in text from GitHub (a description, a comment)
/// may be opened: a web page or an e-mail address. Anyone can write that
/// text, so nothing else it links to reaches the system opener.
pub fn is_openable_link(url: &str) -> bool {
    is_web_url(url) || url.starts_with("mailto:")
}
