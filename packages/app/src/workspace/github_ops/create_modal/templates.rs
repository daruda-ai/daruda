//! The worktree's pull request templates, where GitHub looks for them:
//! `pull_request_template.md` in the root, `docs/` or `.github/` (one,
//! offered by default), and every `.md` in a `PULL_REQUEST_TEMPLATE/`
//! directory there (chosen by name). File names match case-insensitively.
//! Only the worktree's own files: a template GitHub resolves from an
//! organisation's `.github` repository is not read, and a symlink is never
//! followed, so nothing outside the worktree is read.
// ASSUMPTION: not verified against GitHub's documentation — the places,
// their order, `.md` only and case-insensitive names are as commonly
// described; a template GitHub would use and this misses is not offered.

use std::io::Read as _;
use std::path::{Path, PathBuf};

/// Where GitHub looks, in that order.
const DIRS: [&str; 3] = ["", "docs", ".github"];
const SINGLE: &str = "pull_request_template.md";
const MULTIPLE: &str = "pull_request_template";
/// A template larger than this is not offered: it is not a template.
const TEMPLATE_MAX_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrTemplate {
    /// Its path from the worktree root, as the form names it.
    pub path: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Templates {
    /// The single template GitHub fills a new pull request with.
    pub default: Option<PrTemplate>,
    /// The templates of a `PULL_REQUEST_TEMPLATE/` directory, by path.
    pub named: Vec<PrTemplate>,
}

impl Templates {
    pub(crate) fn is_empty(&self) -> bool {
        self.default.is_none() && self.named.is_empty()
    }

    /// Every template, the default first.
    pub(crate) fn all(&self) -> impl Iterator<Item = &PrTemplate> {
        self.default.iter().chain(&self.named)
    }
}

/// Find `root`'s templates. Unreadable files are left out.
pub(crate) fn find_templates(root: &Path) -> Templates {
    let mut found = Templates::default();
    for dir in DIRS {
        let dir_path = root.join(dir);
        for entry in entries(&dir_path) {
            let name = entry
                .file_name()
                .map(|name| name.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if name == SINGLE && found.default.is_none() && is_plain_file(&entry) {
                found.default = read(root, &entry);
            } else if name == MULTIPLE && is_plain_dir(&entry) {
                let mut named: Vec<PrTemplate> = entries(&entry)
                    .into_iter()
                    .filter(|file| {
                        is_plain_file(file)
                            && file
                                .extension()
                                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
                    })
                    .filter_map(|file| read(root, &file))
                    .collect();
                named.sort_by(|a, b| a.path.cmp(&b.path));
                found.named.extend(named);
            }
        }
    }
    found
}

fn entries(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = read.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    paths
}

/// A regular file itself, not a symlink to one: a link could name any
/// file of the user's, and its text would land in the description.
fn is_plain_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_file())
}

fn is_plain_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_dir())
}

fn read(root: &Path, file: &Path) -> Option<PrTemplate> {
    // Read at most one byte past the limit: a file grown since it was
    // listed is still refused, never read whole.
    let mut bytes = Vec::new();
    std::fs::File::open(file)
        .ok()?
        .take(TEMPLATE_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > TEMPLATE_MAX_BYTES {
        return None;
    }
    let body = String::from_utf8(bytes).ok()?;
    let path = file
        .strip_prefix(root)
        .ok()?
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    Some(PrTemplate { path, body })
}

#[cfg(test)]
mod tests;
