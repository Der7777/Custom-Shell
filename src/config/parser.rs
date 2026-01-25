use std::collections::HashMap;
use std::env;
use std::fs;
use std::io;

use crate::colors::{load_color_lines, ColorConfig};
use crate::execution::{apply_sandbox_directive, SandboxConfig};
use crate::parse::{parse_line, parse_sandbox_value};
use crate::prompt::{parse_prompt_theme, PromptTheme};
use crate::utils::is_valid_var_name;

pub fn load_config(
    aliases: &mut HashMap<String, Vec<String>>,
    prompt_template: &mut Option<String>,
    prompt_function: &mut Option<String>,
    prompt_theme: &mut PromptTheme,
    colors: &mut ColorConfig,
    sandbox: &mut SandboxConfig,
    abbreviations: &mut HashMap<String, Vec<String>>,
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
        if let Some(rest) = line.strip_prefix("abbr ") {
            if let Err(err) = parse_abbreviation(abbreviations, rest, idx + 1) {
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
            if key.eq_ignore_ascii_case("prompt_function")
                || key.eq_ignore_ascii_case("prompt_func")
            {
                if value.trim().is_empty() {
                    *prompt_function = None;
                } else {
                    *prompt_function = Some(value.trim().to_string());
                }
                continue;
            }
            if key.eq_ignore_ascii_case("prompt_theme") || key.eq_ignore_ascii_case("theme") {
                if let Some(theme) = parse_prompt_theme(value) {
                    *prompt_theme = theme;
                } else {
                    eprintln!("config:{}: unknown theme '{value}'", idx + 1);
                }
                continue;
            }
            if let Some(color_key) = key.strip_prefix("color.") {
                load_color_lines(colors, &format!("color.{color_key}={value}"));
                continue;
            }
            if key.eq_ignore_ascii_case("sandbox") {
                match parse_sandbox_value(value) {
                    Ok(directive) => apply_sandbox_directive(sandbox, directive),
                    Err(err) => eprintln!("config:{}: {err}", idx + 1),
                }
                continue;
            }
            if let Err(err) = parse_assignment(line, idx + 1) {
                eprintln!("config:{}: {err}", idx + 1);
            }
            continue;
        }
        eprintln!("config:{}: unrecognized directive", idx + 1);
    }

    let abbr_path = format!("{home}/.minishell_abbr");
    if let Ok(content) = fs::read_to_string(&abbr_path) {
        for (idx, raw) in content.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(rest) = line.strip_prefix("abbr ") {
                if let Err(err) = parse_abbreviation(abbreviations, rest, idx + 1) {
                    eprintln!("abbr:{}: {err}", idx + 1);
                }
            }
        }
    }

    let colors_path = format!("{home}/.minishell_colors");
    if let Ok(content) = fs::read_to_string(&colors_path) {
        load_color_lines(colors, &content);
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

fn parse_abbreviation(
    abbreviations: &mut HashMap<String, Vec<String>>,
    input: &str,
    line: usize,
) -> Result<(), String> {
    let trimmed = input.trim();
    let split_at = trimmed
        .char_indices()
        .find(|(_, ch)| ch.is_whitespace())
        .map(|(idx, _)| idx)
        .ok_or_else(|| format!("abbr missing value on line {line}"))?;
    let name = trimmed[..split_at].trim();
    let value = trimmed[split_at..].trim();
    if !is_valid_var_name(name) {
        return Err(format!("invalid abbr name '{name}' on line {line}"));
    }
    let value = strip_quotes(value);
    let tokens =
        parse_line(value).map_err(|err| format!("abbr parse error on line {line}: {err}"))?;
    if tokens.is_empty() {
        return Err(format!("abbr '{name}' empty on line {line}"));
    }
    abbreviations.insert(name.to_string(), tokens);
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

pub(crate) fn shell_quote(token: &str) -> String {
    if token.is_empty() || token.chars().any(needs_quotes) {
        let mut out = String::from("'");
        for ch in token.chars() {
            if ch == '\'' {
                out.push_str("'\\''");
            } else {
                out.push(ch);
            }
        }
        out.push('\'');
        out
    } else {
        token.to_string()
    }
}

fn needs_quotes(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            '\'' | '"'
                | '\\'
                | '$'
                | '`'
                | '#'
                | '|'
                | '&'
                | ';'
                | '<'
                | '>'
                | '('
                | ')'
                | '{'
                | '}'
        )
}
