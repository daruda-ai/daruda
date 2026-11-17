//! A pull request's changed files, one page at a time, as `gh api
//! repos/{owner}/{name}/pulls/{number}/files?per_page=100&page=N` answers
//! them: what changed in each, by how many lines, and its patch when GitHub
//! sends one. Read page by page so a large pull request keeps what arrived.

use serde::Deserialize;

use crate::GhError;

/// GitHub lists at most this many files of one pull request.
pub const PR_FILES_MAX: usize = 3_000;
/// Files per page, GitHub's largest.
pub const PR_FILES_PER_PAGE: usize = 100;
/// The last page GitHub's cap leaves.
const LAST_PAGE: u32 = (PR_FILES_MAX / PR_FILES_PER_PAGE) as u32;

/// How a file changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileChange {
    Added,
    Removed,
    Modified,
    /// Moved from `from`, possibly with edits.
    Renamed {
        from: String,
    },
    /// Copied from `from`.
    Copied {
        from: String,
    },
    /// Only its mode or similar changed.
    Changed,
    Unchanged,
}

/// One changed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrFile {
    pub path: String,
    pub change: FileChange,
    pub additions: u32,
    pub deletions: u32,
    /// The unified diff's hunks. `None` when GitHub leaves it out: a binary
    /// file, or one too large for the API to diff. Empty when nothing in
    /// the text changed, as for a mode change.
    pub patch: Option<String>,
    /// GitHub's blob id of the file's content as listed: it changes exactly
    /// when the content does, which is what a mark of having viewed the file
    /// is checked against. `None` when GitHub leaves it out.
    pub blob: Option<String>,
}

/// Every file GitHub lists for a pull request: what a fake is scripted
/// with, and paged as GitHub pages it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrFiles {
    pub files: Vec<PrFile>,
}

/// One page of a pull request's files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrFilesPage {
    pub files: Vec<PrFile>,
    /// The page to read next: this one was full. A full page can be the
    /// last, in which case the next is empty.
    pub next: Option<u32>,
    /// A full page at GitHub's cap: the pull request may change more files
    /// than GitHub lists.
    pub capped: bool,
}

impl PrFilesPage {
    /// What a page of `files`, page number `page`, leads to.
    pub fn of(files: Vec<PrFile>, page: u32) -> Self {
        let full = files.len() >= PR_FILES_PER_PAGE;
        Self {
            files,
            next: (full && page < LAST_PAGE).then_some(page + 1),
            capped: full && page >= LAST_PAGE,
        }
    }
}

/// What a file holds at one commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileText {
    Text(String),
    /// Not UTF-8, or holding a NUL byte: nothing to diff by line.
    Binary,
    /// No such file at that commit, as for one added or removed.
    Absent,
}

impl FileText {
    /// Read a file's raw bytes.
    pub(crate) fn of(bytes: Vec<u8>) -> Self {
        if bytes.contains(&0) {
            return Self::Binary;
        }
        String::from_utf8(bytes).map_or(Self::Binary, Self::Text)
    }
}

/// `path` as a URL path, each segment percent-encoded: a file name may hold
/// spaces, `#`, `?` or any other byte.
pub(crate) fn url_path(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            segment
                .bytes()
                .map(|byte| match byte {
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                        char::from(byte).to_string()
                    }
                    _ => format!("%{byte:02X}"),
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// The merge base `--jq .merge_base_commit.sha` prints from a comparison.
pub(crate) fn parse_merge_base(out: &str) -> Result<String, GhError> {
    let sha = out.trim();
    if sha.len() >= 7 && sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(sha.to_owned())
    } else {
        Err(GhError::Parse(format!("compare: no merge base in {sha:?}")))
    }
}

#[derive(Deserialize)]
struct RawFile {
    filename: String,
    status: String,
    additions: u32,
    deletions: u32,
    #[serde(default)]
    patch: Option<String>,
    #[serde(default)]
    previous_filename: Option<String>,
    #[serde(default)]
    sha: Option<String>,
}

/// Parse page `page`: an array of files.
pub(crate) fn parse_pr_files_page(json: &str, page: u32) -> Result<PrFilesPage, GhError> {
    let raw: Vec<RawFile> =
        serde_json::from_str(json).map_err(|e| GhError::Parse(format!("pr files: {e}")))?;
    let files: Vec<PrFile> = raw
        .into_iter()
        .map(|raw| {
            let from = || raw.previous_filename.clone().unwrap_or_default();
            let change = match raw.status.as_str() {
                "added" => FileChange::Added,
                "removed" => FileChange::Removed,
                "modified" => FileChange::Modified,
                "renamed" => FileChange::Renamed { from: from() },
                "copied" => FileChange::Copied { from: from() },
                "unchanged" => FileChange::Unchanged,
                // "changed", and any status GitHub adds later.
                _ => FileChange::Changed,
            };
            PrFile {
                path: raw.filename,
                change,
                additions: raw.additions,
                deletions: raw.deletions,
                patch: raw.patch,
                blob: raw.sha,
            }
        })
        .collect();
    Ok(PrFilesPage::of(files, page))
}

#[cfg(test)]
mod tests {
    //! `pr_files_14116.json` (a rename and an edit) and `pr_files_15003.json`
    //! (one added file whose name holds a space) are the one page of
    //! `gh api --paginate --slurp repos/cli/cli/pulls/<n>/files?per_page=100`
    //! with gh 2.74.2, 2026-11-17: `--slurp`'s outer array removed, which
    //! leaves exactly what the single page answers.

    use super::*;

    fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn recorded_pages_read_as_changes_with_their_patches() {
        let renamed = parse_pr_files_page(&fixture("pr_files_14116.json"), 1).unwrap();
        assert_eq!(
            (renamed.next, renamed.capped),
            (None, false),
            "a short page is the last"
        );
        assert_eq!(renamed.files.len(), 2);
        assert_eq!(renamed.files[0].path, ".github/skills/code-review/SKILL.md");
        assert_eq!(
            renamed.files[0].change,
            FileChange::Renamed {
                from: ".github/skills/cli-code-reviewer/SKILL.md".into()
            }
        );
        assert_eq!(renamed.files[1].change, FileChange::Modified);
        assert!(renamed.files.iter().all(|file| file.patch.is_some()));
        assert_eq!(
            renamed.files[0].blob.as_deref(),
            Some("af523ae55eee50554bf6454251bf0ce82ecca903"),
            "the content's blob id, which viewed marks are checked against"
        );

        let added = parse_pr_files_page(&fixture("pr_files_15003.json"), 1).unwrap();
        assert_eq!(added.files[0].path, "READ ME");
        assert_eq!(added.files[0].change, FileChange::Added);
        assert_eq!(
            (added.files[0].additions, added.files[0].deletions),
            (122, 0)
        );
        assert!(added.files[0].patch.as_deref().unwrap().starts_with("@@"));
    }

    #[test]
    fn a_file_without_a_patch_and_unknown_statuses_are_kept() {
        let mut page: serde_json::Value =
            serde_json::from_str(&fixture("pr_files_14116.json")).unwrap();
        let file = &mut page[1];
        file.as_object_mut().unwrap().remove("patch");
        file["status"] = "removed".into();
        page[0]["status"] = "something-new".into();
        let files = parse_pr_files_page(&page.to_string(), 1).unwrap().files;
        assert_eq!(files[1].change, FileChange::Removed);
        assert_eq!(files[1].patch, None, "binary or too large: no diff");
        assert_eq!(files[0].change, FileChange::Changed);
        page[0]["patch"] = "".into();
        let files = parse_pr_files_page(&page.to_string(), 1).unwrap().files;
        assert_eq!(
            files[0].patch.as_deref(),
            Some(""),
            "an empty patch is no line change, not a missing diff"
        );
        assert!(matches!(
            parse_pr_files_page("{}", 1),
            Err(GhError::Parse(_))
        ));
    }

    #[test]
    fn file_bytes_read_as_text_unless_binary() {
        // (label, bytes, expected)
        let cases = [
            (
                "text",
                b"fn main() {}\n".to_vec(),
                FileText::Text("fn main() {}\n".into()),
            ),
            ("a NUL byte", b"PNG\0data".to_vec(), FileText::Binary),
            ("not UTF-8", vec![0xff, 0xfe, b'a'], FileText::Binary),
            ("empty", Vec::new(), FileText::Text(String::new())),
        ];
        for (label, bytes, expected) in cases {
            assert_eq!(FileText::of(bytes), expected, "{label}");
        }
    }

    #[test]
    fn a_path_is_sent_with_each_segment_encoded() {
        assert_eq!(url_path("src/a b/c#1?.rs"), "src/a%20b/c%231%3F.rs");
        assert_eq!(url_path("READ ME"), "READ%20ME");
        assert_eq!(url_path("dir/한.txt"), "dir/%ED%95%9C.txt");
    }

    #[test]
    fn the_merge_base_is_read_from_a_comparison() {
        // What `--jq .merge_base_commit.sha` printed for cli/cli#14116,
        // 2026-11-18.
        assert_eq!(
            parse_merge_base("144728803d9cd4c540d33a26ece728db65e2b357\n").unwrap(),
            "144728803d9cd4c540d33a26ece728db65e2b357"
        );
        for out in ["", "null\n", "{}"] {
            assert!(
                matches!(parse_merge_base(out), Err(GhError::Parse(_))),
                "{out:?}"
            );
        }
    }

    #[test]
    fn a_full_page_leads_to_the_next_until_the_cap() {
        let file = |n: usize| PrFile {
            path: format!("f{n}"),
            change: FileChange::Modified,
            additions: 1,
            deletions: 0,
            patch: None,
            blob: None,
        };
        let full = || (0..PR_FILES_PER_PAGE).map(file).collect::<Vec<_>>();
        // (label, files, page, next, capped)
        let cases = [
            ("a short page ends it", vec![file(0)], 1, None, false),
            ("an empty page ends it", Vec::new(), 2, None, false),
            ("a full page leads on", full(), 1, Some(2), false),
            (
                "a full page before the cap leads on",
                full(),
                29,
                Some(30),
                false,
            ),
            ("a full last page is capped", full(), 30, None, true),
        ];
        for (label, files, page, next, capped) in cases {
            let page = PrFilesPage::of(files, page);
            assert_eq!((page.next, page.capped), (next, capped), "{label}");
        }
    }
}
