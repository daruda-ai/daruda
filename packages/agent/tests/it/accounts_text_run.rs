use daruda_agent::accounts::text_run::text_only_args;
use daruda_store::accounts::AccountRecipeId;

/// A domain whose switches were never measured has no run at all:
/// guessing them would run an agent with its tools.
#[test]
fn only_a_measured_domain_has_a_text_run() {
    let claude = text_only_args(AccountRecipeId::Claude).expect("measured");
    assert!(claude.contains(&"--tools="), "no built-in tools");
    assert!(!claude.contains(&""), "no empty word a shim could drop");
    assert!(claude.contains(&"--strict-mcp-config"), "no MCP tools");
    assert_eq!(text_only_args(AccountRecipeId::Codex), None);
}

#[cfg(unix)]
mod run {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use daruda_agent::accounts::CliCommand;
    use daruda_agent::accounts::text_run::{TextRunError, run_text};

    fn sh(script: &str) -> CliCommand {
        CliCommand {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            env: Vec::new(),
        }
    }

    #[test]
    fn the_prompt_arrives_on_stdin_and_the_answer_is_what_was_printed() {
        let answer = run_text(&sh("cat"), "가 prompt".into(), &AtomicBool::new(false)).unwrap();
        assert_eq!(answer, "가 prompt");
    }

    /// Not the caller's directory, and nothing in it: a CLI that ignored
    /// its switches would still have no repository in reach.
    #[test]
    fn the_run_has_an_empty_directory_of_its_own() {
        let listing = run_text(&sh("pwd; ls -A"), String::new(), &AtomicBool::new(false)).unwrap();
        let mut lines = listing.lines();
        let ran_in = std::path::PathBuf::from(lines.next().expect("a directory"));
        assert_eq!(lines.next(), None, "empty: {listing}");
        let here = std::env::current_dir().unwrap();
        assert_ne!(ran_in.canonicalize().ok(), here.canonicalize().ok());
        assert!(!ran_in.exists(), "removed after the run");
    }

    #[test]
    fn a_failing_cli_reports_what_it_said() {
        let error = run_text(
            &sh("echo 'Not logged in'; exit 1"),
            String::new(),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        match error {
            TextRunError::Exit { reason, .. } => assert_eq!(reason, "Not logged in"),
            other => panic!("{other}"),
        }
    }

    #[test]
    fn a_run_nobody_waits_on_ends_when_cancelled() {
        let cancelled = std::sync::Arc::new(AtomicBool::new(false));
        let raise = cancelled.clone();
        let raiser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            raise.store(true, Ordering::Relaxed);
        });
        let started = Instant::now();
        let outcome = run_text(&sh("sleep 30"), String::new(), &cancelled);
        raiser.join().unwrap();
        assert!(
            matches!(outcome, Err(TextRunError::Cancelled)),
            "{outcome:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
