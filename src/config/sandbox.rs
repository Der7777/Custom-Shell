use std::env;

use crate::execution::SandboxConfig;
use crate::parse::parse_line;

pub(crate) fn apply_sandbox_env(sandbox: &mut SandboxConfig) {
    if let Ok(path) = env::var("MINISHELL_BWRAP_PATH") {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            sandbox.bubblewrap_path = None;
        } else {
            sandbox.bubblewrap_path = Some(trimmed.to_string());
        }
    }
    if let Ok(args) = env::var("MINISHELL_BWRAP_ARGS") {
        let trimmed = args.trim();
        if trimmed.is_empty() {
            sandbox.bubblewrap_args.clear();
        } else {
            match parse_line(trimmed) {
                Ok(tokens) => sandbox.bubblewrap_args = tokens,
                Err(err) => {
                    eprintln!("config error: invalid MINISHELL_BWRAP_ARGS: {err}");
                }
            }
        }
    }
}
