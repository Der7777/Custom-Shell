use std::io;
use std::sync::Arc;

use crate::colors::{apply_color_setting, format_color_lines, resolve_color, save_colors};
use crate::completions::{
    apply_completion_tokens, format_completion_lines, save_completion_file, suggest_command,
};
use crate::config::{format_abbreviation_line, save_abbreviations};
use crate::execution::{
    build_command, run_command_in_foreground, sandbox_options_for_command, status_from_error,
};
use crate::expansion::{expand_globs, expand_tokens};
use crate::io_helpers::read_input_line;
use crate::job_control::{
    add_job_with_status, bring_job_foreground, continue_job, find_job, list_jobs, parse_job_id,
    take_job, JobStatus, WaitOutcome,
};
use crate::parse::split_sequence;
use crate::parse::{parse_line, token_str, CommandSpec, SeqOp, OPERATOR_TOKEN_MARKER};
use crate::utils::is_valid_var_name;
use crate::{build_expansion_context, execute_segment, trace_tokens, ShellState};
use glob::Pattern;

pub fn is_builtin(cmd: Option<&str>) -> bool {
    matches!(
        cmd,
        Some(
            "exit"
                | "cd"
                | "pwd"
                | "jobs"
                | "fg"
                | "bg"
                | "help"
                | "abbr"
                | "complete"
                | "set_color"
                | "fish_config"
                | "source"
                | "history"
        )
    )
}

pub fn try_execute_compound(
    state: &mut ShellState,
    tokens: &[String],
    display: &str,
) -> io::Result<Option<bool>> {
    if is_if_start(tokens) {
        let tokens = read_compound_tokens(state, tokens.to_vec(), CompoundKind::If)?;
        execute_if(state, tokens, display)?;
        return Ok(Some(true));
    }
    if is_while_start(tokens) {
        let tokens = read_compound_tokens(state, tokens.to_vec(), CompoundKind::While)?;
        execute_while(state, tokens, display)?;
        return Ok(Some(true));
    }
    if is_for_start(tokens) {
        let tokens = read_compound_tokens(state, tokens.to_vec(), CompoundKind::For)?;
        execute_for(state, tokens, display)?;
        return Ok(Some(true));
    }
    if is_case_start(tokens) {
        let tokens = read_compound_tokens(state, tokens.to_vec(), CompoundKind::Case)?;
        execute_case(state, tokens, display)?;
        return Ok(Some(true));
    }
    if is_function_def_start(tokens) {
        let tokens = read_compound_tokens(state, tokens.to_vec(), CompoundKind::Function)?;
        define_function(state, tokens)?;
        return Ok(Some(true));
    }
    Ok(Some(false))
}

pub fn execute_script_tokens(state: &mut ShellState, tokens: Vec<String>) -> io::Result<()> {
    let ctx = build_expansion_context(
        Arc::clone(&state.fg_pgid),
        state.trace,
        state.sandbox.clone(),
        &[],
    );
    let expanded = match expand_tokens(tokens, &ctx) {
        Ok(v) => v,
        Err(msg) => {
            state.last_status = 2;
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("parse error: {msg}"),
            ));
        }
    };
    trace_tokens(state, "expanded tokens", &expanded);

    if expanded.is_empty() {
        return Ok(());
    }

    let expanded = match expand_globs(expanded) {
        Ok(v) => v,
        Err(msg) => {
            state.last_status = 2;
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("parse error: {msg}"),
            ));
        }
    };
    trace_tokens(state, "globbed tokens", &expanded);

    if expanded.is_empty() {
        return Ok(());
    }

    let segments = match split_sequence(expanded) {
        Ok(v) => v,
        Err(msg) => {
            state.last_status = 2;
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("parse error: {msg}"),
            ));
        }
    };

    for segment in segments {
        let should_run = match segment.op {
            SeqOp::Always => true,
            SeqOp::And => state.last_status == 0,
            SeqOp::Or => state.last_status != 0,
        };
        if should_run {
            execute_segment(state, segment.tokens, &segment.display)?;
        }
    }

    Ok(())
}

pub fn execute_function(state: &mut ShellState, func_tokens: Vec<String>, args: &[String]) -> io::Result<()> {
    let ctx = build_expansion_context(
        Arc::clone(&state.fg_pgid),
        state.trace,
        state.sandbox.clone(),
        args,
    );
    let expanded = match expand_tokens(func_tokens, &ctx) {
        Ok(v) => v,
        Err(msg) => {
            state.last_status = 2;
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("parse error: {msg}"),
            ));
        }
    };
    trace_tokens(state, "function expanded tokens", &expanded);

    if expanded.is_empty() {
        return Ok(());
    }

    let expanded = match expand_globs(expanded) {
        Ok(v) => v,
        Err(msg) => {
            state.last_status = 2;
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("parse error: {msg}"),
            ));
        }
    };
    trace_tokens(state, "function globbed tokens", &expanded);

    if expanded.is_empty() {
        return Ok(());
    }

    let segments = match split_sequence(expanded) {
        Ok(v) => v,
        Err(msg) => {
            state.last_status = 2;
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("parse error: {msg}"),
            ));
        }
    };

    for segment in segments {
        let should_run = match segment.op {
            SeqOp::Always => true,
            SeqOp::And => state.last_status == 0,
            SeqOp::Or => state.last_status != 0,
        };
        if should_run {
            execute_segment(state, segment.tokens, &segment.display)?;
        }
    }

    Ok(())
}

pub fn execute_builtin(state: &mut ShellState, cmd: &CommandSpec, display: &str) -> io::Result<()> {
    let args = &cmd.args;
    let name = args.first().map(String::as_str);
    match name {
        Some("exit") => {
            let code = args
                .get(1)
                .and_then(|s| s.parse::<i32>().ok())
                .unwrap_or(state.last_status);
            std::process::exit(code);
        }
        Some("cd") => {
            let target = args.get(1).map(String::as_str).unwrap_or("~");
            let expanded = if let Some(rest) = target.strip_prefix('~') {
                if let Ok(home) = std::env::var("HOME") {
                    format!("{home}{rest}")
                } else {
                    target.to_string()
                }
            } else {
                target.to_string()
            };
            if let Err(err) = std::env::set_current_dir(&expanded) {
                eprintln!("cd: {err}");
                state.last_status = 1;
            } else {
                state.last_status = 0;
            }
        }
        Some("pwd") => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| "/".into());
            println!("{}", cwd.display());
            state.last_status = 0;
        }
        Some("jobs") => {
            list_jobs(&state.jobs);
            state.last_status = 0;
        }
        Some("fg") => {
            let job_id = parse_job_id(args.get(1))?;
            let job = match take_job(&mut state.jobs, job_id) {
                Some(job) => job,
                None => {
                    eprintln!("fg: no such job");
                    state.last_status = 1;
                    return Ok(());
                }
            };
            match bring_job_foreground(job, &state.fg_pgid, state.shell_pgid) {
                Ok(result) => {
                    if let Some(stopped) = result.stopped_job {
                        state.jobs.push(stopped);
                    }
                    state.last_status = result.status_code.unwrap_or(0);
                }
                Err(err) => {
                    eprintln!("fg: {err}");
                    state.last_status = 1;
                }
            }
        }
        Some("bg") => {
            let job_id = parse_job_id(args.get(1))?;
            let job = match find_job(&mut state.jobs, job_id) {
                Some(job) => job,
                None => {
                    eprintln!("bg: no such job");
                    state.last_status = 1;
                    return Ok(());
                }
            };
            if let Err(err) = continue_job(job.pgid) {
                eprintln!("bg: {err}");
                state.last_status = 1;
            } else {
                job.status = JobStatus::Running;
                println!("[{}] Running {}", job.id, job.command);
                state.last_status = 0;
            }
        }
        Some("help") => {
            if args.len() > 1 {
                let topic = &args[1];
                match std::process::Command::new("man").arg(topic).status() {
                    Ok(status) => {
                        state.last_status = if status.success() { 0 } else { 1 };
                    }
                    Err(err) if err.kind() == io::ErrorKind::NotFound => {
                        eprintln!("help: man not found");
                        state.last_status = 127;
                    }
                    Err(err) => {
                        eprintln!("help: {err}");
                        state.last_status = 1;
                    }
                }
                return Ok(());
            }
            println!(
                "Built-ins: cd [dir], pwd, jobs, fg [id], bg [id], help, exit [code], abbr, complete"
            );
            println!(
                "External commands support pipes with |, background jobs with &, and redirection with <, >, >>, 2>, 2>>, 2>&1, &>, &>>, and <<<."
            );
            println!("Config: ~/.minishellrc (aliases, env vars, prompt).");
            println!("Abbreviations: ~/.minishell_abbr (or abbr lines in ~/.minishellrc).");
            println!("Sandbox: prefix commands with sandbox=yes/no or use --sandbox/--no-sandbox.");
            println!("Completion: commands, filenames, $vars, %jobs.");
            println!("Completions: ~/.minishell_completions and ~/.config/fish/completions/.");
            println!("Prompt themes: fish (default), classic, minimal.");
            println!("Prompt function: set prompt_function = name in config.");
            println!("Colors: set_color key value (or ~/.minishell_colors).");
            println!(
                "Expansion order: quotes/escapes -> command substitution -> vars/tilde -> glob (no IFS splitting)."
            );
            state.last_status = 0;
        }
        Some("abbr") => {
            if args.len() == 1 {
                let mut entries: Vec<_> = state.abbreviations.iter().collect();
                entries.sort_by_key(|(name, _)| *name);
                for (name, tokens) in entries {
                    println!("{}", format_abbreviation_line(name, tokens));
                }
                state.last_status = 0;
                return Ok(());
            }
            if args[1] == "-e" || args[1] == "--erase" {
                let Some(name) = args.get(2) else {
                    eprintln!("abbr: missing name to erase");
                    state.last_status = 2;
                    return Ok(());
                };
                if state.abbreviations.remove(name).is_none() {
                    eprintln!("abbr: no such abbreviation '{name}'");
                    state.last_status = 1;
                    return Ok(());
                }
                if let Err(err) = save_abbreviations(&state.abbreviations) {
                    eprintln!("abbr: failed to save abbreviations: {err}");
                    state.last_status = 1;
                    return Ok(());
                }
                state.last_status = 0;
                return Ok(());
            }
            if args.len() < 3 {
                eprintln!("usage: abbr name expansion...");
                eprintln!("       abbr -e name");
                state.last_status = 2;
                return Ok(());
            }
            let name = &args[1];
            if !is_valid_var_name(name) {
                eprintln!("abbr: invalid name '{name}'");
                state.last_status = 2;
                return Ok(());
            }
            let expansion = args[2..].iter().cloned().collect::<Vec<_>>();
            state.abbreviations.insert(name.to_string(), expansion);
            if let Err(err) = save_abbreviations(&state.abbreviations) {
                eprintln!("abbr: failed to save abbreviations: {err}");
                state.last_status = 1;
                return Ok(());
            }
            state.last_status = 0;
        }
        Some("complete") => {
            if args.len() == 1 {
                for line in format_completion_lines(&state.completions) {
                    println!("{line}");
                }
                state.last_status = 0;
                return Ok(());
            }
            match apply_completion_tokens(args, &mut state.completions) {
                Ok(()) => {
                    if let Err(err) = save_completion_file(&state.completions) {
                        eprintln!("complete: failed to save completions: {err}");
                        state.last_status = 1;
                        return Ok(());
                    }
                    state.last_status = 0;
                }
                Err(err) => {
                    eprintln!("{err}");
                    eprintln!("usage: complete -c cmd -a 'items...'");
                    eprintln!("       complete -c cmd -x 'script'");
                    eprintln!("       complete -c cmd -r");
                    state.last_status = 2;
                }
            }
        }
        Some("set_color") => {
            if args.len() == 1 {
                for line in format_color_lines(&state.colors) {
                    println!("{line}");
                }
                state.last_status = 0;
                return Ok(());
            }
            if args.len() < 3 {
                eprintln!("usage: set_color key value");
                eprintln!("       set_color");
                state.last_status = 2;
                return Ok(());
            }
            let key = args[1].trim().trim_start_matches("color.");
            let value = args[2..].join(" ");
            match apply_color_setting(&mut state.colors, key, value.trim()) {
                Ok(()) => {
                    if let Err(err) = save_colors(&state.colors) {
                        eprintln!("set_color: failed to save colors: {err}");
                        state.last_status = 1;
                        return Ok(());
                    }
                    state.last_status = 0;
                }
                Err(err) => {
                    eprintln!("set_color: {err}");
                    state.last_status = 2;
                }
            }
        }
        Some("fish_config") => {
            println!("Custom shell config (TUI placeholder).");
            println!("Current colors:");
            for line in format_color_lines(&state.colors) {
                let mut parts = line.splitn(2, '=');
                let key = parts.next().unwrap_or_default();
                let value = parts.next().unwrap_or_default();
                let color = resolve_color(value);
                if color.is_empty() {
                    println!("{key}={value}");
                } else {
                    println!("{key}={color}{value}\x1b[0m");
                }
            }
            println!("Use: set_color key value");
            println!("Keys: prompt_status, prompt_cwd, prompt_git, prompt_symbol, hint");
            state.last_status = 0;
        }
        Some("source") => {
            if let Some(file) = args.get(1) {
                match std::fs::read_to_string(file) {
                    Ok(content) => {
                        let tokens = match parse_line(&content) {
                            Ok(t) => t,
                            Err(msg) => {
                                eprintln!("parse error: {msg}");
                                state.last_status = 2;
                                return Ok(());
                            }
                        };
                        execute_script_tokens(state, tokens)?;
                    }
                    Err(err) => {
                        eprintln!("source: {err}");
                        state.last_status = 1;
                    }
                }
            } else {
                eprintln!("source: missing file");
                state.last_status = 2;
            }
        }
        Some("history") => {
            if let Some(count_str) = args.get(1) {
                if let Ok(count) = count_str.parse::<usize>() {
                    for i in (state.editor.history().len().saturating_sub(count)..state.editor.history().len()).rev() {
                        if let Some(entry) = state.editor.history().get(i) {
                            println!("{} {}", i, entry);
                        }
                    }
                } else {
                    eprintln!("history: invalid number");
                    state.last_status = 2;
                }
            } else {
                for (i, entry) in state.editor.history().iter().enumerate() {
                    println!("{} {}", i, entry);
                }
            }
            state.last_status = 0;
        }
        Some("set") => {
            if args.len() >= 3 && args[1] == "-o" && args[2] == "pipefail" {
                state.pipefail = true;
                state.last_status = 0;
            } else if args.len() >= 3 && args[1] == "+o" && args[2] == "pipefail" {
                state.pipefail = false;
                state.last_status = 0;
            } else if args.len() >= 2 && args[1] == "-x" {
                state.trace = true;
                state.last_status = 0;
            } else if args.len() >= 2 && args[1] == "+x" {
                state.trace = false;
                state.last_status = 0;
            } else if args.len() == 1 {
                println!("pipefail\t{}", if state.pipefail { "on" } else { "off" });
                println!("xtrace\t{}", if state.trace { "on" } else { "off" });
                state.last_status = 0;
            } else {
                eprintln!("set: unsupported option");
                state.last_status = 2;
            }
        }
        Some(cmd_name) => {
            if let Some(body) = state.functions.get(cmd_name) {
                let body_tokens = body.clone();
                execute_script_tokens(state, body_tokens)?;
                return Ok(());
            }
            let mut command = build_command(cmd)?;
            let sandbox = sandbox_options_for_command(cmd, &state.sandbox, state.trace);
            match run_command_in_foreground(
                &mut command,
                &state.fg_pgid,
                state.shell_pgid,
                state.trace,
                sandbox,
            ) {
                Ok(result) => {
                    if matches!(result.outcome, WaitOutcome::Stopped) {
                        let job_id = add_job_with_status(
                            &mut state.jobs,
                            &mut state.next_job_id,
                            result.pgid,
                            result.last_pid,
                            1,
                            display,
                            JobStatus::Stopped,
                        );
                        println!("[{job_id}] Stopped {display}");
                        state.last_status = 128 + libc::SIGTSTP;
                    } else {
                        let last = result.status_code.unwrap_or(0);
                        let pipefail = result.pipefail_status.unwrap_or(last);
                        state.last_status = if state.pipefail { pipefail } else { last };
                    }
                }
                Err(err) => {
                    eprintln!("{err}");
                    if err.kind() == io::ErrorKind::NotFound {
                        if let Some(suggestion) = suggest_command(
                            &cmd.args[0],
                            &state.aliases,
                            &state.functions,
                            &state.abbreviations,
                            &state.completions,
                        ) {
                            if suggestion != cmd.args[0] {
                                eprintln!("Command not found—did you mean '{suggestion}'?");
                            }
                        }
                    }
                    state.last_status = status_from_error(&err);
                }
            }
        }
        None => {
            state.last_status = 0;
        }
    }

    Ok(())
}

pub fn execute_builtin_substitution(pipeline: &[CommandSpec]) -> Result<(String, i32), String> {
    if pipeline.len() != 1 {
        return Err("pipes only work with external commands".to_string());
    }
    let args = &pipeline[0].args;
    match args.first().map(String::as_str) {
        Some("pwd") => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| "/".into());
            Ok((cwd.display().to_string(), 0))
        }
        Some("help") => Ok((
            "Built-ins: cd [dir], pwd, jobs, fg [id], bg [id], help, exit [code], abbr, complete"
                .to_string(),
            0,
        )),
        Some("cd") => Err("cd is not supported in command substitution".to_string()),
        Some("exit") => Err("exit is not supported in command substitution".to_string()),
        Some("abbr") => Err("abbr is not supported in command substitution".to_string()),
        Some("complete") => Err("complete is not supported in command substitution".to_string()),
        Some("jobs") | Some("fg") | Some("bg") => {
            Err("job control is not supported in command substitution".to_string())
        }
        _ => Err("built-ins are not supported in command substitution".to_string()),
    }
}

#[derive(Copy, Clone)]
enum CompoundKind {
    If,
    While,
    For,
    Case,
    Function,
}

fn is_if_start(tokens: &[String]) -> bool {
    tokens.first().map(String::as_str) == Some("if")
}

fn is_while_start(tokens: &[String]) -> bool {
    tokens.first().map(String::as_str) == Some("while")
}

fn is_for_start(tokens: &[String]) -> bool {
    tokens.first().map(String::as_str) == Some("for")
}

fn is_case_start(tokens: &[String]) -> bool {
    tokens.first().map(String::as_str) == Some("case")
}

fn is_function_def_start(tokens: &[String]) -> bool {
    if tokens.len() >= 2 && tokens[0] == "function" {
        return is_valid_var_name(&tokens[1]);
    }
    if tokens.len() >= 3 && tokens[1] == "()" {
        return is_valid_var_name(&tokens[0]);
    }
    if tokens.len() >= 2 && tokens[0] == "function" {
        return is_valid_var_name(&tokens[1]) && tokens.contains(&"{".to_string());
    }
    false
}

fn read_compound_tokens(
    state: &mut ShellState,
    mut tokens: Vec<String>,
    kind: CompoundKind,
) -> io::Result<Vec<String>> {
    while needs_more_compound(&tokens, kind) {
        let line = match read_input_line(&mut state.editor, state.interactive, "> ")? {
            Some(line) => line,
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "unexpected EOF",
                ));
            }
        };
        let more = parse_line(line.trim_end()).map_err(|err| {
            io::Error::new(io::ErrorKind::InvalidInput, format!("parse error: {err}"))
        })?;
        if !more.is_empty() {
            tokens.push(format!("{OPERATOR_TOKEN_MARKER};"));
            tokens.extend(more);
        }
    }
    Ok(tokens)
}

fn needs_more_compound(tokens: &[String], kind: CompoundKind) -> bool {
    let mut if_count = 0i32;
    let mut while_count = 0i32;
    let mut for_count = 0i32;
    let mut case_count = 0i32;
    let mut brace_count = 0i32;
    for token in tokens {
        let t = token_str(token);
        match t {
            "if" => if_count += 1,
            "fi" => if_count -= 1,
            "while" => while_count += 1,
            "done" => {
                while_count -= 1;
                for_count -= 1;
            }
            "for" => for_count += 1,
            "case" => case_count += 1,
            "esac" => case_count -= 1,
            "{" => brace_count += 1,
            "}" => brace_count -= 1,
            _ => {}
        }
    }
    match kind {
        CompoundKind::If => if_count > 0,
        CompoundKind::While => while_count > 0,
        CompoundKind::For => for_count > 0,
        CompoundKind::Case => case_count > 0,
        CompoundKind::Function => brace_count > 0,
    }
}

fn execute_if(state: &mut ShellState, tokens: Vec<String>, display: &str) -> io::Result<()> {
    let (cond_tokens, then_tokens, else_tokens) = parse_if_tokens(tokens)?;
    execute_script_tokens(state, cond_tokens)?;
    if state.last_status == 0 {
        execute_script_tokens(state, then_tokens)?;
    } else if let Some(tokens) = else_tokens {
        execute_script_tokens(state, tokens)?;
    }
    trace_tokens(state, "if display", &[display.to_string()]);
    Ok(())
}

fn execute_while(state: &mut ShellState, tokens: Vec<String>, _display: &str) -> io::Result<()> {
    let (cond_tokens, body_tokens) = parse_while_tokens(tokens)?;
    loop {
        execute_script_tokens(state, cond_tokens.clone())?;
        if state.last_status != 0 {
            break;
        }
        execute_script_tokens(state, body_tokens.clone())?;
    }
    Ok(())
}

fn execute_for(state: &mut ShellState, tokens: Vec<String>, _display: &str) -> io::Result<()> {
    let (var, list_tokens, body_tokens) = parse_for_tokens(tokens)?;
    let ctx = build_expansion_context(
        Arc::clone(&state.fg_pgid),
        state.trace,
        state.sandbox.clone(),
        &[],
    );
    let list_expanded = expand_tokens(list_tokens, &ctx)?;
    let list = expand_globs(list_expanded)?;
    for item in list {
        std::env::set_var(&var, item);
        execute_script_tokens(state, body_tokens.clone())?;
    }
    Ok(())
}

struct CaseClause {
    patterns: Vec<Vec<String>>,
    body: Vec<String>,
}

fn execute_case(state: &mut ShellState, tokens: Vec<String>, display: &str) -> io::Result<()> {
    let (word_tokens, clauses) = parse_case_tokens(tokens)?;
    let ctx = build_expansion_context(
        Arc::clone(&state.fg_pgid),
        state.trace,
        state.sandbox.clone(),
        &[],
    );
    let word_expanded = match expand_tokens(word_tokens, &ctx) {
        Ok(v) => v,
        Err(msg) => {
            state.last_status = 2;
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("parse error: {msg}"),
            ));
        }
    };
    let word = word_expanded.join(" ");

    for clause in clauses {
        let mut matched = false;
        for pattern_tokens in clause.patterns {
            let pattern_expanded = match expand_tokens(pattern_tokens, &ctx) {
                Ok(v) => v,
                Err(msg) => {
                    state.last_status = 2;
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("parse error: {msg}"),
                    ));
                }
            };
            let pattern = pattern_expanded.join(" ");
            let is_match = Pattern::new(&pattern)
                .map(|pat| pat.matches(&word))
                .unwrap_or_else(|_| pattern == word);
            if is_match {
                matched = true;
                break;
            }
        }
        if matched {
            execute_script_tokens(state, clause.body)?;
            trace_tokens(state, "case display", &[display.to_string()]);
            return Ok(());
        }
    }

    state.last_status = 0;
    trace_tokens(state, "case display", &[display.to_string()]);
    Ok(())
}

fn define_function(state: &mut ShellState, tokens: Vec<String>) -> io::Result<()> {
    let (name, body_tokens) = parse_function_tokens(tokens)?;
    state.functions.insert(name, body_tokens);
    state.last_status = 0;
    Ok(())
}

type IfParts = (Vec<String>, Vec<String>, Option<Vec<String>>);

fn parse_if_tokens(tokens: Vec<String>) -> io::Result<IfParts> {
    let iter = tokens.into_iter().peekable();
    let mut condition = Vec::new();
    let mut then_body = Vec::new();
    let mut else_body = None;
    let mut stage = "if";
    for token in iter {
        let t = token_str(&token).to_string();
        match stage {
            "if" => {
                if t == "then" {
                    stage = "then";
                } else if t == "if" {
                    continue;
                } else {
                    condition.push(token);
                }
            }
            "then" => {
                if t == "else" {
                    stage = "else";
                    else_body = Some(Vec::new());
                } else if t == "fi" {
                    break;
                } else {
                    then_body.push(token);
                }
            }
            "else" => {
                if t == "fi" {
                    break;
                }
                if let Some(ref mut body) = else_body {
                    body.push(token);
                }
            }
            _ => {}
        }
    }
    if condition.is_empty() || then_body.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid if statement",
        ));
    }
    Ok((condition, then_body, else_body))
}

fn parse_while_tokens(tokens: Vec<String>) -> io::Result<(Vec<String>, Vec<String>)> {
    let iter = tokens.into_iter().peekable();
    let mut condition = Vec::new();
    let mut body = Vec::new();
    let mut stage = "while";
    for token in iter {
        let t = token_str(&token).to_string();
        match stage {
            "while" => {
                if t == "do" {
                    stage = "do";
                } else if t == "while" {
                    continue;
                } else {
                    condition.push(token);
                }
            }
            "do" => {
                if t == "done" {
                    break;
                } else {
                    body.push(token);
                }
            }
            _ => {}
        }
    }
    if condition.is_empty() || body.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid while statement",
        ));
    }
    Ok((condition, body))
}

fn parse_for_tokens(tokens: Vec<String>) -> io::Result<(String, Vec<String>, Vec<String>)> {
    let mut iter = tokens.into_iter();
    let mut var = String::new();
    let mut list = Vec::new();
    let mut body = Vec::new();
    let mut stage = "for";
    while let Some(token) = iter.next() {
        let t = token_str(&token).to_string();
        match stage {
            "for" => {
                if t == "for" {
                    continue;
                } else {
                    var = token;
                    stage = "in";
                }
            }
            "in" => {
                if t == "in" {
                    stage = "list";
                } else {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "expected 'in' after for variable",
                    ));
                }
            }
            "list" => {
                if t == ";" || t == "do" {
                    if t == "do" {
                        stage = "do";
                    }
                    break;
                } else {
                    list.push(token);
                }
            }
            _ => {}
        }
    }
    if stage == "do" {
        for token in iter {
            let t = token_str(&token).to_string();
            if t == "done" {
                break;
            } else {
                body.push(token);
            }
        }
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected 'do' in for statement",
        ));
    }
    if var.is_empty() || body.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid for statement",
        ));
    }
    Ok((var, list, body))
}

fn parse_case_tokens(tokens: Vec<String>) -> io::Result<(Vec<String>, Vec<CaseClause>)> {
    if tokens.len() < 4 || tokens[0] != "case" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid case statement",
        ));
    }
    let mut idx = 1usize;
    let mut word_tokens = Vec::new();
    while idx < tokens.len() {
        let t = token_str(&tokens[idx]);
        if t == "in" {
            idx += 1;
            break;
        }
        word_tokens.push(tokens[idx].clone());
        idx += 1;
    }
    if word_tokens.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid case statement",
        ));
    }

    let mut clauses = Vec::new();
    let mut saw_esac = false;
    while idx < tokens.len() {
        let t = token_str(&tokens[idx]);
        if t == "esac" {
            saw_esac = true;
            break;
        }

        let mut patterns = Vec::new();
        let mut current_pattern = Vec::new();
        let mut found_paren = false;
        while idx < tokens.len() {
            let raw = tokens[idx].clone();
            let raw_str = token_str(&raw);
            if raw_str == "|" {
                if current_pattern.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "invalid case statement",
                    ));
                }
                patterns.push(current_pattern);
                current_pattern = Vec::new();
                idx += 1;
                continue;
            }
            if raw_str == ")" || raw_str.ends_with(')') {
                let mut trimmed = raw.clone();
                if raw_str == ")" {
                    // No-op; pattern already collected.
                } else if trimmed.pop() != Some(')') {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "invalid case statement",
                    ));
                }
                if !trimmed.is_empty() {
                    current_pattern.push(trimmed);
                }
                if current_pattern.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "invalid case statement",
                    ));
                }
                patterns.push(current_pattern);
                idx += 1;
                found_paren = true;
                break;
            }
            current_pattern.push(raw);
            idx += 1;
        }
        if !found_paren {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid case statement",
            ));
        }

        let mut body = Vec::new();
        while idx < tokens.len() {
            let raw = tokens[idx].clone();
            let raw_str = token_str(&raw);
            if raw_str == "esac" {
                saw_esac = true;
                idx += 1;
                break;
            }
            if raw_str == ";"
                && idx + 1 < tokens.len()
                && token_str(&tokens[idx + 1]) == ";"
            {
                idx += 2;
                break;
            }
            body.push(raw);
            idx += 1;
        }
        if body.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid case statement",
            ));
        }
        clauses.push(CaseClause { patterns, body });

        if saw_esac {
            break;
        }
    }

    if !saw_esac {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid case statement",
        ));
    }

    Ok((word_tokens, clauses))
}

fn parse_function_tokens(tokens: Vec<String>) -> io::Result<(String, Vec<String>)> {
    if tokens.len() < 3 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "function missing",
        ));
    }
    let name = if tokens[0] == "function" {
        tokens
            .get(1)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "function name missing"))?
            .to_string()
    } else if tokens[1] == "()" {
        tokens[0].to_string()
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "function missing",
        ));
    };
    let brace_pos = tokens
        .iter()
        .position(|t| t == "{")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "function missing {"))?;
    let mut depth = 0i32;
    let mut end_pos = None;
    for (idx, tok) in tokens.iter().enumerate().skip(brace_pos) {
        let t = token_str(tok);
        if t == "{" {
            depth += 1;
        } else if t == "}" {
            depth -= 1;
            if depth == 0 {
                end_pos = Some(idx);
                break;
            }
        }
    }
    let end_pos =
        end_pos.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "function missing }"))?;
    let body = tokens[brace_pos + 1..end_pos].to_vec();
    Ok((name, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_line;

    #[test]
    fn parse_case_basic() {
        let tokens = parse_line("case x in foo) echo hi ;; esac").unwrap();
        let (word, clauses) = parse_case_tokens(tokens).unwrap();
        assert_eq!(word, vec!["x"]);
        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].patterns.len(), 1);
        assert_eq!(clauses[0].patterns[0], vec!["foo"]);
        assert_eq!(token_str(&clauses[0].body[0]), "echo");
        assert_eq!(token_str(&clauses[0].body[1]), "hi");
    }

    #[test]
    fn parse_case_multi_pattern() {
        let tokens = parse_line("case x in foo | bar ) echo hi ;; esac").unwrap();
        let (_, clauses) = parse_case_tokens(tokens).unwrap();
        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].patterns.len(), 2);
        assert_eq!(clauses[0].patterns[0], vec!["foo"]);
        assert_eq!(clauses[0].patterns[1], vec!["bar"]);
    }
}
