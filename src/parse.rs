pub const OPERATOR_TOKEN_MARKER: char = '\x1e';
pub const NOGLOB_MARKER: char = '\x1d';
pub const ESCAPE_MARKER: char = '\x1f';

#[derive(Copy, Clone, Eq, PartialEq)]
enum ParseMode {
    Normal,
    Single,
    Double,
}

const MAX_SUBST_DEPTH: usize = 32;

#[derive(Copy, Clone, Debug)]
pub enum SeqOp {
    Always,
    And,
    Or,
}

#[derive(Debug)]
pub struct SeqSegment {
    pub op: SeqOp,
    pub tokens: Vec<String>,
    pub display: String,
}

#[derive(Debug, Clone)]
pub struct OutputRedirection {
    pub path: String,
    pub append: bool,
}

#[derive(Debug, Clone)]
pub struct HeredocSpec {
    pub delimiter: String,
    #[allow(dead_code)]
    pub quoted: bool,
    pub content: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub args: Vec<String>,
    pub stdin: Option<String>,
    pub heredoc: Option<HeredocSpec>,
    pub stdout: Option<OutputRedirection>,
}

impl CommandSpec {
    pub fn new() -> Self {
        Self {
            args: Vec::new(),
            stdin: None,
            heredoc: None,
            stdout: None,
        }
    }
}

impl Default for CommandSpec {
    fn default() -> Self {
        Self::new()
    }
}

pub fn parse_line(input: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut buf = String::new();
    let mut chars = input.chars().peekable();
    let mut mode = ParseMode::Normal;
    let mut in_token = false;

    while let Some(ch) = chars.next() {
        match mode {
            ParseMode::Normal => match ch {
                ' ' | '\t' => {
                    if in_token {
                        args.push(buf.clone());
                        buf.clear();
                        in_token = false;
                    }
                }
                '#' => {
                    if !in_token {
                        break;
                    }
                    buf.push('#');
                }
                '|' => {
                    if in_token {
                        args.push(buf.clone());
                        buf.clear();
                        in_token = false;
                    }
                    if matches!(chars.peek(), Some('|')) {
                        chars.next();
                        args.push(format!("{OPERATOR_TOKEN_MARKER}||"));
                    } else {
                        args.push(format!("{OPERATOR_TOKEN_MARKER}|"));
                    }
                }
                '>' => {
                    if in_token {
                        args.push(buf.clone());
                        buf.clear();
                        in_token = false;
                    }
                    if matches!(chars.peek(), Some('>')) {
                        chars.next();
                        args.push(format!("{OPERATOR_TOKEN_MARKER}>>"));
                    } else {
                        args.push(format!("{OPERATOR_TOKEN_MARKER}>"));
                    }
                }
                '<' => {
                    if in_token {
                        args.push(buf.clone());
                        buf.clear();
                        in_token = false;
                    }
                    if matches!(chars.peek(), Some('<')) {
                        chars.next();
                        args.push(format!("{OPERATOR_TOKEN_MARKER}<<"));
                    } else {
                        args.push(format!("{OPERATOR_TOKEN_MARKER}<"));
                    }
                }
                '&' => {
                    if in_token {
                        args.push(buf.clone());
                        buf.clear();
                        in_token = false;
                    }
                    if matches!(chars.peek(), Some('&')) {
                        chars.next();
                        args.push(format!("{OPERATOR_TOKEN_MARKER}&&"));
                    } else {
                        args.push(format!("{OPERATOR_TOKEN_MARKER}&"));
                    }
                }
                ';' => {
                    if in_token {
                        args.push(buf.clone());
                        buf.clear();
                        in_token = false;
                    }
                    args.push(format!("{OPERATOR_TOKEN_MARKER};"));
                }
                '\\' => {
                    if let Some(next) = chars.next() {
                        in_token = true;
                        push_escaped_normal(&mut buf, next);
                    } else {
                        in_token = true;
                        buf.push('\\');
                    }
                }
                '\'' => {
                    in_token = true;
                    mode = ParseMode::Single;
                }
                '"' => {
                    in_token = true;
                    mode = ParseMode::Double;
                }
                '$' => {
                    in_token = true;
                    if matches!(chars.peek(), Some('(')) {
                        chars.next();
                        let inner = parse_command_substitution(&mut chars)?;
                        buf.push_str("$(");
                        buf.push_str(&inner);
                        buf.push(')');
                    } else {
                        buf.push('$');
                    }
                }
                '`' => {
                    in_token = true;
                    let inner = parse_backticks(&mut chars)?;
                    buf.push_str("$(");
                    buf.push_str(&inner);
                    buf.push(')');
                }
                _ => {
                    in_token = true;
                    buf.push(ch);
                }
            },
            ParseMode::Single => {
                if ch == '\'' {
                    mode = ParseMode::Normal;
                } else {
                    in_token = true;
                    buf.push(ESCAPE_MARKER);
                    buf.push(ch);
                }
            }
            ParseMode::Double => match ch {
                '"' => mode = ParseMode::Normal,
                '\\' => {
                    if let Some(next) = chars.next() {
                        in_token = true;
                        push_escaped_double(&mut buf, next);
                    } else {
                        in_token = true;
                        buf.push('\\');
                    }
                }
                '$' => {
                    in_token = true;
                    if matches!(chars.peek(), Some('(')) {
                        chars.next();
                        let inner = parse_command_substitution(&mut chars)?;
                        buf.push(NOGLOB_MARKER);
                        buf.push_str("$(");
                        buf.push_str(&inner);
                        buf.push(')');
                    } else {
                        buf.push(NOGLOB_MARKER);
                        buf.push('$');
                    }
                }
                '`' => {
                    in_token = true;
                    let inner = parse_backticks(&mut chars)?;
                    buf.push(NOGLOB_MARKER);
                    buf.push_str("$(");
                    buf.push_str(&inner);
                    buf.push(')');
                }
                _ => {
                    in_token = true;
                    buf.push(NOGLOB_MARKER);
                    buf.push(ch);
                }
            },
        }
    }

    if mode != ParseMode::Normal {
        return Err("unterminated quote".to_string());
    }

    if in_token {
        args.push(buf);
    }

    Ok(args)
}

pub(crate) fn parse_command_substitution<I>(
    chars: &mut std::iter::Peekable<I>,
) -> Result<String, String>
where
    I: Iterator<Item = char>,
{
    let mut inner = String::new();
    let mut depth = 1usize;
    let mut mode = ParseMode::Normal;

    while let Some(ch) = chars.next() {
        match mode {
            ParseMode::Normal => match ch {
                '\\' => {
                    if let Some(next) = chars.next() {
                        inner.push('\\');
                        inner.push(next);
                    } else {
                        inner.push('\\');
                    }
                }
                '\'' => {
                    mode = ParseMode::Single;
                    inner.push(ch);
                }
                '"' => {
                    mode = ParseMode::Double;
                    inner.push(ch);
                }
                '$' => {
                    if matches!(chars.peek(), Some('(')) {
                        chars.next();
                        depth += 1;
                        if depth > MAX_SUBST_DEPTH {
                            return Err("command substitution nesting too deep".to_string());
                        }
                        inner.push_str("$(");
                    } else {
                        inner.push(ch);
                    }
                }
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(inner);
                    }
                    inner.push(ch);
                }
                _ => inner.push(ch),
            },
            ParseMode::Single => {
                if ch == '\'' {
                    mode = ParseMode::Normal;
                }
                inner.push(ch);
            }
            ParseMode::Double => match ch {
                '"' => {
                    mode = ParseMode::Normal;
                    inner.push(ch);
                }
                '\\' => {
                    if let Some(next) = chars.next() {
                        inner.push('\\');
                        inner.push(next);
                    } else {
                        inner.push('\\');
                    }
                }
                '$' => {
                    if matches!(chars.peek(), Some('(')) {
                        chars.next();
                        depth += 1;
                        if depth > MAX_SUBST_DEPTH {
                            return Err("command substitution nesting too deep".to_string());
                        }
                        inner.push_str("$(");
                    } else {
                        inner.push(ch);
                    }
                }
                _ => inner.push(ch),
            },
        }
    }

    Err("unterminated $(...)".to_string())
}

fn parse_backticks<I>(chars: &mut std::iter::Peekable<I>) -> Result<String, String>
where
    I: Iterator<Item = char>,
{
    let mut inner = String::new();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                if let Some(next) = chars.next() {
                    inner.push('\\');
                    inner.push(next);
                } else {
                    inner.push('\\');
                }
            }
            '`' => return Ok(inner),
            _ => inner.push(ch),
        }
    }

    Err("unterminated `...`".to_string())
}

fn push_escaped_normal(buf: &mut String, next: char) {
    let resolved = resolve_escape(next);
    buf.push(ESCAPE_MARKER);
    buf.push(resolved);
}

fn push_escaped_double(buf: &mut String, next: char) {
    let resolved = resolve_escape(next);
    buf.push(NOGLOB_MARKER);
    buf.push(ESCAPE_MARKER);
    buf.push(resolved);
}

fn resolve_escape(ch: char) -> char {
    match ch {
        'n' => '\n',
        't' => '\t',
        'r' => '\r',
        ' ' => ' ',
        '\\' => '\\',
        '"' => '"',
        '\'' => '\'',
        _ => ch,
    }
}

pub fn split_sequence(tokens: Vec<String>) -> Result<Vec<SeqSegment>, String> {
    let mut segments = Vec::new();
    let mut current = Vec::new();
    let mut next_op = SeqOp::Always;

    for token in tokens {
        if let Some(stripped) = token.strip_prefix(OPERATOR_TOKEN_MARKER) {
            match stripped {
                ";" | "&&" | "||" => {
                    if current.is_empty() {
                        return Err("empty command".to_string());
                    }
                    let display = tokens_to_display(&current);
                    segments.push(SeqSegment {
                        op: next_op,
                        tokens: current,
                        display,
                    });
                    current = Vec::new();
                    next_op = match stripped {
                        "&&" => SeqOp::And,
                        "||" => SeqOp::Or,
                        _ => SeqOp::Always,
                    };
                    continue;
                }
                _ => {}
            }
        }
        current.push(token);
    }

    if current.is_empty() {
        return Err("trailing operator".to_string());
    }

    let display = tokens_to_display(&current);
    segments.push(SeqSegment {
        op: next_op,
        tokens: current,
        display,
    });

    Ok(segments)
}

fn tokens_to_display(tokens: &[String]) -> String {
    let mut parts = Vec::with_capacity(tokens.len());
    for token in tokens {
        if let Some(stripped) = token.strip_prefix(OPERATOR_TOKEN_MARKER) {
            parts.push(stripped.to_string());
        } else {
            parts.push(token.clone());
        }
    }
    parts.join(" ")
}

pub fn split_pipeline(tokens: Vec<String>) -> Result<(Vec<CommandSpec>, bool), String> {
    let mut pipeline = Vec::new();
    let mut current = CommandSpec::new();
    let mut iter = tokens.into_iter().peekable();
    let mut background = false;

    while let Some(token) = iter.next() {
        if let Some(stripped) = token.strip_prefix(OPERATOR_TOKEN_MARKER) {
            match stripped {
                "|" => {
                    if current.args.is_empty() {
                        return Err("empty command in pipeline".to_string());
                    }
                    pipeline.push(current);
                    current = CommandSpec::new();
                }
                "<" => {
                    let path = iter
                        .next()
                        .ok_or_else(|| "missing input file".to_string())?;
                    if current.stdin.is_some() || current.heredoc.is_some() {
                        return Err("multiple input redirections".to_string());
                    }
                    current.stdin = Some(path);
                }
                "<<" => {
                    let raw = iter
                        .next()
                        .ok_or_else(|| "missing heredoc delimiter".to_string())?;
                    if current.stdin.is_some() || current.heredoc.is_some() {
                        return Err("multiple input redirections".to_string());
                    }
                    let quoted = raw.contains(ESCAPE_MARKER) || raw.contains(NOGLOB_MARKER);
                    let delimiter = strip_markers(&raw);
                    current.heredoc = Some(HeredocSpec {
                        delimiter,
                        quoted,
                        content: None,
                    });
                }
                ">" | ">>" => {
                    let path = iter
                        .next()
                        .ok_or_else(|| "missing output file".to_string())?;
                    if current.stdout.is_some() {
                        return Err("multiple output redirections".to_string());
                    }
                    current.stdout = Some(OutputRedirection {
                        path,
                        append: stripped == ">>",
                    });
                }
                "&" => {
                    if iter.peek().is_some() {
                        return Err("background operator must be at end of line".to_string());
                    }
                    background = true;
                }
                "&&" | "||" | ";" => {
                    return Err("unexpected control operator".to_string());
                }
                _ => current.args.push(stripped.to_string()),
            }
            continue;
        }

        current.args.push(token);
    }

    if current.args.is_empty() {
        return Err("trailing pipe".to_string());
    }

    pipeline.push(current);
    Ok((pipeline, background))
}

#[allow(dead_code)]
pub fn token_str(token: &str) -> &str {
    if let Some(stripped) = token.strip_prefix(OPERATOR_TOKEN_MARKER) {
        stripped
    } else {
        token
    }
}

pub fn strip_markers(input: &str) -> String {
    input
        .chars()
        .filter(|ch| *ch != ESCAPE_MARKER && *ch != NOGLOB_MARKER)
        .collect()
}

#[cfg(test)]
pub fn strip_all_markers(input: &str) -> String {
    let mut chars = input.chars();
    let mut out = String::new();
    if let Some(first) = chars.next() {
        if first != OPERATOR_TOKEN_MARKER && first != ESCAPE_MARKER && first != NOGLOB_MARKER {
            out.push(first);
        }
    }
    for ch in chars {
        if ch == ESCAPE_MARKER || ch == NOGLOB_MARKER {
            continue;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_basic() {
        let tokens = parse_line("ls -la /tmp").unwrap();
        assert_eq!(tokens, vec!["ls", "-la", "/tmp"]);
    }

    #[test]
    fn tokenize_operators_and_pipeline() {
        let tokens = parse_line("echo hi | cat").unwrap();
        assert!(tokens.contains(&format!("{OPERATOR_TOKEN_MARKER}|")));
        let (pipeline, background) = split_pipeline(tokens).unwrap();
        assert!(!background);
        assert_eq!(pipeline.len(), 2);
        assert_eq!(pipeline[0].args, vec!["echo", "hi"]);
        assert_eq!(pipeline[1].args, vec!["cat"]);
    }

    #[test]
    fn split_sequence_ops() {
        let tokens = parse_line("a && b || c ; d").unwrap();
        let segments = split_sequence(tokens).unwrap();
        assert_eq!(segments.len(), 4);
        assert!(matches!(segments[0].op, SeqOp::Always));
        assert!(matches!(segments[1].op, SeqOp::And));
        assert!(matches!(segments[2].op, SeqOp::Or));
        assert!(matches!(segments[3].op, SeqOp::Always));
        assert_eq!(segments[0].tokens, vec!["a"]);
        assert_eq!(segments[1].tokens, vec!["b"]);
        assert_eq!(segments[2].tokens, vec!["c"]);
        assert_eq!(segments[3].tokens, vec!["d"]);
    }

    #[test]
    fn quoting_and_escaping() {
        let tokens = parse_line("echo \"ab\\\"cd\"").unwrap();
        assert_eq!(strip_markers(&tokens[1]), "ab\"cd");

        let tokens = parse_line("echo 'single # and $'").unwrap();
        assert_eq!(strip_markers(&tokens[1]), "single # and $");

        let tokens = parse_line("printf '%s|%s' \"ab\"\"cd\" \"\"").unwrap();
        let stripped: Vec<String> = tokens.iter().map(|t| strip_markers(t)).collect();
        assert_eq!(stripped, vec!["printf", "%s|%s", "abcd", ""]);
    }

    #[test]
    fn backticks_and_command_substitution() {
        let tokens = parse_line("echo `echo hi`").unwrap();
        assert_eq!(tokens[1], "$(echo hi)");

        let tokens = parse_line("echo $(echo $(echo x))").unwrap();
        assert_eq!(tokens[1], "$(echo $(echo x))");
    }

    #[test]
    fn escapes_preserve_spaces() {
        let tokens = parse_line("echo foo\\ bar").unwrap();
        assert_eq!(strip_markers(&tokens[1]), "foo bar");

        let tokens = parse_line("echo \"line\\n\"").unwrap();
        assert_eq!(strip_markers(&tokens[1]), "line\n");
    }

    #[test]
    fn error_cases() {
        assert_eq!(
            parse_line("echo \"unterminated").unwrap_err(),
            "unterminated quote"
        );
        assert_eq!(
            parse_line("echo $(date").unwrap_err(),
            "unterminated $(...)"
        );
    }

    #[test]
    fn split_sequence_trailing_operator_errors() {
        let tokens = parse_line("a &&").unwrap();
        assert_eq!(split_sequence(tokens).unwrap_err(), "trailing operator");
    }

    #[test]
    fn split_pipeline_redirects_and_background() {
        let tokens = parse_line("cmd < in > out").unwrap();
        let (pipeline, background) = split_pipeline(tokens).unwrap();
        assert!(!background);
        assert_eq!(pipeline.len(), 1);
        assert_eq!(pipeline[0].stdin.as_deref(), Some("in"));
        assert_eq!(pipeline[0].stdout.as_ref().unwrap().path, "out");
        assert!(!pipeline[0].stdout.as_ref().unwrap().append);

        let tokens = parse_line("cmd >> out").unwrap();
        let (pipeline, _) = split_pipeline(tokens).unwrap();
        assert!(pipeline[0].stdout.as_ref().unwrap().append);

        let tokens = parse_line("cmd & other").unwrap();
        assert_eq!(
            split_pipeline(tokens).unwrap_err(),
            "background operator must be at end of line"
        );

        let tokens = parse_line("cmd << EOF").unwrap();
        let (pipeline, _) = split_pipeline(tokens).unwrap();
        let heredoc = pipeline[0].heredoc.as_ref().unwrap();
        assert_eq!(heredoc.delimiter, "EOF");
        assert!(!heredoc.quoted);
    }

    #[test]
    fn marker_helpers() {
        let input = format!("{ESCAPE_MARKER}a{NOGLOB_MARKER}b");
        assert_eq!(strip_markers(&input), "ab");

        let input = format!("{OPERATOR_TOKEN_MARKER}&&");
        assert_eq!(strip_all_markers(&input), "&&");
    }

    #[test]
    fn heredoc_quoted_and_unquoted_delimiters() {
        let tokens = parse_line("cat <<EOF").unwrap();
        let (pipeline, _) = split_pipeline(tokens).unwrap();
        let heredoc = pipeline[0].heredoc.as_ref().unwrap();
        assert_eq!(heredoc.delimiter, "EOF");
        assert!(!heredoc.quoted);

        let tokens = parse_line("cat <<'EOF'").unwrap();
        let (pipeline, _) = split_pipeline(tokens).unwrap();
        let heredoc = pipeline[0].heredoc.as_ref().unwrap();
        assert_eq!(heredoc.delimiter, "EOF");
        assert!(heredoc.quoted);
    }

    #[test]
    fn token_str_operator() {
        let token = format!("{OPERATOR_TOKEN_MARKER}||");
        assert_eq!(token_str(&token), "||");
        assert_eq!(token_str("echo"), "echo");
    }

    #[test]
    fn mixed_quotes_and_escaped_operator() {
        let tokens = parse_line("echo \"a'b'\\\"c\\\"\"").unwrap();
        assert_eq!(strip_markers(&tokens[1]), "a'b'\"c\"");

        let tokens = parse_line("echo 'a\"b\"c'").unwrap();
        assert_eq!(strip_markers(&tokens[1]), "a\"b\"c");

        let tokens = parse_line("echo foo\\|bar").unwrap();
        assert_eq!(strip_markers(&tokens[1]), "foo|bar");
    }

    #[test]
    fn command_substitution_with_escaped_quotes() {
        let tokens = parse_line("echo $(printf \\\"hi\\\")").unwrap();
        assert_eq!(tokens[1], "$(printf \\\"hi\\\")");
    }

    #[test]
    fn command_substitution_nesting_limit() {
        let mut input = String::from("echo ");
        for _ in 0..MAX_SUBST_DEPTH {
            input.push_str("$(");
        }
        input.push_str("echo x");
        for _ in 0..MAX_SUBST_DEPTH {
            input.push(')');
        }
        assert!(parse_line(&input).is_ok());

        let mut too_deep = String::from("echo ");
        for _ in 0..=MAX_SUBST_DEPTH {
            too_deep.push_str("$(");
        }
        too_deep.push_str("echo x");
        for _ in 0..=MAX_SUBST_DEPTH {
            too_deep.push(')');
        }
        assert_eq!(
            parse_line(&too_deep).unwrap_err(),
            "command substitution nesting too deep"
        );
    }

    #[test]
    fn backticks_with_nested_substitution() {
        let tokens = parse_line("echo `echo $(echo hi)`").unwrap();
        assert_eq!(tokens[1], "$(echo $(echo hi))");
    }

    #[test]
    fn quoted_command_substitution_token() {
        let tokens = parse_line("echo \"$(echo a)\"").unwrap();
        assert_eq!(strip_markers(&tokens[1]), "$(echo a)");
    }
}
