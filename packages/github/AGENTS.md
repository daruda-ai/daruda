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
  a lane on the default branch has none.
- `prompt` builds text for agents from outside input (CI logs, review
  comments): every piece goes through `strip_control` (`one_line` for a
  field a header holds) and is fenced or quoted so it cannot close its
  quote or end a bracketed paste. The app sends it only to writable agent
  chats, inserted and never submitted.
- Polling (`app/src/github/poller.rs`) is process-wide and asks once per
  repository per round, however many windows show its lanes.
