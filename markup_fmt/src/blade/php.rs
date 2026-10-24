//! Blade ⇄ PHP bridge for the external formatter.
//!
//! Blade snippets are wrapped into complete PHP (`frame`), handed to the host's `php`
//! formatter, and the inner text is extracted again (`unframe`). A result is only accepted when
//! `postconditions_hold` proves it cannot change how Laravel's compiler reads the template.
//! Everything here is pure: no I/O, no formatter calls.
//!
//! Framing table (`ARGS` = directive arguments, trimmed; directive names match
//! case-insensitively):
//!
//! | Snippet | Frame |
//! |---|---|
//! | `Echo { expr: e, .. }`, `Attr(e)` | `<?php (e);` |
//! | `@if` `@elseif` `@unless` | `<?php if (ARGS) {}` |
//! | `@foreach` `@forelse` | `<?php foreach (ARGS) {}` |
//! | `@for` / `@while` / `@switch` | `<?php for (ARGS) {}` / `while` / `switch` |
//! | `@case` | `<?php switch (1) { case ARGS: }` |
//! | `@isset` `@empty` `@unset` | `<?php isset(ARGS);` etc. |
//! | other built-ins with arguments | `<?php name(ARGS);` |
//! | `Raw(body)` | `<?php` + body |
//! | `@use`, `@verbatim`, `@php`, custom/unknown names | not framed (`None`) |
//!
//! `name` in `name(ARGS)` is the directive's own spelling, except for names that are PHP
//! reserved words (`@include`, `@yield`, `@break`, `@class`, `@readonly`, …): `include(...)`
//! or `break($i > 3)` are not call expressions, so those use [`NEUTRAL_CALL`].
//!
//! `<?=` decision: `BladePhpSnippet::Raw` is for statement bodies of `<?php … ?>` and
//! `@php … @endphp` only. A `<?= … ?>` body is an `echo` argument list that may carry a trailing
//! `;` or several comma-separated expressions, so it has no faithful frame: callers MUST keep
//! `<?=` bodies byte-for-byte (never send them).
//!
//! Results of `unframe` for `Raw` have no leading/trailing whitespace, except that a body whose
//! last token is a `//` or `#` comment keeps one trailing `\n` so the closing marker lands on
//! its own line. All results use `\n` line endings.

use std::collections::HashMap;

/// What the printer wants formatted. Every string is the source text between the Blade delimiters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BladePhpSnippet<'a> {
    /// `{{ e }}`, `{!! e !!}`, `{{{ e }}}`; `kind` only sizes the delimiters.
    Echo {
        expr: &'a str,
        kind: crate::ast::BladeEchoKind,
    },
    /// `@name(args)`; `args` is the text strictly inside the outer parentheses.
    Directive { name: &'a str, args: &'a str },
    /// Body of `<?php … ?>` or `@php … @endphp`.
    Raw(&'a str),
    /// Component `:attr="e"` value.
    Attr(&'a str),
}

#[derive(Clone, Debug)]
enum Shape {
    /// `(e);`
    Expr,
    /// `name(ARGS);`
    Call(String),
    /// `keyword (ARGS) {}`
    Control(&'static str),
    /// `switch (1) { case ARGS: }`
    Case,
    /// `<?php` + body
    Raw,
}

/// A snippet wrapped into PHP, plus what is needed to map the formatter's answer back.
#[derive(Clone, Debug)]
pub(crate) struct Framed {
    /// Complete PHP source to send to the formatter.
    pub code: String,
    shape: Shape,
    /// Whether the original arguments ended with a top-level `,` (so one may be kept).
    original_trailing_comma: bool,
}

/// Neutral callee for directives whose name is a PHP reserved word.
const NEUTRAL_CALL: &str = "blade_directive";

/// Built-in directives whose arguments are a PHP argument list (Laravel v13.38.0).
/// `if`-like, loop, `case`, `isset`/`empty`/`unset` are handled separately; `@use`, `@php`,
/// `@verbatim` are deliberately absent.
const CALL_DIRECTIVES: &[&str] = &[
    "include",
    "includeif",
    "includewhen",
    "includeunless",
    "includefirst",
    "includeisolated",
    "each",
    "extends",
    "extendsfirst",
    "yield",
    "stack",
    "props",
    "aware",
    "inject",
    "method",
    "class",
    "style",
    "checked",
    "selected",
    "disabled",
    "readonly",
    "required",
    "json",
    "js",
    "bool",
    "choice",
    "lang",
    "vite",
    "fonts",
    "dd",
    "dump",
    "section",
    "push",
    "prepend",
    "pushonce",
    "prependonce",
    "pushif",
    "elsepushif",
    "elsepush",
    "slot",
    "component",
    "componentfirst",
    "hassection",
    "sectionmissing",
    "hasstack",
    "auth",
    "elseauth",
    "guest",
    "elseguest",
    "can",
    "cannot",
    "canany",
    "elsecan",
    "elsecannot",
    "elsecanany",
    "env",
    "production",
    "error",
    "session",
    "context",
    "fragment",
];

const PHP_RESERVED: &[&str] = &[
    "abstract",
    "and",
    "array",
    "as",
    "break",
    "callable",
    "case",
    "catch",
    "class",
    "clone",
    "const",
    "continue",
    "declare",
    "default",
    "die",
    "do",
    "echo",
    "else",
    "elseif",
    "empty",
    "enddeclare",
    "endfor",
    "endforeach",
    "endif",
    "endswitch",
    "endwhile",
    "enum",
    "eval",
    "exit",
    "extends",
    "final",
    "finally",
    "fn",
    "for",
    "foreach",
    "function",
    "global",
    "goto",
    "if",
    "implements",
    "include",
    "include_once",
    "instanceof",
    "insteadof",
    "interface",
    "isset",
    "list",
    "match",
    "namespace",
    "new",
    "or",
    "print",
    "private",
    "protected",
    "public",
    "readonly",
    "require",
    "require_once",
    "return",
    "static",
    "switch",
    "throw",
    "trait",
    "try",
    "unset",
    "use",
    "var",
    "while",
    "xor",
    "yield",
];

/// Directives whose arguments Laravel splits with a textual `explode(',', …)`.
const COMMA_SPLITTING: &[&str] = &[
    "json",
    "js",
    "pushif",
    "elsepushif",
    "elsepush",
    "inject",
    "pushonce",
    "prependonce",
    "each",
    "section",
    "push",
    "prepend",
    "slot",
];

/// Wraps `snippet` into PHP. `None` means the snippet is opaque or cannot be framed safely
/// (custom/unknown directive, `@use`, empty or unbalanced payload, unterminated string or
/// trailing `//` comment, a `?>` in the payload): the caller keeps the source.
pub(crate) fn frame(snippet: &BladePhpSnippet) -> Option<Framed> {
    match *snippet {
        BladePhpSnippet::Echo { expr, .. } | BladePhpSnippet::Attr(expr) => {
            let expr = expr.trim();
            payload_ok(expr).then(|| Framed {
                code: format!("<?php ({expr});"),
                shape: Shape::Expr,
                original_trailing_comma: false,
            })
        }
        BladePhpSnippet::Raw(body) => {
            if body.trim().is_empty() || body.contains("?>") {
                return None;
            }
            Code::new(body)?;
            let separator = if body.starts_with(char::is_whitespace) {
                ""
            } else {
                " "
            };
            Some(Framed {
                code: format!("<?php{separator}{body}"),
                shape: Shape::Raw,
                original_trailing_comma: false,
            })
        }
        BladePhpSnippet::Directive { name, args } => {
            let args = args.trim();
            if !payload_ok(args) {
                return None;
            }
            let lower = name.to_ascii_lowercase();
            let (code, shape) = match lower.as_str() {
                "if" | "elseif" | "unless" => {
                    (format!("<?php if ({args}) {{}}"), Shape::Control("if"))
                }
                "foreach" | "forelse" => (
                    format!("<?php foreach ({args}) {{}}"),
                    Shape::Control("foreach"),
                ),
                "for" => (format!("<?php for ({args}) {{}}"), Shape::Control("for")),
                "while" => (
                    format!("<?php while ({args}) {{}}"),
                    Shape::Control("while"),
                ),
                "switch" => (
                    format!("<?php switch ({args}) {{}}"),
                    Shape::Control("switch"),
                ),
                "case" => (format!("<?php switch (1) {{ case {args}: }}"), Shape::Case),
                "isset" | "empty" | "unset" => (
                    format!("<?php {lower}({args});"),
                    Shape::Call(lower.clone()),
                ),
                _ if CALL_DIRECTIVES.contains(&lower.as_str()) => {
                    let callee = if PHP_RESERVED.contains(&lower.as_str()) || !is_identifier(name) {
                        NEUTRAL_CALL
                    } else {
                        name
                    };
                    (
                        format!("<?php {callee}({args});"),
                        Shape::Call(callee.to_owned()),
                    )
                }
                _ => return None,
            };
            let original_trailing_comma = Code::new(args)?.trailing_comma().is_some();
            Some(Framed {
                code,
                shape,
                original_trailing_comma,
            })
        }
    }
}

/// Extracts the inner text from the formatter's answer to `framed.code`.
///
/// Tolerates a newline-separated `<?php` header, added/removed trailing newline or `?>`, dropped
/// redundant parentheses around an echo expression, `{}` becoming `{\n}`, conditions wrapped
/// across lines, and a trailing comma added to a call's arguments (stripped again unless the
/// source had one). Multi-line payloads are dedented by the indentation the frame introduced;
/// lines that start inside a string/heredoc are never touched. Returns `None` for anything else.
pub(crate) fn unframe(framed: &Framed, formatted: &str) -> Option<String> {
    let body = strip_php_frame(formatted)?;
    match &framed.shape {
        Shape::Raw => unframe_raw(body),
        Shape::Expr => {
            let code = Code::new(body)?;
            let last = body.len().checked_sub(1)?;
            if !code.top_level(b';').eq([last]) {
                return None;
            }
            let mut inner = &body[..last];
            loop {
                let trimmed = inner.trim();
                if !trimmed.starts_with('(') {
                    break;
                }
                let wrapped = Code::new(trimmed)?;
                if wrapped.matching_close(0) != Some(trimmed.len() - 1) {
                    break;
                }
                inner = &trimmed[1..trimmed.len() - 1];
            }
            finish(inner, false)
        }
        Shape::Call(name) => {
            let (inner, tail) = split_call(body, name)?;
            if tail.trim() != ";" {
                return None;
            }
            finish(inner, !framed.original_trailing_comma)
        }
        Shape::Control(keyword) => {
            let (inner, tail) = split_call(body, keyword)?;
            if compact(tail) != "{}" {
                return None;
            }
            finish(inner, false)
        }
        Shape::Case => {
            let (head, tail) = split_call(body, "switch")?;
            if head.trim() != "1" {
                return None;
            }
            let middle = tail.trim().strip_prefix('{')?.strip_suffix('}')?.trim();
            if !middle.get(..4)?.eq_ignore_ascii_case("case") {
                return None;
            }
            let after = &middle[4..];
            if !after.starts_with(|c: char| c.is_whitespace() || c == '(') {
                return None;
            }
            finish(after.strip_suffix(':')?, false)
        }
    }
}

/// Whether `formatted` (the unframed inner text) is safe to put where `original` was.
pub(crate) fn postconditions_hold(
    snippet: &BladePhpSnippet,
    original: &str,
    formatted: &str,
) -> bool {
    let (Some(original_code), Some(formatted_code)) = (Code::new(original), Code::new(formatted))
    else {
        return false;
    };

    if let BladePhpSnippet::Directive { name, .. } = *snippet {
        let lower = name.to_ascii_lowercase();
        if matches!(lower.as_str(), "foreach" | "forelse") && !foreach_shape_ok(formatted) {
            return false;
        }
        if lower == "lang" && first_char_class(original) != first_char_class(formatted) {
            return false;
        }
        if COMMA_SPLITTING.contains(&lower.as_str())
            && !commas_unchanged(original, &original_code, formatted, &formatted_code)
        {
            return false;
        }
    }

    // Delimiters that would end the construct early (or start a new one).
    let is_raw = matches!(snippet, BladePhpSnippet::Raw(_));
    for token in ["}}", "!!}", "--}}", "@endphp"] {
        if formatted.matches(token).count() > original.matches(token).count() {
            return false;
        }
    }
    let closing_tags = formatted.matches("?>").count();
    if closing_tags > 0 && (!is_raw || closing_tags > original.matches("?>").count()) {
        return false;
    }

    // `\B@name` tokens that were not in the source would become directives.
    let mut allowed = HashMap::new();
    for token in directive_tokens(original) {
        *allowed.entry(token).or_insert(0usize) += 1;
    }
    for token in directive_tokens(formatted) {
        match allowed.get_mut(token) {
            Some(count) if *count > 0 => *count -= 1,
            _ => return false,
        }
    }

    if formatted_code.unterminated_line_comment() {
        return false;
    }

    // No CRLF normalization: `\r\n` ↔ `\n` inside a literal changes the string's value.
    let mut original_literals = original_code.literals();
    let mut formatted_literals = formatted_code.literals();
    loop {
        match (original_literals.next(), formatted_literals.next()) {
            (None, None) => return true,
            (Some(original), Some(formatted)) if literal_equivalent(original, formatted) => {}
            _ => return false,
        }
    }
}

/// Same bytes, or a `'x'` ↔ `"x"` quote flip of content that has no escape, interpolation or
/// quote character (so both quote styles denote the same string). Heredocs, nowdocs and
/// backtick strings must match exactly.
fn literal_equivalent(original: &str, formatted: &str) -> bool {
    if original == formatted {
        return true;
    }
    let simple_content = |literal: &str| {
        let quote = *literal.as_bytes().first()?;
        if !matches!(quote, b'\'' | b'"')
            || literal.len() < 2
            || literal.as_bytes().last() != Some(&quote)
        {
            return None;
        }
        Some(literal[1..literal.len() - 1].to_owned())
    };
    match (simple_content(original), simple_content(formatted)) {
        (Some(a), Some(b)) => a == b && !a.contains(['\\', '$', '\'', '"']),
        _ => false,
    }
}

/// Columns the Blade delimiters take around the snippet, to subtract from the print width.
pub(crate) fn print_width_overhead(snippet: &BladePhpSnippet) -> usize {
    match snippet {
        BladePhpSnippet::Echo { kind, .. } => match kind {
            // `{{ ` + ` }}`
            crate::ast::BladeEchoKind::Escaped => 6,
            // `{!! ` + ` !!}` and `{{{ ` + ` }}}`
            crate::ast::BladeEchoKind::Raw | crate::ast::BladeEchoKind::Triple => 8,
        },
        BladePhpSnippet::Attr(_) => "\"\"".len(),
        // `@name(` + `)`
        BladePhpSnippet::Directive { name, .. } => name.len() + 3,
        BladePhpSnippet::Raw(_) => 0,
    }
}

// ---------------------------------------------------------------------------------------------
// postcondition helpers

/// `^ *(.+) +as +(.+)$` on one line with single spaces around `as` (Laravel splits on the last
/// ` +as +`).
fn foreach_shape_ok(formatted: &str) -> bool {
    if formatted.contains(['\n', '\r']) {
        return false;
    }
    let lower = formatted.to_ascii_lowercase();
    let Some(position) = lower.rfind(" as ") else {
        return false;
    };
    let left = &formatted[..position];
    let right = &formatted[position + " as ".len()..];
    !left.trim().is_empty()
        && !right.trim().is_empty()
        && !left.ends_with(char::is_whitespace)
        && !right.starts_with(char::is_whitespace)
}

/// `@lang` is a block only when the character after `(` is `[`; whitespace there also matters.
fn first_char_class(args: &str) -> u8 {
    match args.chars().next() {
        Some('[') => 1,
        Some(c) if c.is_whitespace() => 2,
        _ => 3,
    }
}

fn commas_unchanged(
    original: &str,
    original_code: &Code,
    formatted: &str,
    formatted_code: &Code,
) -> bool {
    // Laravel explodes these on every `,` of the raw text, strings and nested arrays included.
    original.matches(',').count() == formatted.matches(',').count()
        && original_code.top_level(b',').count() == formatted_code.top_level(b',').count()
        && (formatted_code.trailing_comma().is_none() || original_code.trailing_comma().is_some())
}

/// `\B@[A-Za-z_]\w*` tokens.
fn directive_tokens(text: &str) -> impl Iterator<Item = &str> {
    let bytes = text.as_bytes();
    text.match_indices('@').filter_map(move |(at, _)| {
        let next = *bytes.get(at + 1)?;
        if !(next.is_ascii_alphabetic() || next == b'_') {
            return None;
        }
        if at > 0 && is_word_byte(bytes[at - 1]) {
            return None;
        }
        let len = bytes[at + 1..]
            .iter()
            .take_while(|b| is_word_byte(**b))
            .count();
        Some(&text[at..at + 1 + len])
    })
}

// ---------------------------------------------------------------------------------------------
// framing helpers

fn payload_ok(payload: &str) -> bool {
    !payload.is_empty()
        && !payload.contains("?>")
        && Code::new(payload)
            .is_some_and(|code| code.balanced() && !code.unterminated_line_comment())
}

fn is_identifier(name: &str) -> bool {
    name.bytes().next().is_some_and(|b| !b.is_ascii_digit()) && name.bytes().all(is_word_byte)
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Removes the `<?php` header and an optional closing `?>`; returns the trimmed rest.
fn strip_php_frame(formatted: &str) -> Option<&str> {
    let text = formatted.trim_start_matches('\u{feff}').trim_start();
    if !text.get(..5)?.eq_ignore_ascii_case("<?php") {
        return None;
    }
    let rest = &text[5..];
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_end();
    Some(rest.strip_suffix("?>").unwrap_or(rest).trim())
}

fn unframe_raw(body: &str) -> Option<String> {
    let body = body.trim().replace("\r\n", "\n");
    if body.is_empty() {
        return None;
    }
    let unterminated = Code::new(&body)?.unterminated_line_comment();
    let mut text = body;
    if unterminated {
        text.push('\n');
    }
    Some(text)
}

/// Parses `keyword ws* ( inner ) tail`, returning `(inner, tail)`.
fn split_call<'a>(text: &'a str, keyword: &str) -> Option<(&'a str, &'a str)> {
    if !text.get(..keyword.len())?.eq_ignore_ascii_case(keyword) {
        return None;
    }
    let after = text[keyword.len()..].trim_start();
    if !after.starts_with('(') {
        return None;
    }
    let close = Code::new(after)?.matching_close(0)?;
    Some((&after[1..close], &after[close + 1..]))
}

fn compact(text: &str) -> String {
    text.split_whitespace().collect()
}

/// Final shaping of an extracted payload.
fn finish(inner: &str, strip_trailing_comma: bool) -> Option<String> {
    let mut text = dedent(inner)?;
    if strip_trailing_comma {
        let comma = Code::new(&text)?.trailing_comma();
        if let Some(comma) = comma {
            text.truncate(comma);
            text.truncate(text.trim_end().len());
        }
    }
    (!text.is_empty() && Code::new(&text)?.balanced()).then_some(text)
}

/// Trims blank lines around `text` and removes the indentation the frame added.
///
/// When the payload started on its own line (`(\n    a,\n    b\n)`), every line is dedented by the
/// smallest indentation; when it started right after the opening token, only the continuation
/// lines are measured. Lines starting inside a string/heredoc are left alone.
fn dedent(text: &str) -> Option<String> {
    let text = text.replace("\r\n", "\n");
    let code = Code::new(&text)?;
    let text = text.trim_end();

    let mut lines = Vec::new();
    let mut offset = 0;
    for line in text.split('\n') {
        lines.push((offset, line));
        offset += line.len() + 1;
    }
    let first = lines.iter().position(|(_, line)| !line.trim().is_empty())?;
    let starts_on_own_line = first > 0;
    let lines = &lines[first..];

    let protected = |start: usize| code.inside_literal(start);
    let indent_of = |line: &str| line.len() - line.trim_start_matches([' ', '\t']).len();
    let min_indent = lines
        .iter()
        .enumerate()
        .filter(|&(index, &(start, line))| {
            (starts_on_own_line || index > 0) && !line.trim().is_empty() && !protected(start)
        })
        .map(|(_, &(_, line))| indent_of(line))
        .min()
        .unwrap_or(0);

    let mut out = Vec::with_capacity(lines.len());
    for (index, &(start, line)) in lines.iter().enumerate() {
        out.push(if index == 0 && !starts_on_own_line {
            line.trim_start().to_owned()
        } else if protected(start) {
            line.to_owned()
        } else if line.trim().is_empty() {
            String::new()
        } else {
            line[min_indent.min(indent_of(line))..].to_owned()
        });
    }
    Some(out.join("\n"))
}

// ---------------------------------------------------------------------------------------------
// PHP-ish lexer

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RegionKind {
    /// `'…'`, `"…"`, `` `…` ``, heredoc, nowdoc (whole token including delimiters).
    Literal,
    /// `//` or `#` up to (excluding) the newline or `?>`.
    LineComment {
        newline_follows: bool,
    },
    BlockComment,
}

#[derive(Clone, Copy, Debug)]
struct Region {
    start: usize,
    end: usize,
    kind: RegionKind,
}

/// Source split into code and non-code regions. Only ASCII delimiters are inspected, so all
/// region boundaries are char boundaries.
struct Code<'a> {
    src: &'a str,
    regions: Vec<Region>,
    is_code: Vec<bool>,
}

impl<'a> Code<'a> {
    /// `None` when a string, heredoc or block comment is unterminated.
    fn new(src: &'a str) -> Option<Self> {
        let bytes = src.as_bytes();
        let mut regions = Vec::new();
        let mut is_code = vec![true; bytes.len()];
        let mut i = 0;
        while i < bytes.len() {
            match scan_region(src, i).ok()? {
                Some(region) => {
                    is_code[region.start..region.end].fill(false);
                    i = region.end;
                    regions.push(region);
                }
                None => i += 1,
            }
        }
        Some(Self {
            src,
            regions,
            is_code,
        })
    }

    fn literals(&self) -> impl Iterator<Item = &'a str> {
        self.regions
            .iter()
            .filter(|region| region.kind == RegionKind::Literal)
            .map(|region| &self.src[region.start..region.end])
    }

    /// Whether the line break just before `line_start` is inside a literal.
    fn inside_literal(&self, line_start: usize) -> bool {
        self.regions.iter().any(|region| {
            region.kind == RegionKind::Literal
                && region.start < line_start
                && line_start <= region.end
        })
    }

    fn unterminated_line_comment(&self) -> bool {
        self.regions.iter().any(|region| {
            region.kind
                == RegionKind::LineComment {
                    newline_follows: false,
                }
        })
    }

    fn code_bytes(&self) -> impl Iterator<Item = (usize, u8)> {
        self.src
            .bytes()
            .enumerate()
            .filter(|&(i, _)| self.is_code[i])
    }

    /// Offsets of `target` (not a bracket) outside any bracket pair.
    fn top_level(&self, target: u8) -> impl Iterator<Item = usize> {
        let mut depth = 0usize;
        self.code_bytes().filter_map(move |(i, byte)| match byte {
            b'(' | b'[' | b'{' => {
                depth += 1;
                None
            }
            b')' | b']' | b'}' => {
                depth = depth.saturating_sub(1);
                None
            }
            _ => (depth == 0 && byte == target).then_some(i),
        })
    }

    /// Offset of the `,` that is the last significant character (ignoring whitespace and comments), if any.
    fn trailing_comma(&self) -> Option<usize> {
        for (i, byte) in self.src.bytes().enumerate().rev() {
            if byte.is_ascii_whitespace() {
                continue;
            }
            if self.is_code[i] {
                return (byte == b',').then_some(i);
            }
            // The last token is a literal (not a comma); comments are skipped like whitespace.
            let in_comment = self.regions.iter().any(|region| {
                region.kind != RegionKind::Literal && region.start <= i && i < region.end
            });
            if !in_comment {
                return None;
            }
        }
        None
    }

    /// Offset of the bracket closing the one at `open`.
    fn matching_close(&self, open: usize) -> Option<usize> {
        let mut depth = 0usize;
        for (i, byte) in self.code_bytes().filter(|&(i, _)| i >= open) {
            match byte {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn balanced(&self) -> bool {
        let mut stack = Vec::new();
        for (_, byte) in self.code_bytes() {
            match byte {
                b'(' => stack.push(b')'),
                b'[' => stack.push(b']'),
                b'{' => stack.push(b'}'),
                b')' | b']' | b'}' if stack.pop() != Some(byte) => {
                    return false;
                }
                _ => {}
            }
        }
        stack.is_empty()
    }
}

/// `Err` = unterminated token, `Ok(None)` = `src[i]` does not start a region.
fn scan_region(src: &str, i: usize) -> Result<Option<Region>, ()> {
    let bytes = src.as_bytes();
    let region = |end, kind| {
        Ok(Some(Region {
            start: i,
            end,
            kind,
        }))
    };
    match bytes[i] {
        b'\'' | b'"' | b'`' => region(skip_php_string(src, i).ok_or(())?, RegionKind::Literal),
        b'/' if bytes.get(i + 1) == Some(&b'*') => match src[i + 2..].find("*/") {
            Some(position) => region(i + 2 + position + 2, RegionKind::BlockComment),
            None => Err(()),
        },
        b'/' if bytes.get(i + 1) == Some(&b'/') => Ok(Some(line_comment(src, i))),
        b'#' if bytes.get(i + 1) != Some(&b'[') => Ok(Some(line_comment(src, i))),
        b'<' if src[i..].starts_with("<<<") => heredoc(src, i),
        _ => Ok(None),
    }
}

/// Offset just behind the `'…'`, `"…"` or `` `…` `` string starting at `src[i]`, or `None` when it
/// is unterminated. Double-quoted and backtick strings honour `\` escapes and `{$…}` / `${…}`
/// interpolation, whose PHP code may itself contain quotes and `?>`. Shared with the parser's
/// `?>` finder so both agree on where a string ends.
pub(crate) fn skip_php_string(src: &str, i: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    let quote = bytes[i];
    let mut j = i + 1;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => j += 2,
            byte if byte == quote => return Some(j + 1),
            b'{' if quote != b'\'' && bytes.get(j + 1) == Some(&b'$') => {
                j = skip_braces(src, j + 1).ok()?;
            }
            b'$' if quote != b'\'' && bytes.get(j + 1) == Some(&b'{') => {
                j = skip_braces(src, j + 2).ok()?;
            }
            _ => j += 1,
        }
    }
    None
}

/// `from` is just after an opening `{`; returns the offset just after the matching `}`.
fn skip_braces(src: &str, from: usize) -> Result<usize, ()> {
    let bytes = src.as_bytes();
    let mut depth = 0usize;
    let mut j = from;
    while j < bytes.len() {
        if let Some(region) = scan_region(src, j)? {
            j = region.end;
            continue;
        }
        match bytes[j] {
            b'{' => depth += 1,
            b'}' => {
                if depth == 0 {
                    return Ok(j + 1);
                }
                depth -= 1;
            }
            _ => {}
        }
        j += 1;
    }
    Err(())
}

/// Ends at the newline, or at `?>` (which closes PHP mode), or at EOF.
fn line_comment(src: &str, i: usize) -> Region {
    let bytes = src.as_bytes();
    let mut j = i;
    while j < bytes.len() {
        if bytes[j] == b'\n' {
            return Region {
                start: i,
                end: j,
                kind: RegionKind::LineComment {
                    newline_follows: true,
                },
            };
        }
        if bytes[j] == b'?' && bytes.get(j + 1) == Some(&b'>') {
            break;
        }
        j += 1;
    }
    Region {
        start: i,
        end: j,
        kind: RegionKind::LineComment {
            newline_follows: false,
        },
    }
}

/// `<<<ID`, `<<<'ID'`, `<<<"ID"` through the line whose trimmed start is `ID` + non-word char.
fn heredoc(src: &str, i: usize) -> Result<Option<Region>, ()> {
    let bytes = src.as_bytes();
    let mut j = i + 3;
    while matches!(bytes.get(j), Some(b' ' | b'\t')) {
        j += 1;
    }
    let quote = match bytes.get(j) {
        Some(&q @ (b'\'' | b'"')) => {
            j += 1;
            Some(q)
        }
        _ => None,
    };
    let id_start = j;
    if !bytes
        .get(j)
        .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
    {
        return Ok(None);
    }
    while bytes.get(j).is_some_and(|b| is_word_byte(*b)) {
        j += 1;
    }
    let id = &src[id_start..j];
    if let Some(quote) = quote {
        if bytes.get(j) != Some(&quote) {
            return Ok(None);
        }
        j += 1;
    }
    if bytes.get(j) == Some(&b'\r') {
        j += 1;
    }
    if bytes.get(j) != Some(&b'\n') {
        return Ok(None);
    }

    let mut line_start = j + 1;
    loop {
        let line_end = src[line_start..]
            .find('\n')
            .map_or(bytes.len(), |p| line_start + p);
        let line = &src[line_start..line_end];
        let trimmed = line.trim_start_matches([' ', '\t']);
        if let Some(after) = trimmed.strip_prefix(id)
            && !after.bytes().next().is_some_and(is_word_byte)
        {
            return Ok(Some(Region {
                start: i,
                end: line_start + (line.len() - trimmed.len()) + id.len(),
                kind: RegionKind::Literal,
            }));
        }
        if line_end >= bytes.len() {
            return Err(());
        }
        line_start = line_end + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::BladeEchoKind;

    fn dir<'a>(name: &'a str, args: &'a str) -> BladePhpSnippet<'a> {
        BladePhpSnippet::Directive { name, args }
    }

    fn escaped(expr: &str) -> BladePhpSnippet<'_> {
        BladePhpSnippet::Echo {
            expr,
            kind: BladeEchoKind::Escaped,
        }
    }

    fn code_of(snippet: BladePhpSnippet) -> String {
        frame(&snippet).expect("framed").code
    }

    /// Frames `snippet`, "formats" with `formatted`, and unframes.
    fn round_trip(snippet: BladePhpSnippet, formatted: &str) -> Option<String> {
        unframe(&frame(&snippet)?, formatted)
    }

    // ----- frame -----

    #[test]
    fn frames_echo_and_attr() {
        assert_eq!(code_of(escaped(" $user->name ")), "<?php ($user->name);");
        assert_eq!(
            code_of(BladePhpSnippet::Attr("$count > 0")),
            "<?php ($count > 0);"
        );
    }

    #[test]
    fn frames_control_directives() {
        assert_eq!(code_of(dir("if", " $a && $b ")), "<?php if ($a && $b) {}");
        assert_eq!(code_of(dir("elseif", "$a")), "<?php if ($a) {}");
        assert_eq!(code_of(dir("UNLESS", "$a")), "<?php if ($a) {}");
        assert_eq!(
            code_of(dir("foreach", "$users as $user")),
            "<?php foreach ($users as $user) {}"
        );
        assert_eq!(
            code_of(dir("forelse", "$users as $user")),
            "<?php foreach ($users as $user) {}"
        );
        assert_eq!(
            code_of(dir("for", "$i = 0; $i < 3; $i++")),
            "<?php for ($i = 0; $i < 3; $i++) {}"
        );
        assert_eq!(code_of(dir("while", "$a")), "<?php while ($a) {}");
        assert_eq!(code_of(dir("switch", "$a")), "<?php switch ($a) {}");
        assert_eq!(
            code_of(dir("case", "'a'")),
            "<?php switch (1) { case 'a': }"
        );
    }

    #[test]
    fn frames_call_like_directives() {
        assert_eq!(code_of(dir("isset", "$a, $b")), "<?php isset($a, $b);");
        assert_eq!(code_of(dir("Empty", "$a")), "<?php empty($a);");
        assert_eq!(code_of(dir("unset", "$a")), "<?php unset($a);");
        assert_eq!(
            code_of(dir("json", "$a, JSON_PRETTY_PRINT")),
            "<?php json($a, JSON_PRETTY_PRINT);"
        );
        assert_eq!(code_of(dir("pushIf", "$a, 'x'")), "<?php pushIf($a, 'x');");
        assert_eq!(
            code_of(dir("each", "'v', $a, 'x'")),
            "<?php each('v', $a, 'x');"
        );
    }

    #[test]
    fn reserved_word_directives_use_neutral_callee() {
        assert_eq!(
            code_of(dir("include", "'x', ['a' => 1]")),
            "<?php blade_directive('x', ['a' => 1]);"
        );
        assert_eq!(code_of(dir("yield", "'x'")), "<?php blade_directive('x');");
        assert_eq!(
            code_of(dir("class", "['a']")),
            "<?php blade_directive(['a']);"
        );
        assert_eq!(code_of(dir("readonly", "$a")), "<?php blade_directive($a);");
    }

    #[test]
    fn frames_raw() {
        assert_eq!(
            code_of(BladePhpSnippet::Raw("\n    $a = 1;\n")),
            "<?php\n    $a = 1;\n"
        );
        assert_eq!(code_of(BladePhpSnippet::Raw(" $a = 1; ")), "<?php $a = 1; ");
        assert_eq!(code_of(BladePhpSnippet::Raw("$a = 1;")), "<?php $a = 1;");
    }

    #[test]
    fn opaque_or_unsafe_snippets_are_not_framed() {
        for snippet in [
            dir("use", "App\\Models\\User"),
            dir("verbatim", "x"),
            dir("php", "$a = 1"),
            dir("myCustom", "$a"),
            dir("livewire", "'x'"),
            dir("if", ""),
            dir("if", "   "),
            dir("if", "$a) { evil("),
            dir("json", "$a, 'unterminated"),
            dir("json", "$a // note"),
            dir("json", "'?>'"),
            dir("include::sub", "'x'"),
            escaped(""),
            escaped("$a) + (1"),
            escaped("$a // note"),
            BladePhpSnippet::Attr("  "),
            BladePhpSnippet::Raw("  \n "),
            BladePhpSnippet::Raw("$a = '?>';"),
            BladePhpSnippet::Raw("$a = 'unterminated;"),
        ] {
            assert!(frame(&snippet).is_none(), "{snippet:?}");
        }
    }

    // ----- unframe -----

    #[test]
    fn unframes_echo() {
        let echo = escaped("$user->name");
        assert_eq!(
            round_trip(echo, "<?php (\n$user->name\n);").as_deref(),
            Some("$user->name")
        );
        // parens dropped, blank line after the header, trailing newline
        assert_eq!(
            round_trip(echo, "<?php\n\n$user->name;\n").as_deref(),
            Some("$user->name")
        );
        // parens kept
        assert_eq!(
            round_trip(echo, "<?php ($user->name);").as_deref(),
            Some("$user->name")
        );
        // closing tag added
        assert_eq!(
            round_trip(echo, "<?php $user->name; ?>\n").as_deref(),
            Some("$user->name")
        );
        // case-insensitive header
        assert_eq!(
            round_trip(echo, "<?PHP $user->name;").as_deref(),
            Some("$user->name")
        );
    }

    #[test]
    fn unframe_keeps_inner_parens_of_echo() {
        let echo = escaped("($a) + ($b)");
        assert_eq!(
            round_trip(echo, "<?php\n($a) + ($b);\n").as_deref(),
            Some("($a) + ($b)")
        );
        let echo = escaped("($a ?? $b)");
        assert_eq!(
            round_trip(echo, "<?php\n$a ?? $b;\n").as_deref(),
            Some("$a ?? $b")
        );
    }

    #[test]
    fn unframes_multiline_echo() {
        let echo = escaped("route('home', ['a' => 1])");
        let formatted = "<?php\n\nroute(\n    'home',\n    [\n        'a' => 1,\n    ],\n);\n";
        assert_eq!(
            round_trip(echo, formatted).as_deref(),
            Some("route(\n    'home',\n    [\n        'a' => 1,\n    ],\n)")
        );
        // parens kept by the formatter around a multi-line expression
        let formatted = "<?php\n(\n    foo(\n        1\n    )\n);\n";
        assert_eq!(
            round_trip(escaped("foo(1)"), formatted).as_deref(),
            Some("foo(\n    1\n)")
        );
    }

    #[test]
    fn unframe_rejects_bad_echo_shapes() {
        let framed = frame(&escaped("$a")).unwrap();
        for formatted in [
            "",
            "$a;",
            "<?php\n$a;\n$b;\n",
            "<?php\n$a; // trailing\n",
            "<?php\n$a\n",
            "<?phpx $a;",
            "<?php\nfunction f() { $a; }\n",
            "<html>$a;",
        ] {
            let result = unframe(&framed, formatted);
            assert!(result.is_none(), "{formatted:?} -> {result:?}");
        }
    }

    #[test]
    fn unframes_if_family() {
        let snippet = dir("if", "$a");
        assert_eq!(
            round_trip(snippet, "<?php if ($a) {}").as_deref(),
            Some("$a")
        );
        assert_eq!(
            round_trip(snippet, "<?php\n\nif ($a) {\n}\n").as_deref(),
            Some("$a")
        );
        assert_eq!(
            round_trip(snippet, "<?php\nIF ($a) { }\n").as_deref(),
            Some("$a")
        );
        let snippet = dir("unless", "$a && ($b || $c)");
        assert_eq!(
            round_trip(snippet, "<?php\n\nif ($a && ($b || $c)) {\n}\n").as_deref(),
            Some("$a && ($b || $c)")
        );
    }

    #[test]
    fn unframes_wrapped_condition() {
        let snippet = dir("if", "$user->isAdmin() && $user->active");
        assert_eq!(
            round_trip(
                snippet,
                "<?php\n\nif (\n    $user->isAdmin() &&\n    $user->active\n) {\n}\n"
            )
            .as_deref(),
            Some("$user->isAdmin() &&\n$user->active")
        );
        // continuation indent only (first line right after the paren)
        assert_eq!(
            round_trip(
                snippet,
                "<?php\nif ($user->isAdmin() &&\n    $user->active\n) {\n}\n"
            )
            .as_deref(),
            Some("$user->isAdmin() &&\n$user->active")
        );
        // nested structure keeps its relative indentation
        let snippet = dir("if", "in_array($a, [1, 2])");
        assert_eq!(
            round_trip(
                snippet,
                "<?php\nif (\n    in_array(\n        $a,\n        [1, 2],\n    )\n) {\n}\n"
            )
            .as_deref(),
            Some("in_array(\n    $a,\n    [1, 2],\n)")
        );
    }

    #[test]
    fn unframes_loops_and_switch() {
        let snippet = dir("foreach", "$users as $user");
        assert_eq!(
            round_trip(snippet, "<?php\n\nforeach ($users as $user) {\n}\n").as_deref(),
            Some("$users as $user")
        );
        let snippet = dir("forelse", "$users as $k=>$v");
        assert_eq!(
            round_trip(snippet, "<?php\nforeach ($users as $k => $v) {\n}\n").as_deref(),
            Some("$users as $k => $v")
        );
        let snippet = dir("for", "$i=0;$i<3;$i++");
        assert_eq!(
            round_trip(snippet, "<?php\nfor ($i = 0; $i < 3; $i++) {\n}\n").as_deref(),
            Some("$i = 0; $i < 3; $i++")
        );
        let snippet = dir("while", "$a");
        assert_eq!(
            round_trip(snippet, "<?php\nwhile ($a) {\n}\n").as_deref(),
            Some("$a")
        );
        let snippet = dir("switch", "$a");
        assert_eq!(
            round_trip(snippet, "<?php\nswitch ($a) {\n}\n").as_deref(),
            Some("$a")
        );
    }

    #[test]
    fn unframes_case() {
        let snippet = dir("case", "'a'");
        assert_eq!(
            round_trip(snippet, "<?php\n\nswitch (1) {\n    case 'a':\n}\n").as_deref(),
            Some("'a'")
        );
        let snippet = dir("case", "Foo::BAR");
        assert_eq!(
            round_trip(snippet, "<?php switch (1) {\n    case Foo::BAR:\n}").as_deref(),
            Some("Foo::BAR")
        );
        assert!(round_trip(snippet, "<?php\nswitch (2) {\n    case Foo::BAR:\n}\n").is_none());
        assert!(round_trip(snippet, "<?php\nswitch (1) {\n    default:\n}\n").is_none());
    }

    #[test]
    fn unframes_call_like_directives() {
        let snippet = dir("isset", "$a,$b");
        assert_eq!(
            round_trip(snippet, "<?php\nisset($a, $b);\n").as_deref(),
            Some("$a, $b")
        );
        let snippet = dir("json", "$a");
        assert_eq!(
            round_trip(snippet, "<?php\njson($a);\n").as_deref(),
            Some("$a")
        );
        let snippet = dir("include", "'x', ['a'=>1]");
        assert_eq!(
            round_trip(snippet, "<?php\nblade_directive('x', ['a' => 1]);\n").as_deref(),
            Some("'x', ['a' => 1]")
        );
        assert!(round_trip(snippet, "<?php\nblade_directive('x'); foo();\n").is_none());
        assert!(round_trip(snippet, "<?php\nother('x');\n").is_none());
    }

    #[test]
    fn unframe_strips_trailing_comma_added_by_wrapping() {
        let snippet = dir("include", "'x', ['a'=>1]");
        let formatted = "<?php\n\nblade_directive(\n    'x',\n    ['a' => 1],\n);\n";
        assert_eq!(
            round_trip(snippet, formatted).as_deref(),
            Some("'x',\n['a' => 1]")
        );
        // a comma that was in the source is kept
        let snippet = dir("include", "'x', ['a'=>1],");
        assert_eq!(
            round_trip(snippet, formatted).as_deref(),
            Some("'x',\n['a' => 1],")
        );
    }

    #[test]
    fn unframe_does_not_dedent_inside_literals() {
        let snippet = dir("json", "'p', 'x\n   y'");
        let formatted = "<?php\n\njson(\n    'p',\n    'x\n   y',\n);\n";
        assert_eq!(
            round_trip(snippet, formatted).as_deref(),
            Some("'p',\n'x\n   y'")
        );
        let snippet = dir("json", "<<<EOT\n  text\n  EOT");
        let formatted = "<?php\njson(\n    <<<EOT\n  text\n  EOT,\n);\n";
        assert_eq!(
            round_trip(snippet, formatted).as_deref(),
            Some("<<<EOT\n  text\n  EOT")
        );
    }

    #[test]
    fn unframes_raw() {
        let snippet = BladePhpSnippet::Raw("\n    declare(strict_types=1);\n    $a=1;\n");
        assert_eq!(
            round_trip(snippet, "<?php\n\ndeclare(strict_types=1);\n\n$a = 1;\n").as_deref(),
            Some("declare(strict_types=1);\n\n$a = 1;")
        );
        // closing tag added by the formatter
        assert_eq!(
            round_trip(snippet, "<?php\n$a = 1;\n?>\n").as_deref(),
            Some("$a = 1;")
        );
        // a trailing line comment keeps a newline so the closer is on its own line
        let snippet = BladePhpSnippet::Raw(" $a = 1; // set ");
        assert_eq!(
            round_trip(snippet, "<?php\n$a = 1; // set\n").as_deref(),
            Some("$a = 1; // set\n")
        );
        assert!(round_trip(snippet, "$a = 1;").is_none());
        assert!(round_trip(snippet, "<?php\n").is_none());
    }

    // ----- postconditions -----

    #[test]
    fn rejects_foreach_without_single_spaced_as() {
        let snippet = dir("foreach", "$users as $user");
        assert!(postconditions_hold(
            &snippet,
            "$users as $user",
            "$users as $user"
        ));
        assert!(postconditions_hold(
            &snippet,
            "$users  as  $user",
            "$users as $user"
        ));
        assert!(!postconditions_hold(
            &snippet,
            "$users as $user",
            "$users\nas $user"
        ));
        assert!(!postconditions_hold(
            &snippet,
            "$users as $user",
            "$users as\n$user"
        ));
        assert!(!postconditions_hold(
            &snippet,
            "$users as $user",
            "$users  as $user"
        ));
        assert!(!postconditions_hold(
            &snippet,
            "$users as $user",
            "$users as  $user"
        ));
        assert!(!postconditions_hold(
            &snippet,
            "$users as $user",
            "$users\tas $user"
        ));
        assert!(!postconditions_hold(&snippet, "$users as $user", "$users"));
        assert!(!postconditions_hold(
            &snippet,
            "$users as $user",
            " as $user"
        ));
        let snippet = dir("forelse", "$users as $user");
        assert!(!postconditions_hold(
            &snippet,
            "$users as $user",
            "$users\n    as $user"
        ));
    }

    #[test]
    fn rejects_lang_changing_char_after_paren() {
        let snippet = dir("lang", "['a' => 'b']");
        assert!(postconditions_hold(&snippet, "['a'=>'b']", "['a' => 'b']"));
        let spaced = dir("lang", " ['a' => 'b']");
        assert!(!postconditions_hold(
            &spaced,
            " ['a' => 'b']",
            "['a' => 'b']"
        ));
        let key = dir("lang", "'key'");
        assert!(!postconditions_hold(&key, "'key'", "['key']"));
        // quote style may change when both forms denote the same bytes
        assert!(postconditions_hold(&key, "\"key\"", "'key'"));
    }

    #[test]
    fn rejects_comma_changes_for_comma_splitting_directives() {
        let snippet = dir("json", "$a, JSON_PRETTY_PRINT");
        assert!(postconditions_hold(
            &snippet,
            "$a,JSON_PRETTY_PRINT",
            "$a, JSON_PRETTY_PRINT"
        ));
        // trailing comma added
        assert!(!postconditions_hold(&snippet, "$a, $b", "$a, $b,"));
        // trailing comma already in source is fine
        assert!(postconditions_hold(&snippet, "$a, $b,", "$a, $b,"));
        // comma moved into / out of a nested array changes the textual split
        assert!(!postconditions_hold(&snippet, "['a', 'b']", "['a', 'b',]"));
        assert!(!postconditions_hold(&snippet, "$a, [1, 2]", "$a, [1]"));
        for name in [
            "js",
            "pushIf",
            "elsePushIf",
            "elsePush",
            "inject",
            "pushOnce",
            "prependOnce",
            "each",
            "section",
            "push",
            "prepend",
            "slot",
        ] {
            let snippet = dir(name, "$a, $b");
            assert!(
                !postconditions_hold(&snippet, "$a, $b", "$a, $b,"),
                "{name}"
            );
        }
        // other directives may gain no trailing comma through unframe, but are not restricted here
        let include = dir("include", "'x', $b");
        assert!(postconditions_hold(&include, "'x', $b", "'x', $b"));
    }

    #[test]
    fn rejects_closing_delimiters() {
        let echo = escaped("$a");
        assert!(postconditions_hold(&echo, "$a", "$a"));
        assert!(!postconditions_hold(&echo, "$a", "$a }}"));
        assert!(!postconditions_hold(&echo, "$a", "$a !!}"));
        assert!(!postconditions_hold(&echo, "$a", "$a --}}"));
        assert!(!postconditions_hold(&echo, "$a", "$a @endphp"));
        assert!(!postconditions_hold(&echo, "$a", "$a ?>"));
        let directive = dir("if", "$a");
        assert!(!postconditions_hold(&directive, "$a", "[$a]}}"));
        // a delimiter that was already in the source (inside a string) is not new
        let echo = escaped("'!!}'");
        assert!(postconditions_hold(&echo, "'!!}'", "'!!}'"));
        let echo = escaped("f('?>')");
        assert!(!postconditions_hold(&echo, "f('?>')", "f('?>')"));
    }

    #[test]
    fn raw_allows_closing_tag_only_if_original_had_it() {
        let raw = BladePhpSnippet::Raw("$a = '?>';");
        assert!(postconditions_hold(&raw, "$a = '?>';", "$a = '?>';"));
        assert!(!postconditions_hold(&raw, "$a = 1;", "$a = 1; ?>"));
        assert!(!postconditions_hold(&raw, "$a = '?>';", "$a = '?>'; ?>"));
        assert!(!postconditions_hold(&raw, "$a = 1;", "$a = 1; // @endphp"));
        assert!(postconditions_hold(
            &raw,
            "if (a) { if (b) {}}",
            "if (a) {\n    if (b) {\n    }\n}"
        ));
    }

    #[test]
    fn rejects_new_directive_tokens() {
        let echo = escaped("$a");
        assert!(!postconditions_hold(&echo, "$a", "$a ? '@csrf' : ''"));
        assert!(!postconditions_hold(&echo, "$a", "@foo($a)"));
        // `\B`: inside a word it is not a directive
        assert!(postconditions_hold(&echo, "$a", "$a user@example"));
        let raw = BladePhpSnippet::Raw("/** @var int $a */\n$a = 1;");
        assert!(postconditions_hold(
            &raw,
            "/** @var int $a */\n$a = 1;",
            "/** @var int $a */\n$a = 1;"
        ));
        assert!(!postconditions_hold(
            &raw,
            "/** @var int $a */\n$a = 1;",
            "/** @var int $a @param */\n$a = 1;"
        ));
        // suppression operator that is already in the source is fine
        let echo = escaped("@foo()");
        assert!(postconditions_hold(&echo, "@foo()", "@foo()"));
        assert!(!postconditions_hold(&echo, "@foo()", "@foo() + @foo()"));
    }

    #[test]
    fn rejects_unterminated_line_comments() {
        let echo = escaped("$a");
        assert!(!postconditions_hold(&echo, "$a", "$a // note"));
        assert!(!postconditions_hold(&echo, "$a", "$a # note"));
        assert!(postconditions_hold(
            &echo,
            "$a",
            "foo(\n    $a, // note\n    $b\n)"
        ));
        assert!(postconditions_hold(&echo, "$a", "$a #[Attr]"));
        let raw = BladePhpSnippet::Raw("$a = 1; // set");
        assert!(!postconditions_hold(
            &raw,
            "$a = 1; // set",
            "$a = 1; // set"
        ));
        assert!(postconditions_hold(
            &raw,
            "$a = 1; // set",
            "$a = 1; // set\n"
        ));
    }

    #[test]
    fn rejects_changed_literals() {
        let echo = escaped("route('home')");
        assert!(postconditions_hold(
            &echo,
            "route( 'home' )",
            "route('home')"
        ));
        assert!(!postconditions_hold(
            &echo,
            "route('home')",
            "route('Home')"
        ));
        assert!(!postconditions_hold(
            &echo,
            "route('home')",
            "route(\"hom\")"
        ));
        assert!(!postconditions_hold(
            &echo,
            "route('home')",
            "route(\"Home\")"
        ));
        assert!(!postconditions_hold(&echo, "route('home')", "route()"));
        assert!(!postconditions_hold(
            &echo,
            "route('home')",
            "route('home', 'x')"
        ));
        assert!(!postconditions_hold(
            &echo,
            "f('a', \"b\")",
            "f(\"b\", 'a')"
        ));
        // multi-line strings
        assert!(!postconditions_hold(&echo, "f('a\n b')", "f('a\n    b')"));
        // heredoc / nowdoc
        let heredoc = "f(<<<EOT\n  text $x\n  EOT)";
        assert!(postconditions_hold(
            &echo,
            heredoc,
            "f(<<<EOT\n  text $x\n  EOT\n)"
        ));
        assert!(!postconditions_hold(
            &echo,
            heredoc,
            "f(<<<EOT\n    text $x\n    EOT\n)"
        ));
        let nowdoc = "f(<<<'EOT'\ntext\nEOT)";
        assert!(postconditions_hold(
            &echo,
            nowdoc,
            "f(<<<'EOT'\ntext\nEOT\n)"
        ));
        assert!(!postconditions_hold(
            &echo,
            nowdoc,
            "f(<<<'EOT'\ntexts\nEOT\n)"
        ));
        // interpolation with nested quotes is one literal
        let nested = r#"f("a {$b["c"]} d")"#;
        assert!(postconditions_hold(&echo, nested, nested));
        assert!(!postconditions_hold(&echo, nested, r#"f("a {$b['c']} d")"#));
        // flipping the quotes of a literal with escapes, interpolation or quotes is not allowed
        assert!(!postconditions_hold(&echo, r"f('a\n')", r#"f("a\n")"#));
        assert!(!postconditions_hold(&echo, "f('$a')", "f(\"$a\")"));
        assert!(!postconditions_hold(&echo, "f('a\"b')", "f(\"a\"b\")"));
        assert!(!postconditions_hold(&echo, "f(`a`)", "f('a')"));
        // heredoc delimiters and bodies stay byte-identical
        assert!(!postconditions_hold(
            &echo,
            "f(<<<'EOT'\ntext\nEOT)",
            "f(<<<EOT\ntext\nEOT\n)"
        ));
        // unterminated source or result
        assert!(!postconditions_hold(&echo, "f('a)", "f('a)"));
        assert!(!postconditions_hold(&echo, "f('a')", "f('a)"));
    }

    #[test]
    fn accepts_quote_flip_of_simple_literals() {
        let echo = escaped("route('home')");
        assert!(postconditions_hold(
            &echo,
            "route('home')",
            "route(\"home\")"
        ));
        assert!(postconditions_hold(
            &echo,
            "f(\"a b\", 'c')",
            "f('a b', \"c\")"
        ));
        assert!(postconditions_hold(&echo, "f('')", "f(\"\")"));
        assert!(!postconditions_hold(&echo, "f('home')", "f(\"hom\")"));
    }

    #[test]
    fn trailing_comma_ignores_commas_inside_a_last_literal() {
        let code = Code::new("'p', 'x,\n y'").unwrap();
        assert_eq!(code.trailing_comma(), None);
        let code = Code::new("'p', 'y', // note\n").unwrap();
        assert_eq!(code.trailing_comma(), Some(8));
        let code = Code::new("'p', 'y',").unwrap();
        assert_eq!(code.trailing_comma(), Some(8));
    }

    #[test]
    fn lexer_ignores_quotes_in_comments() {
        let echo = escaped("f(1)");
        let source = "f(/* it's */ 1, // don't\n 2)";
        assert!(postconditions_hold(
            &echo,
            source,
            "f(\n    /* it's */ 1, // don't\n    2\n)"
        ));
    }

    // ----- width -----

    #[test]
    fn width_overhead() {
        let echo = |kind| BladePhpSnippet::Echo { expr: "$a", kind };
        assert_eq!(print_width_overhead(&echo(BladeEchoKind::Escaped)), 6);
        assert_eq!(print_width_overhead(&echo(BladeEchoKind::Raw)), 8);
        assert_eq!(print_width_overhead(&echo(BladeEchoKind::Triple)), 8);
        assert_eq!(print_width_overhead(&BladePhpSnippet::Attr("$a")), 2);
        assert_eq!(print_width_overhead(&dir("include", "'x'")), 10);
        assert_eq!(print_width_overhead(&BladePhpSnippet::Raw("$a;")), 0);
    }

    // ----- literal postcondition -----

    #[test]
    fn crlf_inside_a_literal_is_not_normalized() {
        let raw = BladePhpSnippet::Raw("");
        let crlf = "$s = \"a\r\nb\";";
        let lf = "$s = \"a\nb\";";
        assert!(postconditions_hold(&raw, crlf, crlf));
        assert!(postconditions_hold(&raw, lf, lf));
        assert!(!postconditions_hold(&raw, crlf, lf));
        assert!(!postconditions_hold(&raw, lf, crlf));

        let heredoc_crlf = "$s = <<<EOT\r\nx\r\ny\r\nEOT;";
        let heredoc_lf = "$s = <<<EOT\nx\ny\nEOT;";
        assert!(postconditions_hold(&raw, heredoc_crlf, heredoc_crlf));
        assert!(!postconditions_hold(&raw, heredoc_crlf, heredoc_lf));

        // Line breaks between tokens are layout, not literal content.
        assert!(postconditions_hold(
            &raw,
            "$a = 1;\r\n$b = 'x';",
            "$a = 1;\n$b = 'x';"
        ));
    }
}
