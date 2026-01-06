# custom_shell

A minimal Rust shell with job control, pipelines, redirection, and a small scripting layer.

## Build

```
cargo build --bins
```

## Library API (parse + expansion)

The crate exports a minimal parser/expansion API for fuzzing and unit tests:

- `parse_tokens`
- `parse_sequence`
- `parse_pipeline`

When built with the `expansion` feature, it also exports:

- `expand_tokens`
- `expand_token`
- `expand_globs`
- `glob_pattern`

## Test

```
cargo test
```

## Config (`~/.minishellrc`)

Supported directives:

- `alias ll='ls -la'`
- `export VAR=value`
- `prompt = {cwd} $ `

Notes:
- `prompt` supports `{cwd}`, `{status}`, and `{status?}`.
- Set `MINISHELL_EDITMODE=vi` in your environment to enable vi mode for line editing.
- Set `MINISHELL_LOG=debug` (or `RUST_LOG`) to control log verbosity.

## Fuzz (optional)

```
cargo fuzz run parser
```

## Security notes

By default, this shell does not sandbox execution. Do not run untrusted scripts or binaries.
The optional `sandbox` feature lets you run commands with `sandbox=yes` or `--sandbox`, but it is not a security boundary unless configured correctly.
For isolation, run inside a container/VM or wrap with OS-level sandboxes (e.g., seccomp, namespaces, chroot), and consider dropping privileges before executing commands.
Command substitution runs with the full environment and privileges of the shell unless sandboxing is enabled.

## CI

GitHub Actions runs `cargo fmt --check`, `cargo clippy`, and `cargo test` on push/PR.

Also if your reading this your probably a nerd, like just figure it out
