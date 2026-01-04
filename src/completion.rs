use std::collections::HashMap;
use std::env;
use std::fs;

use rustyline::Editor;
use rustyline::completion::{Completer, FilenameCompleter, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::{Highlighter, MatchingBracketHighlighter};
use rustyline::hint::{Hinter, HistoryHinter};
use rustyline::history::DefaultHistory;
use rustyline::validate::{MatchingBracketValidator, Validator};
use rustyline::{Context, Helper};

use crate::job_control::Job;

pub struct LineHelper {
    completer: FilenameCompleter,
    hinter: HistoryHinter,
    highlighter: MatchingBracketHighlighter,
    validator: MatchingBracketValidator,
    commands: Vec<String>,
    vars: Vec<String>,
    jobs: Vec<String>,
}

impl LineHelper {
    pub fn new() -> Self {
        Self {
            completer: FilenameCompleter::new(),
            hinter: HistoryHinter {},
            highlighter: MatchingBracketHighlighter::new(),
            validator: MatchingBracketValidator::new(),
            commands: Vec::new(),
            vars: Vec::new(),
            jobs: Vec::new(),
        }
    }

    fn update_context(&mut self, commands: Vec<String>, vars: Vec<String>, jobs: Vec<String>) {
        self.commands = commands;
        self.vars = vars;
        self.jobs = jobs;
    }
}

pub fn update_completion_context(
    editor: &mut Editor<LineHelper, DefaultHistory>,
    aliases: &HashMap<String, Vec<String>>,
    functions: &HashMap<String, Vec<String>>,
    jobs: &[Job],
) {
    let commands = collect_commands(aliases, functions);
    let vars = env::vars().map(|(k, _)| k).collect();
    let jobs = jobs.iter().map(|job| job.id.to_string()).collect();
    if let Some(helper) = editor.helper_mut() {
        helper.update_context(commands, vars, jobs);
    }
}

fn collect_commands(
    aliases: &HashMap<String, Vec<String>>,
    functions: &HashMap<String, Vec<String>>,
) -> Vec<String> {
    let mut entries = Vec::new();
    entries.extend(
        ["cd", "pwd", "jobs", "fg", "bg", "help", "exit", "set"]
            .iter()
            .map(|s| s.to_string()),
    );
    entries.extend(aliases.keys().cloned());
    entries.extend(functions.keys().cloned());
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

fn current_token(line: &str, pos: usize) -> (usize, String) {
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

fn is_command_position(line: &str, start: usize) -> bool {
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

fn complete_from_list(prefix: &str, list: &[String], leader: &str) -> Vec<Pair> {
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

impl Helper for LineHelper {}

impl Completer for LineHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        ctx: &Context<'_>,
    ) -> Result<(usize, Vec<Pair>), ReadlineError> {
        let (start, token) = current_token(line, pos);
        if let Some(rest) = token.strip_prefix("${") {
            let prefix = rest;
            let mut pairs = complete_from_list(prefix, &self.vars, "${");
            for pair in &mut pairs {
                if !pair.replacement.ends_with('}') {
                    pair.replacement.push('}');
                    pair.display.push('}');
                }
            }
            pairs.extend(self.completer.complete(line, pos, ctx)?.1);
            return Ok((start, pairs));
        }
        if token.starts_with('$') {
            let prefix = token.trim_start_matches('$');
            let mut pairs = complete_from_list(prefix, &self.vars, "$");
            pairs.extend(self.completer.complete(line, pos, ctx)?.1);
            return Ok((start, pairs));
        }
        if token.starts_with('%') {
            let prefix = token.trim_start_matches('%');
            let mut pairs = complete_from_list(prefix, &self.jobs, "%");
            pairs.extend(self.completer.complete(line, pos, ctx)?.1);
            return Ok((start, pairs));
        }
        if is_command_position(line, start) {
            let mut pairs = complete_from_list(token.as_str(), &self.commands, "");
            pairs.extend(self.completer.complete(line, pos, ctx)?.1);
            return Ok((start, pairs));
        }
        self.completer.complete(line, pos, ctx)
    }
}

impl Hinter for LineHelper {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, ctx: &Context<'_>) -> Option<String> {
        self.hinter.hint(line, pos, ctx)
    }
}

impl Highlighter for LineHelper {
    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
        &'s self,
        prompt: &'p str,
        default: bool,
    ) -> std::borrow::Cow<'b, str> {
        self.highlighter.highlight_prompt(prompt, default)
    }
}

impl Validator for LineHelper {
    fn validate(
        &self,
        ctx: &mut rustyline::validate::ValidationContext<'_>,
    ) -> Result<rustyline::validate::ValidationResult, ReadlineError> {
        self.validator.validate(ctx)
    }
}
