//! Blade-specific parsing: the raw-span pre-pass, directive/echo/PHP dispatch and attribute parsing.
//!
//! Everything here mirrors what Laravel's `BladeCompiler` does to a template, in the same order:
//! `@verbatim`/`@php` spans are extracted first, then comments, then PHP tags/echoes/directives
//! are recognized in the remaining text. Whitespace is never a node of its own: it stays inside the
//! `Text` nodes, so the printer can tell whether a directive or echo is adjacent to its neighbours.

use super::{PResult, Parser, is_attr_name_char, is_html_tag_name_char};
use crate::{
    ast::*,
    blade::{
        BladeRegistry, DirectiveRole, DirectiveToken, ParenIndex, php::skip_php_string,
        scan_directive,
    },
    error::{SyntaxError, SyntaxErrorKind},
    helpers,
};
use std::{
    cell::{Cell, RefCell},
    mem,
};

const VERBATIM_OPEN: &str = "@verbatim";
const VERBATIM_CLOSE: &str = "@endverbatim";
const PHP_OPEN: &str = "@php";
const PHP_CLOSE: &str = "@endphp";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RawSpanKind {
    Verbatim,
    Php,
}

/// A region the compiler extracts before it looks at anything else.
#[derive(Clone, Copy, Debug)]
pub(super) struct RawSpan {
    pub start: usize,
    pub end: usize,
    pub kind: RawSpanKind,
}

impl RawSpan {
    fn label(&self) -> &'static str {
        match self.kind {
            RawSpanKind::Verbatim => "@verbatim block",
            RawSpanKind::Php => "@php block",
        }
    }
}

#[derive(Clone, Copy)]
enum Closer {
    Raw,
    Triple,
    Escaped,
    Comment,
}

impl Closer {
    fn text(self) -> &'static str {
        match self {
            Closer::Raw => "!!}",
            Closer::Triple => "}}}",
            Closer::Escaped => "}}",
            Closer::Comment => "--}}",
        }
    }
}

/// Memo for searches that fail by running to the end of the source. A closer missing from
/// `src[from..]` is missing from every later start too, and a `(` that the failed bracket scan
/// already passed has its answer. Without it an input of `{{ ` or `@if(` repeated and never closed
/// rescans the remaining source for each opener.
#[derive(Default)]
pub(super) struct ScanCache {
    /// Per [`Closer`]: the smallest `from` known to have no closer at or after it.
    closer_missing_from: [Cell<Option<usize>>; 4],
    parens: RefCell<ParenIndex>,
}

impl ScanCache {
    /// Offset of the first `close` in `src[from..]`.
    fn find_closer(&self, src: &str, close: Closer, from: usize) -> Option<usize> {
        let missing_from = &self.closer_missing_from[close as usize];
        if missing_from.get().is_some_and(|missing| from >= missing) {
            return None;
        }
        let found = src[from..].find(close.text()).map(|rel| from + rel);
        if found.is_none() {
            missing_from.set(Some(from));
        }
        found
    }
}

/// Finds the raw spans the way `storeUncompiledBlocks` does: `@verbatim` first on the original
/// text, then `@php` on what is left, where every verbatim span is a single opaque placeholder
/// that ends with `@` (so an `@php` right behind it fails the `(?<!@)` lookbehind).
pub(super) fn find_raw_spans(src: &str) -> Vec<RawSpan> {
    if !src.contains(VERBATIM_OPEN) && !src.contains(PHP_OPEN) {
        return vec![];
    }
    let bytes = src.as_bytes();

    let mut verbatim = vec![];
    let mut pos = 0;
    while let Some(rel) = src[pos..].find(VERBATIM_OPEN) {
        let start = pos + rel;
        if start > 0 && bytes[start - 1] == b'@' {
            pos = start + 1;
            continue;
        }
        let body_from = start + VERBATIM_OPEN.len();
        let Some(rel_end) = src[body_from..].find(VERBATIM_CLOSE) else {
            break;
        };
        let end = body_from + rel_end + VERBATIM_CLOSE.len();
        verbatim.push(RawSpan {
            start,
            end,
            kind: RawSpanKind::Verbatim,
        });
        pos = end;
    }

    // `start <= at < end` for the verbatim span covering `at`.
    let covering = |at: usize| {
        let idx = verbatim.partition_point(|span| span.start <= at);
        idx.checked_sub(1)
            .map(|idx| verbatim[idx])
            .filter(|span| at < span.end)
    };

    let mut php: Vec<RawSpan> = vec![];
    let mut pos = 0;
    while let Some(rel) = src[pos..].find(PHP_OPEN) {
        let start = pos + rel;
        if let Some(span) = covering(start) {
            pos = span.end;
            continue;
        }
        let behind_placeholder = verbatim
            .binary_search_by_key(&start, |span| span.end)
            .is_ok();
        if behind_placeholder || (start > 0 && bytes[start - 1] == b'@') {
            pos = start + 1;
            continue;
        }
        let mut from = start + PHP_OPEN.len();
        let end = loop {
            let Some(rel) = src[from..].find(PHP_CLOSE) else {
                break None;
            };
            let at = from + rel;
            if let Some(span) = covering(at) {
                from = span.end;
                continue;
            }
            break Some(at + PHP_CLOSE.len());
        };
        let Some(end) = end else {
            break;
        };
        php.push(RawSpan {
            start,
            end,
            kind: RawSpanKind::Php,
        });
        pos = end;
    }

    let mut spans = php.clone();
    spans.extend(verbatim.into_iter().filter(|span| {
        let idx = php.partition_point(|outer| outer.start <= span.start);
        !idx.checked_sub(1)
            .is_some_and(|idx| span.end <= php[idx].end)
    }));
    spans.sort_unstable_by_key(|span| span.start);
    spans
}

/// `<?php` (ASCII case-insensitive, followed by whitespace or the end) or `<?=`.
fn php_open(rest: &str) -> Option<(usize, BladePhpKind)> {
    let bytes = rest.as_bytes();
    if bytes.starts_with(b"<?=") {
        Some((3, BladePhpKind::EchoTag))
    } else if bytes.len() >= 5
        && bytes[..5].eq_ignore_ascii_case(b"<?php")
        && bytes.get(5).is_none_or(|byte| byte.is_ascii_whitespace())
    {
        Some((5, BladePhpKind::Tag))
    } else {
        None
    }
}

/// Lexes PHP from `from` (just behind the opening tag).
///
/// Returns `(code_end, after, closed)`: the offset of `?>` (or the end of the source when the
/// tag is never closed) and the offset behind it. Strings, backticks, heredocs/nowdocs and block
/// comments hide a `?>`; `//` and `#` comments end at the line break *or* at a `?>`.
fn php_tag_end(src: &str, from: usize) -> (usize, usize, bool) {
    let bytes = src.as_bytes();
    let len = bytes.len();
    let mut i = from;
    while i < len {
        match bytes[i] {
            b'?' if bytes.get(i + 1) == Some(&b'>') => return (i, i + 2, true),
            b'\'' | b'"' | b'`' => match skip_php_string(src, i) {
                Some(end) => i = end,
                None => return (len, len, false),
            },
            b'/' if bytes.get(i + 1) == Some(&b'*') => match src[i + 2..].find("*/") {
                Some(rel) => i += 2 + rel + 2,
                None => return (len, len, false),
            },
            b'/' if bytes.get(i + 1) == Some(&b'/') => i = php_line_comment_end(bytes, i + 2),
            // `#[` starts an attribute, not a comment.
            b'#' if bytes.get(i + 1) != Some(&b'[') => i = php_line_comment_end(bytes, i + 1),
            b'<' if bytes[i..].starts_with(b"<<<") => match php_heredoc(bytes, i) {
                Heredoc::Ends(end) => i = end,
                Heredoc::Unterminated => return (len, len, false),
                Heredoc::None => i += 3,
            },
            _ => i += 1,
        }
    }
    (len, len, false)
}

/// Offset of the line break or `?>` that ends a line comment whose text starts at `from`.
fn php_line_comment_end(bytes: &[u8], from: usize) -> usize {
    let mut j = from;
    while j < bytes.len() {
        if bytes[j] == b'\n' || (bytes[j] == b'?' && bytes.get(j + 1) == Some(&b'>')) {
            return j;
        }
        j += 1;
    }
    bytes.len()
}

enum Heredoc {
    None,
    Unterminated,
    /// Offset just behind the closing label.
    Ends(usize),
}

fn is_php_label_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
}

/// `at` points at `<<<`: `<<<ID`, `<<<'ID'` or `<<<"ID"` and a line break, then lines up to the
/// first one whose indentation-trimmed start is the label not followed by a label char.
fn php_heredoc(bytes: &[u8], at: usize) -> Heredoc {
    let mut j = at + 3;
    while matches!(bytes.get(j), Some(b' ' | b'\t')) {
        j += 1;
    }
    let quote = match bytes.get(j) {
        Some(&quote @ (b'\'' | b'"')) => {
            j += 1;
            Some(quote)
        }
        _ => None,
    };
    let label_start = j;
    while bytes.get(j).is_some_and(|&byte| is_php_label_byte(byte)) {
        j += 1;
    }
    if j == label_start || bytes[label_start].is_ascii_digit() {
        return Heredoc::None;
    }
    let label = &bytes[label_start..j];
    if let Some(quote) = quote {
        if bytes.get(j) != Some(&quote) {
            return Heredoc::None;
        }
        j += 1;
    }
    match bytes.get(j) {
        Some(b'\r') => {
            j += 1;
            if bytes.get(j) == Some(&b'\n') {
                j += 1;
            }
        }
        Some(b'\n') => j += 1,
        _ => return Heredoc::None,
    }

    let mut line = j;
    while line < bytes.len() {
        let mut k = line;
        while matches!(bytes.get(k), Some(b' ' | b'\t')) {
            k += 1;
        }
        if bytes[k..].starts_with(label)
            && !bytes
                .get(k + label.len())
                .is_some_and(|&byte| is_php_label_byte(byte))
        {
            return Heredoc::Ends(k + label.len());
        }
        match bytes[line..].iter().position(|&byte| byte == b'\n') {
            Some(rel) => line += rel + 1,
            None => break,
        }
    }
    Heredoc::Unterminated
}

/// `<x-slot>`, `<x-slot:name>` and the `x:` spellings.
fn is_slot_tag(name: &str) -> bool {
    ["x-slot", "x:slot"].into_iter().any(|prefix| {
        name.strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(':'))
    })
}

/// The compiler rewrites slots first, with any `</x-slot…>`/`</x:slot…>`
/// (`/<\/\s*x[\-\:]slot[^>]*>/`), and then every remaining component closer
/// (`/<\/\s*x[\-\:][\w\-\:\.]*\s*>/`) without looking at the name.
fn is_slot_closer(close: &str) -> bool {
    close.starts_with("x-slot") || close.starts_with("x:slot")
}

/// Whether `</close>` ends the element `<open>`.
pub(super) fn closes_tag(open: &str, close: &str) -> bool {
    if open.contains('{') || close.contains('{') {
        // Echoes are compared by source text.
        return open == close;
    }
    if open.eq_ignore_ascii_case(close) {
        return true;
    }
    if !helpers::is_blade_component(open) || !helpers::is_blade_component(close) {
        return false;
    }
    is_slot_tag(open) == is_slot_closer(close)
}

/// A `{{ }}`, `{!! !!}` or `{{{ }}}` echo; offsets are absolute.
struct EchoSpan {
    kind: BladeEchoKind,
    /// Start of the text between the delimiters.
    start: usize,
    /// End of the text between the delimiters.
    end: usize,
    /// Offset just behind the closing delimiter.
    after: usize,
}

/// What an `@` at some position stands for.
pub(super) enum At<'s> {
    /// Plain text (an escape, an unknown name, a lone `@`); the number of bytes it covers.
    Text(usize),
    /// A directive the registry knows.
    Directive {
        directive: BladeDirective<'s>,
        end: usize,
        role: DirectiveRole,
    },
}

/// Generic over nodes and attributes, like `HasJinjaFlowControl`.
pub(super) trait HasBladeFlowControl<'s>: Sized {
    type Intermediate;

    fn from_directive(directive: BladeDirective<'s>) -> Self::Intermediate;
    fn from_block(block: BladeBlock<'s, Self>) -> Self::Intermediate;

    /// Attribute lists are separated by whitespace which isn't a child.
    fn skip_ws_between_children() -> bool;
}

impl<'s> HasBladeFlowControl<'s> for Node<'s> {
    type Intermediate = NodeKind<'s>;

    fn from_directive(directive: BladeDirective<'s>) -> Self::Intermediate {
        NodeKind::BladeDirective(directive)
    }

    fn from_block(block: BladeBlock<'s, Self>) -> Self::Intermediate {
        NodeKind::BladeBlock(block)
    }

    fn skip_ws_between_children() -> bool {
        false
    }
}

impl<'s> HasBladeFlowControl<'s> for Attribute<'s> {
    type Intermediate = Attribute<'s>;

    fn from_directive(directive: BladeDirective<'s>) -> Self::Intermediate {
        Attribute::BladeDirective(directive)
    }

    fn from_block(block: BladeBlock<'s, Self>) -> Self::Intermediate {
        Attribute::BladeBlock(block)
    }

    fn skip_ws_between_children() -> bool {
        true
    }
}

fn build_directive<'s>(src: &'s str, token: DirectiveToken) -> (BladeDirective<'s>, usize) {
    let DirectiveToken {
        name,
        space,
        args,
        end,
    } = token;
    (
        BladeDirective {
            name: &src[name],
            space: &src[space],
            args: args.map(|args| (&src[args.clone()], args.start)),
        },
        end,
    )
}

impl<'s> Parser<'s> {
    fn registry(&self) -> &BladeRegistry {
        self.blade
            .as_ref()
            .expect("the Blade registry exists whenever Blade is parsed")
    }

    /// Consumes everything before `pos`.
    fn advance_to(&mut self, pos: usize) {
        while self.chars.next_if(|&(i, _)| i < pos).is_some() {}
    }

    // ---- raw spans ----

    fn blade_span_at(&self, pos: usize) -> Option<RawSpan> {
        self.blade_spans
            .binary_search_by_key(&pos, |span| span.start)
            .ok()
            .map(|idx| self.blade_spans[idx])
    }

    /// The span with `start < pos < end`, i.e. one whose start was skipped over.
    fn blade_span_containing(&self, pos: usize) -> Option<RawSpan> {
        let idx = self.blade_spans.partition_point(|span| span.start < pos);
        idx.checked_sub(1)
            .map(|idx| self.blade_spans[idx])
            .filter(|span| pos < span.end)
    }

    /// The span with `start <= pos < end`.
    fn blade_span_covering(&self, pos: usize) -> Option<RawSpan> {
        let idx = self.blade_spans.partition_point(|span| span.start <= pos);
        idx.checked_sub(1)
            .map(|idx| self.blade_spans[idx])
            .filter(|span| pos < span.end)
    }

    fn blade_unsupported(&self, span: RawSpan, what: &str) -> SyntaxError {
        let (line, column) = helpers::pos_to_line_col(self.source, span.start);
        self.emit_error_with_pos(
            SyntaxErrorKind::BladeUnsupported {
                reason: format!("{} starts inside {what}", span.label()),
                line,
                column,
            },
            span.start,
        )
    }

    /// A raw span may not start strictly inside `(lo, hi)`: the compiler would extract it first
    /// and the tree would no longer match what it compiles.
    fn blade_ensure_no_span_inside(&self, lo: usize, hi: usize, what: &str) -> PResult<()> {
        let idx = self.blade_spans.partition_point(|span| span.start <= lo);
        match self.blade_spans.get(idx) {
            Some(span) if span.start < hi => Err(self.blade_unsupported(*span, what)),
            _ => Ok(()),
        }
    }

    /// Called once the start tag, including `>`, is consumed.
    pub(super) fn blade_check_start_tag(&mut self, element_start: usize) -> PResult<()> {
        if self.blade_spans.is_empty() {
            return Ok(());
        }
        let end = self.peek_pos();
        self.blade_ensure_no_span_inside(element_start, end, "a tag")
    }

    /// Spans inside `<script>`, `<style>`, `<pre>` and `<textarea>` are kept verbatim anyway, unless
    /// they'd swallow the closing tag.
    pub(super) fn blade_check_raw_text(&self, start: usize, end: usize) -> PResult<()> {
        let idx = self.blade_spans.partition_point(|span| span.start < end);
        match idx.checked_sub(1).map(|idx| self.blade_spans[idx]) {
            Some(span) if span.start >= start && span.end > end => {
                Err(self.blade_unsupported(span, "a raw text element"))
            }
            _ => Ok(()),
        }
    }

    /// A branch or closer without an opener. When an element opened inside the same Blade block
    /// is still open, the HTML is what is unbalanced across branches, so that is blamed instead.
    fn blade_unexpected(&self, pos: usize, name: &str) -> SyntaxError {
        let (line, column) = helpers::pos_to_line_col(self.source, pos);
        if let Some(&(tag_name, start, depth)) = self.blade_open_elements.last()
            && depth == self.blade_depth
        {
            let (open_line, open_column) = helpers::pos_to_line_col(self.source, start);
            return self.emit_error_with_pos(
                SyntaxErrorKind::BladeUnsupported {
                    reason: format!(
                        "HTML element <{tag_name}> opened at line {open_line}, column {open_column} is not closed before @{name} (HTML unbalanced across Blade branches is not supported)"
                    ),
                    line,
                    column,
                },
                pos,
            );
        }
        self.emit_error_with_pos(
            SyntaxErrorKind::BladeUnexpectedDirective {
                name: name.into(),
                line,
                column,
            },
            pos,
        )
    }

    // ---- lexical helpers ----

    /// `{!! !!}`, `{{{ }}}` or `{{ }}` at `at`, ending at the first closer. The text in between
    /// needs at least one char (`{{}}` is no echo).
    fn find_echo(&self, at: usize) -> Option<EchoSpan> {
        let src = self.source;
        let rest = src.get(at..)?;
        let delimited = |open: &str, close: Closer, kind| {
            let start = at + open.len();
            let first = src[start..].chars().next()?;
            let from = start + first.len_utf8();
            let end = self.blade_scan.find_closer(src, close, from)?;
            Some(EchoSpan {
                kind,
                start,
                end,
                after: end + close.text().len(),
            })
        };
        if rest.starts_with("{!!") {
            delimited("{!!", Closer::Raw, BladeEchoKind::Raw)
        } else if rest.starts_with("{{{") {
            delimited("{{{", Closer::Triple, BladeEchoKind::Triple)
                .or_else(|| delimited("{{", Closer::Escaped, BladeEchoKind::Escaped))
        } else if rest.starts_with("{{") {
            delimited("{{", Closer::Escaped, BladeEchoKind::Escaped)
        } else {
            None
        }
    }

    /// `{{-- … --}}` at `at`: `(raw start, raw end, after)`.
    fn find_blade_comment(&self, at: usize) -> Option<(usize, usize, usize)> {
        let from = at + "{{--".len();
        let end = self
            .blade_scan
            .find_closer(self.source, Closer::Comment, from)?;
        Some((from, end, end + Closer::Comment.text().len()))
    }

    /// [`scan_directive`] on the source, sharing the parser's bracket memo.
    fn scan_blade_directive(&self, at: usize) -> Option<DirectiveToken> {
        scan_directive(self.source, at, &mut self.blade_scan.parens.borrow_mut())
    }

    /// End of the comment or echo starting at `at`, if there is one.
    fn blade_brace_end(&self, at: usize) -> Option<usize> {
        if self.source[at..].starts_with("{{--") {
            self.find_blade_comment(at).map(|(_, _, after)| after)
        } else {
            self.find_echo(at).map(|echo| echo.after)
        }
    }

    /// Classifies the `@` at `at`.
    pub(super) fn blade_at(&self, at: usize) -> At<'s> {
        let src = self.source;
        if src.as_bytes().get(at + 1) == Some(&b'{') {
            // `@{{ … }}`, `@{!! … !!}`, `@{{{ … }}}` print the echo literally. A comment is
            // removed by the compiler before echoes are looked at, so it isn't escaped.
            if !src[at + 1..].starts_with("{{--")
                && let Some(echo) = self.find_echo(at + 1)
                && self.blade_ensure_no_span_inside(at, echo.after, "").is_ok()
            {
                return At::Text(echo.after - at);
            }
            return At::Text(1);
        }
        let Some(token) = self.scan_blade_directive(at) else {
            return At::Text(1);
        };
        // `@@name` is an escaped directive.
        if src.as_bytes()[token.name.start] == b'@' {
            return At::Text(token.name.end - at);
        }
        let name_end = token.name.end;
        let (directive, end) = build_directive(src, token);
        let role = self
            .registry()
            .role(directive.name, directive.args.map(|(args, _)| args));
        if role == DirectiveRole::Unknown {
            return At::Text(name_end - at);
        }
        At::Directive {
            directive,
            end,
            role,
        }
    }

    // ---- nodes ----

    /// Blade's part of node dispatch; `None` leaves the construct to the HTML parsing.
    pub(super) fn parse_blade_node_kind(&mut self) -> Option<PResult<NodeKind<'s>>> {
        let &(pos, c) = self.chars.peek()?;
        if let Some(span) = self.blade_span_containing(pos) {
            return Some(Err(self.blade_unsupported(span, "another construct")));
        }
        if let Some(span) = self.blade_span_at(pos) {
            return Some(self.parse_blade_raw_span(span));
        }
        match c {
            '<' => match self.second() {
                Some('?') => {
                    let (open_len, kind) = php_open(&self.source[pos..])?;
                    Some(self.parse_blade_php_tag(pos, open_len, kind))
                }
                Some('!') if self.source[pos..].starts_with("<!--") => {
                    Some(self.parse_blade_html_comment(pos))
                }
                // `<{` only starts an element when an echo follows (`<{{ $tag }}`, `<{!! $tag !!}`).
                Some('{') if self.blade_tag_echo_at(pos + 1).is_none() => {
                    Some(self.parse_text_node().map(NodeKind::Text))
                }
                _ => None,
            },
            '{' => Some(self.parse_blade_brace(pos)),
            '@' => Some(self.parse_blade_at(pos)),
            _ => None,
        }
    }

    fn parse_blade_raw_span(&mut self, span: RawSpan) -> PResult<NodeKind<'s>> {
        let src = self.source;
        self.advance_to(span.end);
        let raw = &src[span.start..span.end];
        match span.kind {
            RawSpanKind::Verbatim => Ok(NodeKind::BladeVerbatim(BladeVerbatim { raw })),
            RawSpanKind::Php => {
                let start = span.start + PHP_OPEN.len();
                let code = &src[start..span.end - PHP_CLOSE.len()];
                // `@php($x = 1)` followed by a later `@endphp` is swallowed whole by the compiler.
                if code.trim_start_matches([' ', '\t']).starts_with('(') {
                    Ok(NodeKind::BladeVerbatim(BladeVerbatim { raw }))
                } else {
                    Ok(NodeKind::BladePhp(BladePhp {
                        kind: BladePhpKind::Directive,
                        code,
                        start,
                        closed: true,
                    }))
                }
            }
        }
    }

    fn parse_blade_php_tag(
        &mut self,
        pos: usize,
        open_len: usize,
        kind: BladePhpKind,
    ) -> PResult<NodeKind<'s>> {
        let start = pos + open_len;
        let (code_end, after, closed) = php_tag_end(self.source, start);
        self.blade_ensure_no_span_inside(pos, after, "a PHP tag")?;
        self.advance_to(after);
        Ok(NodeKind::BladePhp(BladePhp {
            kind,
            code: &self.source[start..code_end],
            start,
            closed,
        }))
    }

    fn parse_blade_html_comment(&mut self, pos: usize) -> PResult<NodeKind<'s>> {
        let comment = self.parse_comment()?;
        let end = self.peek_pos();
        self.blade_ensure_no_span_inside(pos, end, "an HTML comment")?;
        // The compiler still sees what is inside a comment, so a block structure hidden there
        // would not match the tree.
        let visible = &self.source[..end];
        let mut parens = ParenIndex::default();
        for (rel, _) in self.source[pos..end].match_indices('@') {
            let at = pos + rel;
            let Some(token) = scan_directive(visible, at, &mut parens) else {
                continue;
            };
            let (directive, _) = build_directive(visible, token);
            let role = self
                .registry()
                .role(directive.name, directive.args.map(|(args, _)| args));
            if matches!(
                role,
                DirectiveRole::Opener | DirectiveRole::Branch | DirectiveRole::Closer
            ) {
                let (line, column) = helpers::pos_to_line_col(self.source, at);
                return Err(self.emit_error_with_pos(
                    SyntaxErrorKind::BladeUnsupported {
                        reason: format!("`@{}` inside an HTML comment", directive.name),
                        line,
                        column,
                    },
                    at,
                ));
            }
        }
        Ok(NodeKind::Comment(comment))
    }

    fn parse_blade_comment(&mut self, pos: usize) -> PResult<BladeComment<'s>> {
        let Some((start, end, after)) = self.find_blade_comment(pos) else {
            return Err(self.emit_error_with_pos(SyntaxErrorKind::ExpectBladeCommentEnd, pos));
        };
        self.blade_ensure_no_span_inside(pos, after, "a Blade comment")?;
        self.advance_to(after);
        Ok(BladeComment {
            raw: &self.source[start..end],
        })
    }

    fn parse_blade_echo(&mut self, pos: usize, echo: EchoSpan) -> PResult<BladeEcho<'s>> {
        self.blade_ensure_no_span_inside(pos, echo.after, "an echo")?;
        self.advance_to(echo.after);
        Ok(BladeEcho {
            kind: echo.kind,
            expr: &self.source[echo.start..echo.end],
            start: echo.start,
        })
    }

    fn parse_blade_brace(&mut self, pos: usize) -> PResult<NodeKind<'s>> {
        if self.source[pos..].starts_with("{{--") {
            return self.parse_blade_comment(pos).map(NodeKind::BladeComment);
        }
        match self.find_echo(pos) {
            Some(echo) => self.parse_blade_echo(pos, echo).map(NodeKind::BladeEcho),
            // Without a closer (or content) Blade leaves it alone.
            None => self.parse_text_node().map(NodeKind::Text),
        }
    }

    fn parse_blade_at(&mut self, pos: usize) -> PResult<NodeKind<'s>> {
        let At::Directive {
            directive,
            end,
            role,
        } = self.blade_at(pos)
        else {
            return self.parse_text_node().map(NodeKind::Text);
        };
        if role == DirectiveRole::Opener
            && let Some(closers) = self.registry().raw_body_closers(directive.name)
        {
            let closers = closers.to_vec();
            return self.parse_blade_raw_body(pos, directive, end, &closers);
        }
        self.parse_blade_directive::<Node<'s>, _>(
            pos,
            directive,
            end,
            role,
            &mut Parser::parse_node,
        )
    }

    /// `@script … @endscript`: the body is not Blade-parsed (and thus not formatted).
    fn parse_blade_raw_body(
        &mut self,
        pos: usize,
        opener: BladeDirective<'s>,
        opener_end: usize,
        closers: &[String],
    ) -> PResult<NodeKind<'s>> {
        self.blade_ensure_no_span_inside(pos, opener_end, "a directive")?;
        let src = self.source;
        let mut from = opener_end;
        let close_end = loop {
            let Some(rel) = src[from..].find('@') else {
                let (line, column) = helpers::pos_to_line_col(src, pos);
                self.advance_to(src.len());
                return Err(self.emit_error(SyntaxErrorKind::ExpectBladeBlockEnd {
                    name: opener.name.into(),
                    line,
                    column,
                }));
            };
            let at = from + rel;
            // Raw spans are extracted first, so what they contain can't close the block.
            if let Some(span) = self.blade_span_covering(at) {
                from = span.end;
                continue;
            }
            if let Some(token) = self.scan_blade_directive(at)
                && closers
                    .iter()
                    .any(|closer| *closer == src[token.name.clone()])
            {
                break token.end;
            }
            from = at + 1;
        };
        self.advance_to(close_end);
        Ok(NodeKind::BladeVerbatim(BladeVerbatim {
            raw: &src[pos..close_end],
        }))
    }

    /// Handles a directive found by [`Parser::blade_at`], inline or as the opener of a block.
    pub(super) fn parse_blade_directive<T, F>(
        &mut self,
        pos: usize,
        directive: BladeDirective<'s>,
        end: usize,
        role: DirectiveRole,
        children_parser: &mut F,
    ) -> PResult<T::Intermediate>
    where
        T: HasBladeFlowControl<'s>,
        F: FnMut(&mut Self) -> PResult<T>,
    {
        match role {
            DirectiveRole::Branch | DirectiveRole::Closer => {
                Err(self.blade_unexpected(pos, directive.name))
            }
            DirectiveRole::Opener => {
                self.blade_ensure_no_span_inside(pos, end, "a directive")?;
                self.advance_to(end);
                let block = self.parse_blade_block(directive, pos, children_parser)?;
                Ok(T::from_block(block))
            }
            DirectiveRole::Inline | DirectiveRole::Unknown => {
                self.blade_ensure_no_span_inside(pos, end, "a directive")?;
                self.advance_to(end);
                Ok(T::from_directive(directive))
            }
        }
    }

    /// The opener is consumed; reads children up to and including its closer.
    fn parse_blade_block<T, F>(
        &mut self,
        opener: BladeDirective<'s>,
        opener_pos: usize,
        children_parser: &mut F,
    ) -> PResult<BladeBlock<'s, T>>
    where
        T: HasBladeFlowControl<'s>,
        F: FnMut(&mut Self) -> PResult<T>,
    {
        let probe = BladeDirective {
            name: opener.name,
            space: opener.space,
            args: opener.args,
        };
        let mut body = vec![BladeDirectiveOrChildren::Directive(opener)];
        let mut children = vec![];
        self.blade_depth += 1;
        loop {
            if T::skip_ws_between_children() {
                self.eat_ws();
            }
            let Some(&(pos, c)) = self.chars.peek() else {
                let (line, column) = helpers::pos_to_line_col(self.source, opener_pos);
                return Err(self.emit_error(SyntaxErrorKind::ExpectBladeBlockEnd {
                    name: probe.name.into(),
                    line,
                    column,
                }));
            };
            if c == '@'
                && self.blade_span_at(pos).is_none()
                && let At::Directive { directive, end, .. } = self.blade_at(pos)
            {
                let registry = self.registry();
                let is_branch = registry.is_branch_of(&probe, &directive);
                let is_closer = !is_branch && registry.is_closer_of(&probe, &directive);
                if is_branch || is_closer {
                    self.blade_ensure_no_span_inside(pos, end, "a directive")?;
                    self.advance_to(end);
                    if !children.is_empty() {
                        body.push(BladeDirectiveOrChildren::Children(mem::take(&mut children)));
                    }
                    body.push(BladeDirectiveOrChildren::Directive(directive));
                    if is_closer {
                        self.blade_depth -= 1;
                        return Ok(BladeBlock { body });
                    }
                    continue;
                }
            }
            children.push(children_parser(self)?);
        }
    }

    // ---- text ----

    /// Text up to the next construct Blade (or HTML) parses itself. The first char is always
    /// consumed because dispatch has already rejected whatever starts there.
    pub(super) fn parse_blade_text_node(&mut self) -> PResult<TextNode<'s>> {
        let Some(&(start, _)) = self.chars.peek() else {
            return Err(self.emit_error(SyntaxErrorKind::ExpectTextNode));
        };
        let limit = self
            .blade_spans
            .get(self.blade_spans.partition_point(|span| span.start <= start))
            .map_or(usize::MAX, |span| span.start);

        let mut line_breaks = 0;
        let end = loop {
            let Some(&(i, c)) = self.chars.peek() else {
                break self.source.len();
            };
            if i >= limit {
                break i;
            }
            match c {
                '@' => match self.blade_at(i) {
                    At::Directive { .. } if i > start => break i,
                    At::Directive { .. } => {
                        self.chars.next();
                    }
                    At::Text(len) => {
                        line_breaks += self.source.as_bytes()[i..i + len]
                            .iter()
                            .filter(|&&byte| byte == b'\n')
                            .count();
                        self.advance_to(i + len);
                    }
                },
                '{' if i > start && self.is_blade_echo_start() => break i,
                '<' if i > start && self.is_blade_text_stop_at_angle(i) => break i,
                _ => {
                    if c == '\n' {
                        line_breaks += 1;
                    }
                    self.chars.next();
                }
            }
        };

        Ok(TextNode {
            raw: &self.source[start..end],
            line_breaks,
            start,
        })
    }

    fn is_blade_echo_start(&self) -> bool {
        match self.second() {
            Some('{') => true,
            Some('!') => self.third() == Some('!'),
            _ => false,
        }
    }

    fn is_blade_text_stop_at_angle(&self, pos: usize) -> bool {
        match self.second() {
            Some('{') => self.blade_tag_echo_at(pos + 1).is_some(),
            Some('?') => php_open(&self.source[pos..]).is_some(),
            Some(c) => is_html_tag_name_char(c) || c == '/' || c == '!',
            None => false,
        }
    }

    // ---- attributes ----

    pub(super) fn parse_blade_attr(&mut self) -> PResult<Attribute<'s>> {
        let Some(&(pos, c)) = self.chars.peek() else {
            return Err(self.emit_error(SyntaxErrorKind::ExpectAttrName));
        };
        match c {
            '{' if self.source[pos..].starts_with("{{--") => {
                self.parse_blade_comment(pos).map(Attribute::BladeComment)
            }
            '{' => match self.find_echo(pos) {
                // An echo glued to more name chars or to `=` is (part of) an attribute name.
                Some(echo)
                    if self.source[echo.after..]
                        .chars()
                        .next()
                        .is_some_and(|next| {
                            next == '=' || (next != '@' && is_attr_name_char(next))
                        }) =>
                {
                    self.parse_native_attr().map(Attribute::Native)
                }
                Some(echo) => self.parse_blade_echo(pos, echo).map(Attribute::BladeEcho),
                None => self.parse_native_attr().map(Attribute::Native),
            },
            '@' => match self.blade_at(pos) {
                At::Directive {
                    directive,
                    end,
                    role,
                } => self.parse_blade_directive::<Attribute<'s>, _>(
                    pos,
                    directive,
                    end,
                    role,
                    &mut Parser::parse_attr,
                ),
                At::Text(_) => self.parse_native_attr().map(Attribute::Native),
            },
            _ => self.parse_native_attr().map(Attribute::Native),
        }
    }

    /// Like an HTML attribute name, with echoes allowed inside (`data-{{ $k }}`).
    pub(super) fn parse_blade_attr_name(&mut self) -> PResult<&'s str> {
        let start = self.peek_pos();
        let mut end = start;
        while let Some(&(i, c)) = self.chars.peek() {
            if c == '{'
                && let Some(echo) = self.find_echo(i)
            {
                self.advance_to(echo.after);
                end = echo.after;
            } else if is_attr_name_char(c) {
                self.chars.next();
                end = i + c.len_utf8();
            } else {
                break;
            }
        }
        if end == start {
            return Err(self.emit_error(SyntaxErrorKind::ExpectAttrName));
        }
        Ok(&self.source[start..end])
    }

    /// An echo (not a comment) at `pos`: the only `{` that may sit inside a tag name.
    fn blade_tag_echo_at(&self, pos: usize) -> Option<EchoSpan> {
        if self.source[pos..].starts_with("{{--") {
            return None;
        }
        self.find_echo(pos)
    }

    /// Tag names may mix name chars with echoes in any position (`h{!! $level !!}`).
    pub(super) fn parse_blade_tag_name(&mut self) -> PResult<&'s str> {
        let start = self.peek_pos();
        let mut end = start;
        while let Some(&(i, c)) = self.chars.peek() {
            if c == '{'
                && let Some(echo) = self.blade_tag_echo_at(i)
            {
                self.advance_to(echo.after);
                end = echo.after;
            } else if is_html_tag_name_char(c) {
                self.chars.next();
                end = i + c.len_utf8();
            } else {
                break;
            }
        }
        if end == start {
            return Err(self.emit_error(SyntaxErrorKind::ExpectTagName));
        }
        Ok(&self.source[start..end])
    }

    /// Echoes, comments and known directives with arguments hide the quote char in quoted
    /// values; unquoted values may contain echoes.
    pub(super) fn parse_blade_attr_value(&mut self) -> PResult<(&'s str, usize)> {
        if let Some((quote_pos, quote)) = self.chars.next_if(|(_, c)| *c == '"' || *c == '\'') {
            let start = quote_pos + 1;
            loop {
                let Some(&(i, c)) = self.chars.peek() else {
                    return Err(self.emit_error(SyntaxErrorKind::ExpectChar(quote)));
                };
                if c == quote {
                    self.chars.next();
                    return Ok((&self.source[start..i], start));
                }
                match c {
                    '{' => match self.blade_brace_end(i) {
                        Some(after) => self.advance_to(after),
                        None => {
                            self.chars.next();
                        }
                    },
                    '@' => match self.blade_at(i) {
                        At::Directive { end, .. } => self.advance_to(end),
                        At::Text(len) => self.advance_to(i + len),
                    },
                    _ => {
                        self.chars.next();
                    }
                }
            }
        }

        fn is_unquoted_attr_value_char(c: char) -> bool {
            !c.is_ascii_whitespace() && !matches!(c, '"' | '\'' | '=' | '<' | '>' | '`')
        }

        let start = match self.chars.peek() {
            Some(&(i, c)) if is_unquoted_attr_value_char(c) => i,
            _ => return Err(self.emit_error(SyntaxErrorKind::ExpectAttrValue)),
        };
        let mut end = start;
        while let Some(&(i, c)) = self.chars.peek() {
            if c == '{'
                && let Some(after) = self.blade_brace_end(i)
            {
                self.advance_to(after);
                end = after;
            } else if is_unquoted_attr_value_char(c) {
                self.chars.next();
                end = i + c.len_utf8();
            } else {
                break;
            }
        }
        Ok((&self.source[start..end], start))
    }
}

#[cfg(test)]
mod tests {
    use super::{closes_tag, php_tag_end};
    use crate::{
        ast::NodeKind,
        config::LanguageOptions,
        parser::{Language, Parser},
    };
    use std::time::{Duration, Instant};

    fn parse_blade(src: &str) -> Result<usize, ()> {
        Parser::new(src, Language::Blade, &LanguageOptions::default())
            .parse_root()
            .map(|root| root.children.len())
            .map_err(|_| ())
    }

    #[test]
    fn php_close_tag_inside_string_interpolation_is_hidden() {
        // `token_get_all` ends the tag only at the final `?>`.
        for code in [
            r#"echo "{$a["?>"]}  x"; "#,
            r#"echo "${a["?>"]}  x"; "#,
            r#"echo `{$a["?>"]}  x`; "#,
            r#"echo "{$a["{$b["?>"]}"]}"; "#,
            r#"echo '{$a["?>"]}'; "#,
        ] {
            let src = format!("<?php {code}?> tail");
            let (code_end, after, closed) = php_tag_end(&src, "<?php".len());
            assert!(closed, "{src}");
            assert_eq!(&src[code_end..after], "?>", "{src}");
            assert_eq!(&src[after..], " tail", "{src}");
        }
    }

    #[test]
    fn single_quoted_string_does_not_interpolate() {
        let src = "<?php echo '{$'; ?>x'";
        let (code_end, after, closed) = php_tag_end(src, "<?php".len());
        assert!(closed);
        assert_eq!(&src[..code_end], "<?php echo '{$'; ");
        assert_eq!(&src[after..], "x'");
    }

    #[test]
    fn line_comment_still_ends_at_close_tag() {
        let src = "<?php // note ?> tail";
        let (code_end, after, closed) = php_tag_end(src, "<?php".len());
        assert!(closed);
        assert_eq!(&src[..code_end], "<?php // note ");
        assert_eq!(&src[after..], " tail");
    }

    #[test]
    fn php_tag_with_interpolated_close_tag_is_one_node() {
        let src = r#"<?php echo "{$a["?>"]}  x"; ?>"#;
        let root = Parser::new(src, Language::Blade, &LanguageOptions::default())
            .parse_root()
            .map_err(|_| ())
            .unwrap();
        assert_eq!(root.children.len(), 1);
        let NodeKind::BladePhp(php) = &root.children[0].kind else {
            panic!("expected a PHP node");
        };
        assert_eq!(php.code, r#" echo "{$a["?>"]}  x"; "#);
        assert!(php.closed);
    }

    /// Each opener below used to search the whole remaining source for a closer that is not there.
    #[test]
    fn unmatched_openers_parse_in_linear_time() {
        let count = 200_000;
        for unit in ["{{ ", "{!! ", "{{{ ", "@include( ", "<a b=\"{{ "] {
            let src = unit.repeat(count);
            let started = Instant::now();
            // Only the running time matters here; a syntax error would be just as fast.
            let _ = parse_blade(&src);
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "{unit:?} took {:?}",
                started.elapsed()
            );
        }
    }

    #[test]
    fn closer_cache_does_not_hide_later_closers() {
        // The `}}` after the unmatched `{!!` must still close the echo that follows it.
        let src = "{!! a }} {{ b }}";
        let root = Parser::new(src, Language::Blade, &LanguageOptions::default())
            .parse_root()
            .map_err(|_| ())
            .unwrap();
        assert!(
            root.children
                .iter()
                .any(|node| matches!(node.kind, NodeKind::BladeEcho(_)))
        );
    }

    #[test]
    fn component_closers() {
        assert!(closes_tag("div", "DIV"));
        assert!(!closes_tag("div", "span"));
        assert!(closes_tag("x-ui.button-dropdown", "x-button-dropdown"));
        assert!(closes_tag("x:modal", "x-card"));
        assert!(closes_tag("x-slot:title", "x-slot"));
        assert!(closes_tag("x-slot", "x:slot:other"));
        assert!(!closes_tag("x-slot:title", "x-card"));
        assert!(!closes_tag("x-card", "x-slot"));
        assert!(!closes_tag("x-card", "div"));
        assert!(!closes_tag("div", "x-card"));
    }

    #[test]
    fn echo_names_compare_exactly() {
        assert!(closes_tag("h{!! $l !!}", "h{!! $l !!}"));
        assert!(!closes_tag("h{!! $l !!}", "h{!! $L !!}"));
        assert!(!closes_tag("h{{ $l }}", "h"));
    }
}
