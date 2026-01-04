use std::collections::HashMap;
use std::env;
use std::fs;
use std::io;
use std::path::Path;

use crate::parse::{OPERATOR_TOKEN_MARKER, parse_line};
use crate::utils::is_valid_var_name;

pub fn build_prompt(
    interactive: bool,
    prompt_template: &Option<String>,
    last_status: i32,
    cwd: &Path,
) -> String {
    if !interactive {
        return String::new();
    }
    if let Some(ref template) = prompt_template {
        let status_str = last_status.to_string();
        let status_opt = if last_status == 0 { "" } else { &status_str };
        let mut out = template.replace("{status?}", status_opt);
        out = out.replace("{status}", &status_str);
        out = out.replace("{cwd}", &cwd.display().to_string());
        out
    } else if last_status == 0 {
        format!("{} $ ", cwd.display())
    } else {
        format!("[{}] {} $ ", last_status, cwd.display())
    }
}

pub fn apply_aliases(tokens: Vec<String>, aliases: &HashMap<String, Vec<String>>) -> Vec<String> {
    let Some(first) = tokens.first() else {
        return tokens;
    };
    if first.starts_with(OPERATOR_TOKEN_MARKER) {
        return tokens;
    }
    let Some(repl) = aliases.get(first) else {
        return tokens;
    };
    let mut out = Vec::with_capacity(repl.len() + tokens.len());
    out.extend(repl.iter().cloned());
    out.extend(tokens.into_iter().skip(1));
    out
}

pub fn load_config(
    aliases: &mut HashMap<String, Vec<String>>,
    prompt_template: &mut Option<String>,
) -> io::Result<()> {
    let Some(home) = env::var("HOME").ok() else {
        return Ok(());
    };
    let path = format!("{home}/.minishellrc");
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };

    for (idx, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("alias ") {
            if let Err(err) = parse_alias(aliases, rest, idx + 1) {
                eprintln!("config:{}: {err}", idx + 1);
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("export ") {
            if let Err(err) = parse_assignment(rest, idx + 1) {
                eprintln!("config:{}: {err}", idx + 1);
            }
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = strip_quotes(value.trim());
            if key.eq_ignore_ascii_case("prompt") || key == "PROMPT" {
                *prompt_template = Some(value.to_string());
                continue;
            }
            if let Err(err) = parse_assignment(line, idx + 1) {
                eprintln!("config:{}: {err}", idx + 1);
            }
            continue;
        }
        eprintln!("config:{}: unrecognized directive", idx + 1);
    }

    Ok(())
}

fn parse_alias(
    aliases: &mut HashMap<String, Vec<String>>,
    input: &str,
    line: usize,
) -> Result<(), String> {
    let (name, value) = input
        .split_once('=')
        .ok_or_else(|| format!("alias missing '=' on line {line}"))?;
    let name = name.trim();
    if !is_valid_var_name(name) {
        return Err(format!("invalid alias name '{name}' on line {line}"));
    }
    let value = strip_quotes(value.trim());
    let tokens =
        parse_line(value).map_err(|err| format!("alias parse error on line {line}: {err}"))?;
    if tokens.is_empty() {
        return Err(format!("alias '{name}' empty on line {line}"));
    }
    aliases.insert(name.to_string(), tokens);
    Ok(())
}

fn parse_assignment(input: &str, line: usize) -> Result<(), String> {
    let trimmed = input.trim();
    let (name, value) = trimmed
        .split_once('=')
        .ok_or_else(|| format!("assignment missing '=' on line {line}"))?;
    let name = name.trim();
    if !is_valid_var_name(name) {
        return Err(format!("invalid variable name '{name}' on line {line}"));
    }
    let value = strip_quotes(value.trim());
    env::set_var(name, value);
    Ok(())
}

fn strip_quotes(input: &str) -> &str {
    let bytes = input.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &input[1..bytes.len() - 1];
        }
    }
    input
}
