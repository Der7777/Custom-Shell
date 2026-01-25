use std::collections::HashMap;
use std::env;
use std::fs;

use rustyline::completion::Pair;
use rustyline::history::DefaultHistory;
use rustyline::Editor;

use crate::colors::ColorConfig;
use crate::completions::CompletionSet;
use crate::job_control::Job;
use crate::parse::{parse_line_lenient, OPERATOR_TOKEN_MARKER};
use crate::completion::LineHelper;

pub fn update_completion_context(
    editor: &mut Editor<LineHelper, DefaultHistory>,
    aliases: &HashMap<String, Vec<String>>,
    functions: &HashMap<String, Vec<String>>,
    abbreviations: &HashMap<String, Vec<String>>,
    completions: &CompletionSet,
    colors: &ColorConfig,
    jobs: &[Job],
) {
    let commands = collect_commands(aliases, functions, abbreviations);
    let vars = env::vars().map(|(k, _)| k).collect();
    let jobs = jobs.iter().map(|job| job.id.to_string()).collect();
    if let Some(helper) = editor.helper_mut() {
        helper.update_context(
            commands,
            vars,
            jobs,
            abbreviations.clone(),
            completions.clone(),
            colors,
        );
    }
}

fn collect_commands(
    aliases: &HashMap<String, Vec<String>>,
    functions: &HashMap<String, Vec<String>>,
    abbreviations: &HashMap<String, Vec<String>>,
) -> Vec<String> {
    let mut entries = Vec::new();
    entries.extend(
        [
            "cd", "pwd", "jobs", "fg", "bg", "help", "exit", "set", "abbr", "complete",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    entries.extend(aliases.keys().cloned());
    entries.extend(functions.keys().cloned());
    entries.extend(abbreviations.keys().cloned());
    if let Ok(path) = env::var("PATH") {
        for dir in path.split(':') {
            if let Ok(read) = fs::read_dir(dir) {
                for entry in read.flatten() {
                    if let Ok(name) = entry.file_name().into_string() {
                        entries.push(name);
                    }
                }
            }
        }
    }
    entries.sort();
    entries.dedup();
    entries
}

pub(crate) fn current_token(line: &str, pos: usize) -> (usize, String) {
    let mut start = pos;
    let bytes = line.as_bytes();
    while start > 0 {
        let ch = bytes[start - 1] as char;
        if ch.is_whitespace() || is_operator_char(ch) {
            break;
        }
        start -= 1;
    }
    (start, line[start..pos].to_string())
}

fn is_operator_char(ch: char) -> bool {
    matches!(ch, '|' | '&' | ';' | '(' | ')' | '{' | '}')
}

pub(crate) fn is_command_position(line: &str, start: usize) -> bool {
    if start == 0 {
        return true;
    }
    let prefix = line[..start].trim_end();
    if prefix.is_empty() {
        return true;
    }
    if let Some(ch) = prefix.chars().last() {
        return is_operator_char(ch);
    }
    false
}

pub(crate) fn complete_from_list(prefix: &str, list: &[String], leader: &str) -> Vec<Pair> {
    let mut out = Vec::new();
    for item in list {
        if item.starts_with(prefix) {
            out.push(Pair {
                display: format!("{leader}{item}"),
                replacement: format!("{leader}{item}"),
            });
        }
    }
    out
}

pub(crate) fn command_for_position(line: &str, pos: usize) -> Option<String> {
    let prefix = &line[..pos];
    let tokens = parse_line_lenient(prefix).ok()?;
    let mut in_command = true;
    let mut command = None;
    for token in tokens {
        if token.starts_with(OPERATOR_TOKEN_MARKER) {
            if is_command_delimiter(&token) {
                in_command = true;
            }
            continue;
        }
        if in_command {
            command = Some(token);
            in_command = false;
        }
    }
    command
}

fn is_command_delimiter(token: &str) -> bool {
    let op = token.trim_start_matches(OPERATOR_TOKEN_MARKER);
    matches!(op, "|" | "||" | "&&" | ";" | "&")
}
