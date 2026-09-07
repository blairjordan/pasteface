# Contributing

Read [AGENTS.md](AGENTS.md) for architecture constraints and verification expectations.

Enable the Conventional Commits hook after cloning:

```sh
git config core.hooksPath .githooks
```

Use messages such as `feat: add transcript tabs`, `fix(audio): retain interrupted recordings`, or `docs: improve installation instructions`.

Before submitting a change, run:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo check --no-default-features
```

Keep changes focused and explain their behavior and validation. Tests must use isolated fixtures; do not add recordings or API credentials to the repository.
