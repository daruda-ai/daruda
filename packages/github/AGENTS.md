# daruda_github

GitHub pull requests for lanes, through the `gh` CLI. GPUI-free; depends on
`daruda_core` only. Read this before reaching GitHub from any package.

## The gh boundary

Every GitHub request goes through `trait Gh`. Tests must not reach GitHub
with the developer's own login, the same reason app tests reach git only
through `GitHost`.

- App code reaches `gh` only through `crate::gh_host::GhHost`; test builds
  install an empty `FakeGh`, and tests script one with
  `test_support::fake_gh`. A `--screenshot` launch installs one before the
  PR poller starts (`startup::inspection::before_globals`). `scripts/lint-real-gh-tests.sh` rejects
  `SystemGh` or a `gh` child anywhere else under `packages/app/src`.
- `Gh` methods are whole questions (`prs_for_branches`, `failed_log`, ...),
  never a passthrough to `gh api`, so a fake answers each one directly.
  A new question adds a method to `Gh`, `SystemGh` and `FakeGh` together.
- `SystemGh` runs through `daruda_core::process::run_captured` with argv only,
  `GH_PROMPT_DISABLED=1`, and `-f` for every GraphQL string variable (`-F`
  would turn a branch named like a number into one).
- Parsers are tested against recorded `gh` output in `tests/fixtures/`.
  Record a new fixture with the exact query the code sends.

## Rules that live here

- `checks::summarize` is the only place that decides what checks add up to.
- `link::pick` decides which PR a branch belongs to: same repository only;
  a lane on the default branch has none. A fork PR's review branch is not
  looked up by name: it carries a `link::ReviewMark` (the one format of the
  mark, kept in the branch's git config), and the poller asks for its PR by
  number in the same per-repository request. Such a lane never reads as
  having no PR, so it is never offered one to create. A number GitHub cannot
  resolve fails only that alias (`NOT_FOUND`; `gh` exits non-zero but
  prints the partial answer), so it never pauses the repository's poll.
- `PrDetail`'s `is_draft`, `merge_block`, `can_merge`, `merge_menu`,
  `can_close_now`, `can_reopen_now` and `failed_runs` (`actions.rs`) decide what the detail may ask of GitHub;
  the app maps their answers to labels and buttons and never re-derives them.
- `PrCheckout::of` decides how a pull request's head reaches a worktree:
  a same-repository PR's own branch, or a fork's `pr-<N>` review branch
  fetched from `refs/pull/<N>/head` and marked with its `ReviewMark`.
  The app runs the git it names.
- `prompt` builds text for agents from outside input (CI logs, review
  comments): every piece goes through `strip_control` (`one_line` for a
  field a header holds) and is fenced or quoted so it cannot close its
  quote or end a bracketed paste. The app sends it only to writable agent
  chats, inserted and never submitted.
- Mutations (`merge_pr`, `set_ready`, `close_pr`, `reopen_pr`, `rerun_failed`,
  `reply_to_thread`, `set_thread_resolved`, `edit_pr_body`,
  `edit_pr_reviewers`, `submit_review`) run only on the
  user's action and are never retried on their own; a failed thread reply is
  reconciled against a fresh thread read before it can be posted again, and a
  description changed on GitHub since editing began is overwritten only when
  asked. Reply and description text go to `gh` on stdin (`--input -`,
  `--body-file -`), not as arguments, which every OS caps. A review is
  submitted at the head commit the user reviewed (`commitOID`); when its
  answer is lost, `viewer_latest_review` (read before and after) decides
  whether it went through, and an outcome that cannot be read is reported
  unknown rather than offered for another submit. Reviewer names
  pass `reviewer_name_ok` and are joined with `=` before reaching `gh`.
- `pr_conversation` reads the pull request's own comments, 100 per page up
  to 10 pages; past that it is marked `truncated`. A merge is pinned with `--match-head-commit` to the
  head the user saw and never passes `--delete-branch`, `--auto` or
  `--admin`. A mutation's error is reported to the user who asked; it is
  not a poll failure and does not feed the poller's host state.
- `auth` answers the active account and where gh keeps its token
  (`CredentialSource`), parsed from `gh auth status` output, which gh
  writes to stdout or stderr. The token itself is never read or kept.
- A rate-limit pause starts on the poller's estimate (`Resume::Estimated`).
  The poller then asks `graphql_rate_limit_reset` once for that pause;
  `gh api rate_limit` does not draw on the limit (measured with gh 2.74.2).
  A reported reset replaces only a running estimated pause
  (`Resume::Reported`); an unreported or implausible one leaves the estimate,
  and the page says which of the two it shows.
- Polling (`app/src/github/poller.rs`) is process-wide and asks once per
  repository per round, however many windows show its lanes. Its timing,
  backoff and auth-recheck rules are GPUI-free in `app/src/github/schedule.rs`.
- The pull request list (`app/src/github/list.rs`) runs from the same tick:
  one paged search (`search_prs_page`) per (host, audience, state) per round
  while a window's page is shown, each 20-repository chunk with its own
  cursor. All three audiences supply tab counts through that same cache and
  cadence; selecting a tab reuses its answer. A later page is read only on
  request and appended without repeats; while one is shown the cadence
  leaves the list alone, and a refresh reads it from the first page. Each
  list has an epoch: a page that answers after its list was dropped or
  replaced is discarded. Lists are dropped when the host's `gh` account
  changes. A GitHub search (`app/src/github/refine.rs`: words and typed
  `key:value` facets read from the page's field once typing pauses) is its
  own key, sent for the shown tab alone; tab counts stay on the bare tabs,
  and a typed key nobody shows is let go. A facet that would repeat the
  tab's own identity qualifier is not sent. Only hosts the poller's state
  lets through are asked; host-wide
  failures feed that same host state. Timing is GPUI-free in
  `app/src/github/list_schedule.rs`.
- `pr_files_page` reads one page of changed files (`PR_FILES_PER_PAGE`, 100)
  with plain `gh api ... ?per_page=100&page=N`. A full page leads to the next;
  a full page 30 is `capped` at GitHub's `PR_FILES_MAX` (3,000). A missing
  `patch` stays `None` (binary or too large); an empty one stays `Some("")`
  (no line changed). The app reads pages in turn and keeps what arrived.
  Each file's `blob` is the listing's `sha`, the content's blob id.
- `merge_base` reads `compare/{base}...{head}?per_page=1` with
  `--jq .merge_base_commit.sha` (the answer still carries the comparison's
  files; only the commit is captured) for the commit a pull request's diff
  starts from; `file_at` reads raw content
  (`Accept: application/vnd.github.raw`) at a commit, `Absent` on HTTP 404
  and `Binary` for a NUL byte or non-UTF-8. Path and ref segments are
  percent-encoded. The app uses them to diff a file the listing sent no
  patch for, on request only; output above the capture limit is a
  too-large failure.
- `search_prs_page` is the history/search boundary the page lists through;
  `search_prs` remains for compatibility. Each immutable request
  covers one host and up to 20 canonical repositories, with 1–50 results.
  Its continuation retains criteria, chunk and cursor history; a new refresh
  starts a new request. The caller owns account/generation and refresh epochs,
  cross-page deduplication, chunk failures and cache lifetime.
  `PrSearchContinuation` distinguishes more pages, exhausted coverage, the
  1,000-result search cap and detected omissions. Counts describe matches
  visible to the account, never proof that an inaccessible resource is absent.
  Criteria use typed qualifiers and literal words/phrases (AND); quotes,
  backslashes and controls are rejected rather than interpreted as raw syntax.
  `commit_status` is GitHub's search qualifier, not `checks::summarize`.
  Unlike the active-work legacy search, history includes archived repositories.
