//! How the app answers the engine's "where is the flow this one includes":
//! from the same three scopes, with the same precedence, as the picker.

use super::flow_paths::FlowSources;
use daruda_flow::{FlowError, IncludeSource, IncludedFlow, Inspected, LoadedFlow};

impl IncludeSource for FlowSources {
    fn find(&self, flow: &str) -> Option<IncludedFlow> {
        let (path, _) = self.locate(flow)?;
        Some(IncludedFlow {
            text: std::fs::read_to_string(&path).ok()?,
            dir: path.parent()?.to_path_buf(),
        })
    }
}

/// One lookup an expansion made: the name, and the file it found then with a
/// SHA-256 of its bytes — `None` when nothing answered to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::workspace) struct IncludeSeen {
    name: String,
    file: Option<(std::path::PathBuf, [u8; 32])>,
}

/// [`FlowSources`] that remember every lookup, so a reader can later tell
/// whether the same expansion would read the same bytes: an included flow
/// edited, shadowed by a narrower scope, or newly there all differ.
pub(in crate::workspace) struct RecordingSources<'a> {
    sources: &'a FlowSources,
    seen: std::cell::RefCell<Vec<IncludeSeen>>,
}

impl<'a> RecordingSources<'a> {
    pub(in crate::workspace) fn new(sources: &'a FlowSources) -> Self {
        Self {
            sources,
            seen: Default::default(),
        }
    }

    pub(in crate::workspace) fn into_seen(self) -> Vec<IncludeSeen> {
        self.seen.into_inner()
    }
}

impl IncludeSource for RecordingSources<'_> {
    fn find(&self, flow: &str) -> Option<IncludedFlow> {
        use sha2::{Digest as _, Sha256};
        let found = self.sources.locate(flow).and_then(|(path, _)| {
            let bytes = std::fs::read(&path).ok()?;
            Some((path, bytes))
        });
        self.seen.borrow_mut().push(IncludeSeen {
            name: flow.to_string(),
            file: found
                .as_ref()
                .map(|(path, bytes)| (path.clone(), Sha256::digest(bytes).into())),
        });
        let (path, bytes) = found?;
        Some(IncludedFlow {
            text: String::from_utf8_lossy(&bytes).into_owned(),
            dir: path.parent()?.to_path_buf(),
        })
    }
}

/// `daruda_flow::load`, expanding includes from `sources` when there are
/// any to look in. Without them an include is refused, which is the
/// engine's answer for a flow read where no flows can be found.
pub(in crate::workspace) fn load_flow(
    text: &str,
    profile: Option<&str>,
    sources: Option<&FlowSources>,
) -> Result<LoadedFlow, FlowError> {
    match sources {
        Some(sources) => daruda_flow::load_with(text, profile, sources),
        None => daruda_flow::load(text, profile),
    }
}

/// [`load_flow`]'s `inspect`.
pub(in crate::workspace) fn inspect_flow(
    text: &str,
    profile: Option<&str>,
    sources: Option<&FlowSources>,
) -> Result<Inspected, FlowError> {
    match sources {
        Some(sources) => daruda_flow::inspect_with(text, profile, sources),
        None => daruda_flow::inspect(text, profile),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::flow_paths::FlowOrigin;

    fn write(dir: &std::path::Path, name: &str, text: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), text).unwrap();
    }

    /// The narrowest scope with the name wins, as it does in the picker; a
    /// lane's repository flows live under its `.daruda/flows`.
    #[test]
    fn an_include_is_found_where_the_picker_would_find_it() {
        let root = tempfile::tempdir().unwrap();
        let sources = FlowSources {
            lane: root.path().join("lane"),
            project: Some(root.path().join("project")),
            global: root.path().join("global"),
        };
        let repo = crate::workspace::flow_paths::flows_dir(&sources.lane);
        write(&sources.global, "review.yaml", "global");
        write(sources.project.as_ref().unwrap(), "review.yaml", "project");
        write(
            sources.project.as_ref().unwrap(),
            "lint.yml",
            "project lint",
        );
        write(&repo, "review.yaml", "repo");

        let rows: Vec<(&str, Option<(&str, FlowOrigin)>)> = vec![
            ("review", Some(("repo", FlowOrigin::Repo))),
            ("review.yaml", Some(("repo", FlowOrigin::Repo))),
            ("lint", Some(("project lint", FlowOrigin::Project))),
            ("missing", None),
        ];
        for (name, want) in rows {
            let got = sources
                .locate(name)
                .map(|(path, origin)| (std::fs::read_to_string(path).unwrap(), origin));
            assert_eq!(
                got,
                want.map(|(text, origin)| (text.to_string(), origin)),
                "{name}"
            );
        }
        // Two spellings in two scopes: the same file the picker's listing
        // would name, whichever it is.
        write(&repo, "audit.yml", "repo audit");
        write(&sources.global, "audit.yaml", "global audit");
        let listed = sources
            .list_flows()
            .into_iter()
            .find(|found| crate::workspace::flow_paths::names_flow(&found.path, "audit"))
            .map(|found| found.path);
        assert_eq!(sources.locate("audit").map(|(path, _)| path), listed);

        let found = IncludeSource::find(&sources, "lint").expect("found");
        assert_eq!(found.text, "project lint");
        assert_eq!(found.dir, sources.project.clone().unwrap());
    }

    #[test]
    fn a_flow_with_an_include_loads_only_with_somewhere_to_look() {
        let root = tempfile::tempdir().unwrap();
        let sources = FlowSources {
            lane: root.path().join("lane"),
            project: None,
            global: root.path().join("global"),
        };
        write(
            &sources.global,
            "steps.yaml",
            "version: 1\nnodes:\n  - id: a\n    kind: command\n    run: \"true\"\n",
        );
        let text = "version: 1\nnodes:\n  - id: s\n    kind: include\n    flow: steps\n";
        let loaded = load_flow(text, None, Some(&sources)).expect("the include expands");
        assert_eq!(loaded.flow().nodes[0].id, "s--a");
        assert!(load_flow(text, None, None).is_err());
        assert!(inspect_flow(text, None, Some(&sources)).is_ok());
    }
}
