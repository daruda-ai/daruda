//! Which GitHub repository a git remote URL names.

use crate::RepoId;

/// The repository `url` names, or `None` for a local path or anything not
/// shaped `host/owner/name`. Accepts `https://`, `http://`, `ssh://`,
/// `git://` and scp-like `user@host:owner/name`, with or without `.git`.
///
/// The port is dropped: `gh` identifies a host by name alone. Whether the host
/// is GitHub at all is for `gh auth status` to answer, not a URL.
pub fn parse_remote(url: &str) -> Option<RepoId> {
    let url = url.trim();
    let (authority, path) = if let Some((scheme, rest)) = url.split_once("://") {
        if !matches!(scheme, "https" | "http" | "ssh" | "git") {
            return None;
        }
        rest.split_once('/')?
    } else {
        // scp-like syntax needs a `:` before any `/`; otherwise it is a path.
        let (authority, path) = url.split_once(':')?;
        // `C:/repos/r.git` is a Windows path (git reads it so on Windows);
        // a one-letter scp host elsewhere is no GitHub remote either.
        if authority.contains('/') || authority.len() <= 1 {
            return None;
        }
        (authority, path)
    };
    let host = host_of(authority)?;
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.split('/');
    let (owner, name) = (parts.next()?, parts.next()?);
    if parts.next().is_some() || owner.is_empty() || name.is_empty() {
        return None;
    }
    Some(RepoId {
        host,
        owner: owner.to_ascii_lowercase(),
        name: name.to_ascii_lowercase(),
    })
}

/// `user@host:port` → `host`, lowercased. GitHub's SSH-over-443 endpoint is
/// the same host as `github.com` to `gh`.
fn host_of(authority: &str) -> Option<String> {
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = host.split_once(':').map_or(host, |(h, _)| h);
    if host.is_empty() {
        return None;
    }
    let host = host.to_ascii_lowercase();
    Some(if host == "ssh.github.com" {
        "github.com".to_owned()
    } else {
        host
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_port_is_not_part_of_the_host() {
        assert_eq!(host_of("git@ghe.corp:2222").as_deref(), Some("ghe.corp"));
    }
}
