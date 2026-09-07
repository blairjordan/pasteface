# Repository guidance

Read this file before changing Pasteface. It applies to Codex, Claude, and other coding agents.

## Product

Pasteface records and transcribes speech from the terminal on Linux and macOS. Keep the interface compact and usable with both keyboard and mouse. Keep transcript text free of decorative side borders so terminal selection stays useful.

Recording belongs to the background service. Closing a UI must not stop capture. Transcription runs independently of capture; completed chunks append in recording order. Never discard transcript or audio without an explicit clear action. Cancel must stop active and queued transcription without deleting recordings or stopping live capture.

Backends are runtime settings. Local Whisper is an external executable; do not link machine-specific GPU requirements into the application. Cloud transcription must be explicitly selected. Keep API keys out of public state, logs, fixtures, screenshots, and source control.

## Code quality

- Prefer small, direct Rust functions and explicit types. Reuse existing abstractions before adding dependencies or architectural layers.
- Keep terminal rendering, input handling, capture, service state, and provider transport separate.
- Keep blocking audio, process, and network work off the UI thread.
- Preserve persisted-state compatibility with serde defaults or an explicit migration.
- Handle errors with actionable messages. Avoid panics in production paths.
- Use the same rendered geometry for mouse hit targets and controls. Modal input must not trigger underlying controls.
- Preserve Linux and macOS support; isolate platform-specific code with cfg.
- Use isolated fixtures and mocked network responses for tests. Do not use personal recordings, credentials, or paid API calls for verification without authorization.
- Run formatting and lint checks. Run relevant existing tests for the change; add tests for behavior, regressions, and concurrency boundaries rather than implementation details or trivial visual edits.
- Report what changed, how it was verified, and any remaining limitation. Do not claim a feature is installed or a CI job passed without checking.

## Git and commits

**Always use Conventional Commits**, including the initial commit:

    feat: add transcript tabs
    fix(audio): retain capture during queue cancellation
    docs: explain transcription providers

Allowed types: feat, fix, docs, style, refactor, perf, test, build, ci, chore, revert. A scope and breaking-change marker are optional. Use a concise imperative description.

Enable the repository commit-message hook with:

    git config core.hooksPath .githooks

Do not bypass the hook. Keep unrelated work out of a commit. Never include secrets or generated recording data. Do not rewrite published history unless the user explicitly authorizes that specific rewrite. Preserve a recovery bundle before an authorized rewrite.

## Checks

    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    cargo test
    cargo check --no-default-features

CI runs these checks on Linux and macOS. Socket tests need an environment that permits local Unix and TCP sockets; sandbox denials are not evidence of broken application behavior.

If a .codegraph directory exists, use CodeGraph before searching or reading code to locate symbols. Do not create an index without the user's request.
