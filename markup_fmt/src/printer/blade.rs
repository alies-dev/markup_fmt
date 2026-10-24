use super::{DocGen, format_control_structure_block_children, is_all_ascii_whitespace, reflow_raw};
use crate::{
    ast::*,
    blade::{DirectiveRole, php::BladePhpSnippet, scan_directive},
    config::{BladeDirectiveSpace, LineBreak},
    ctx::{Ctx, Hints},
    helpers,
    state::State,
};
use anyhow::Error;
use std::borrow::Cow;
use tiny_pretty::Doc;

/// Built-in directives that `blade.directive_space = normalize` separates from `(` by one space.
const CONTROL_FLOW: [&str; 11] = [
    "if", "elseif", "unless", "isset", "empty", "foreach", "forelse", "for", "while", "switch",
    "case",
];

pub(super) fn format_block_nodes<'s, F>(
    block: &BladeBlock<'s, Node<'s>>,
    ctx: &mut Ctx<'s, F>,
    state: &State<'s>,
) -> Doc<'s>
where
    F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, Error>,
{
    let doc = Doc::list(
        block
            .body
            .iter()
            .map(|part| match part {
                BladeDirectiveOrChildren::Directive(directive) => {
                    format_directive(directive, ctx, state, false)
                }
                BladeDirectiveOrChildren::Children(children) => {
                    format_control_structure_block_children(children, ctx, state)
                }
            })
            .collect(),
    );
    if is_inline_block(block) {
        // Lets the whole block fall back to one line, keeping its spaces as spaces.
        doc.group()
    } else {
        doc
    }
}

pub(super) fn format_block_attrs<'s, F>(
    block: &BladeBlock<'s, Attribute<'s>>,
    ctx: &mut Ctx<'s, F>,
    state: &State<'s>,
) -> Doc<'s>
where
    F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, Error>,
{
    Doc::list(
        block
            .body
            .iter()
            .map(|part| match part {
                BladeDirectiveOrChildren::Directive(directive) => {
                    format_directive(directive, ctx, state, true)
                }
                // Whitespace around the children is kept: `selected@endif` would hide `@endif`.
                BladeDirectiveOrChildren::Children(children) if children.is_empty() => {
                    Doc::line_or_space()
                }
                BladeDirectiveOrChildren::Children(children) => Doc::line_or_space()
                    .concat(itertools::intersperse(
                        children.iter().map(|attr| attr.doc(ctx, state)),
                        Doc::line_or_space(),
                    ))
                    .nest(ctx.indent_width)
                    .append(Doc::line_or_space()),
            })
            .collect(),
    )
    .group()
}

pub(super) fn format_comment<'s, F>(
    comment: &BladeComment<'s>,
    _ctx: &mut Ctx<'s, F>,
    _state: &State<'s>,
) -> Doc<'s>
where
    F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, Error>,
{
    Doc::text("{{--")
        .concat(reflow_raw(comment.raw))
        .append(Doc::text("--}}"))
}

pub(super) fn format_directive<'s, F>(
    directive: &BladeDirective<'s>,
    ctx: &mut Ctx<'s, F>,
    state: &State<'s>,
    in_tag: bool,
) -> Doc<'s>
where
    F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, Error>,
{
    directive_doc(directive, ctx, state, in_tag).0
}

/// The directive's doc and whether it spans several lines once printed.
fn directive_doc<'s, F>(
    directive: &BladeDirective<'s>,
    ctx: &mut Ctx<'s, F>,
    state: &State<'s>,
    in_tag: bool,
) -> (Doc<'s>, bool)
where
    F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, Error>,
{
    let mut docs = Vec::with_capacity(6);
    docs.push(Doc::char('@'));
    docs.push(Doc::text(directive.name));
    let Some((args, start)) = directive.args else {
        return (Doc::list(docs), false);
    };

    // Inside a component start tag only `@class`/`@style` are directives for the PHP formatter;
    // everything else there is kept exactly as written.
    let in_component_tag = in_tag
        && state
            .current_tag_name
            .is_some_and(helpers::is_blade_component);
    let formats_args = ctx
        .blade
        .as_ref()
        .is_some_and(|registry| registry.formats_args(directive.name))
        && (!in_component_tag || matches!(directive.name, "class" | "style"));
    let has_args = !args.trim_ascii().is_empty();
    let space = if formats_args
        && has_args
        && !in_component_tag
        && ctx.options.blade_directive_space == BladeDirectiveSpace::Normalize
    {
        if CONTROL_FLOW
            .iter()
            .any(|name| directive.name.eq_ignore_ascii_case(name))
        {
            " "
        } else {
            ""
        }
    } else {
        directive.space
    };
    if !space.is_empty() {
        docs.push(Doc::text(space));
    }
    docs.push(Doc::char('('));

    let formatted = if formats_args && has_args {
        ctx.format_blade_php(
            BladePhpSnippet::Directive {
                name: directive.name,
                args,
            },
            args,
            start,
            state,
        )
    } else {
        Cow::Borrowed(args)
    };
    let multi_line = match formatted {
        Cow::Borrowed(args) => {
            let shifted = args
                .contains('\n')
                .then(|| {
                    reflow_shifted(
                        args,
                        source_line_indent(ctx.source, start),
                        ")",
                        ctx.line_break,
                    )
                })
                .flatten();
            match shifted {
                Some(lines) => docs.extend(lines),
                None => {
                    docs.extend(reflow_exact(args, ctx.line_break));
                    docs.push(Doc::char(')'));
                }
            }
            args.contains('\n')
        }
        Cow::Owned(text) if !text.contains('\n') => {
            docs.push(Doc::text(text));
            docs.push(Doc::char(')'));
            false
        }
        Cow::Owned(text) => {
            let multi_line = match PhpText::new(&text) {
                Some(lines) => {
                    docs.push(
                        Doc::hard_line()
                            .concat(lines.docs(ctx.line_break).into_iter())
                            .nest(ctx.indent_width),
                    );
                    docs.push(Doc::hard_line());
                    true
                }
                None => {
                    docs.extend(reflow_exact(args, ctx.line_break));
                    args.contains('\n')
                }
            };
            docs.push(Doc::char(')'));
            multi_line
        }
    };
    (Doc::list(docs), multi_line)
}

pub(super) fn format_echo<'s, F>(
    echo: &BladeEcho<'s>,
    ctx: &mut Ctx<'s, F>,
    state: &State<'s>,
) -> Doc<'s>
where
    F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, Error>,
{
    echo_doc(echo, ctx, state).0
}

/// The echo's doc and whether it spans several lines once printed.
fn echo_doc<'s, F>(echo: &BladeEcho<'s>, ctx: &mut Ctx<'s, F>, state: &State<'s>) -> (Doc<'s>, bool)
where
    F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, Error>,
{
    let (open, close) = match echo.kind {
        BladeEchoKind::Escaped => ("{{", "}}"),
        BladeEchoKind::Raw => ("{!!", "!!}"),
        BladeEchoKind::Triple => ("{{{", "}}}"),
    };
    let expr = echo.expr;
    let line_break = ctx.line_break;
    let verbatim = || {
        (
            Doc::text(open)
                .concat(reflow_exact(expr, line_break).into_iter())
                .append(Doc::text(close)),
            expr.contains('\n'),
        )
    };

    if expr.trim_ascii().is_empty() || ["{{", "{!!", "}}", "!!}"].iter().any(|d| expr.contains(d)) {
        return verbatim();
    }

    let formatted = ctx.format_blade_php(
        BladePhpSnippet::Echo {
            expr,
            kind: echo.kind,
        },
        expr,
        echo.start,
        state,
    );
    let text = formatted.trim_ascii();
    if !text.contains('\n') {
        return (
            Doc::list(vec![
                Doc::text(open),
                Doc::space(),
                Doc::text(text.to_owned()),
                Doc::space(),
                Doc::text(close),
            ]),
            false,
        );
    }
    if matches!(formatted, Cow::Borrowed(..)) {
        // No formatter result: the layout stays as written, only moved along with the echo.
        // Shifting by the echo's line move (not re-indenting) keeps a second pass at shift 0.
        return match reflow_shifted(
            expr,
            source_line_indent(ctx.source, echo.start),
            close,
            ctx.line_break,
        ) {
            Some(lines) => (Doc::text(open).concat(lines.into_iter()), true),
            None => verbatim(),
        };
    }
    match PhpText::new(text) {
        Some(lines) => (
            Doc::text(open)
                .append(
                    Doc::hard_line()
                        .concat(lines.docs(ctx.line_break).into_iter())
                        .nest(ctx.indent_width),
                )
                .append(Doc::hard_line())
                .append(Doc::text(close)),
            true,
        ),
        None => verbatim(),
    }
}

/// Blade echoes and directives in attribute position, with whether they print on several lines.
/// The start tag's layout follows this, not the source, so the formatted result decides it.
pub(super) fn format_attr<'s, F>(
    attr: &Attribute<'s>,
    ctx: &mut Ctx<'s, F>,
    state: &State<'s>,
) -> Option<(Doc<'s>, bool)>
where
    F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, Error>,
{
    match attr {
        Attribute::BladeEcho(echo) => Some(echo_doc(echo, ctx, state)),
        Attribute::BladeDirective(directive) => Some(directive_doc(directive, ctx, state, true)),
        _ => None,
    }
}

pub(super) fn format_php<'s, F>(
    php: &BladePhp<'s>,
    ctx: &mut Ctx<'s, F>,
    state: &State<'s>,
) -> Doc<'s>
where
    F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, Error>,
{
    let (open, close): (&'s str, &'static str) = match php.kind {
        BladePhpKind::Tag => (
            // The tag keeps its source spelling (`<?PHP` is valid).
            php.start
                .checked_sub("<?php".len())
                .and_then(|from| ctx.source.get(from..php.start))
                .filter(|tag| tag.eq_ignore_ascii_case("<?php"))
                .unwrap_or("<?php"),
            "?>",
        ),
        BladePhpKind::EchoTag => ("<?=", "?>"),
        BladePhpKind::Directive => ("@php", "@endphp"),
    };

    let mut verbatim = vec![Doc::text(open)];
    verbatim.extend(reflow_exact(php.code, ctx.line_break));
    if php.closed {
        verbatim.push(Doc::text(close));
    }

    // `<?=` bodies are `echo` argument lists without a faithful PHP frame, and an unclosed tag
    // has no end to put a formatted body in front of.
    if php.kind == BladePhpKind::EchoTag || !php.closed {
        return Doc::list(verbatim);
    }
    let Cow::Owned(text) =
        ctx.format_blade_php(BladePhpSnippet::Raw(php.code), php.code, php.start, state)
    else {
        return Doc::list(verbatim);
    };

    if !text.contains('\n') {
        return Doc::list(vec![
            Doc::text(open),
            Doc::space(),
            Doc::text(text),
            Doc::space(),
            Doc::text(close),
        ]);
    }
    // A body ending in a `//` comment keeps its newline, so `?>` always starts its own line.
    match PhpText::new(&text) {
        Some(lines) => Doc::text(open)
            .append(Doc::hard_line())
            .concat(lines.docs(ctx.line_break).into_iter())
            .append(Doc::hard_line())
            .append(Doc::text(close)),
        None => Doc::list(verbatim),
    }
}

pub(super) fn format_verbatim<'s, F>(
    verbatim: &BladeVerbatim<'s>,
    ctx: &mut Ctx<'s, F>,
    _state: &State<'s>,
) -> Doc<'s>
where
    F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, Error>,
{
    Doc::list(reflow_exact(verbatim.raw, ctx.line_break))
}

/// Attributes of Blade component tags and attributes whose value carries Blade syntax.
/// Returns `None` when the regular attribute printer should handle it.
pub(super) fn format_native_attr<'s, F>(
    attr: &NativeAttribute<'s>,
    ctx: &mut Ctx<'s, F>,
    state: &State<'s>,
) -> Option<Doc<'s>>
where
    F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, Error>,
{
    let (value, start) = attr.value?;
    let is_component = state
        .current_tag_name
        .is_some_and(helpers::is_blade_component);
    let has_echo = value.contains("{{") || value.contains("{!!");
    if !is_component && !has_echo && !value.contains('@') {
        return None;
    }

    // `:attr="expr"` binds a PHP expression; `::attr` is escaped and stays as written.
    let is_binding = is_component
        && attr.name.starts_with(':')
        && !attr.name.starts_with("::")
        && !has_echo
        && !value.trim_ascii().is_empty();
    let formatted = if is_binding {
        ctx.format_blade_php(BladePhpSnippet::Attr(value), value, start, state)
    } else {
        Cow::Borrowed(value)
    };

    let mut docs = vec![Doc::text(attr.name), Doc::char('=')];
    if let Some(quote) = attr.quote {
        docs.push(Doc::char(quote));
    }
    match formatted {
        // An unquoted value can't hold whitespace, and the quote must not reappear inside.
        Cow::Owned(text)
            if attr.quote.is_some_and(|quote| !text.contains(quote)) && !text.contains('\n') =>
        {
            docs.push(Doc::text(text));
        }
        Cow::Owned(text) if attr.quote.is_some_and(|quote| !text.contains(quote)) => {
            match PhpText::new(&text) {
                Some(lines) => docs.extend(lines.docs(ctx.line_break)),
                None => docs.extend(reflow_exact(value, ctx.line_break)),
            }
        }
        _ => docs.extend(reflow_exact(value, ctx.line_break)),
    }
    if let Some(quote) = attr.quote {
        docs.push(Doc::char(quote));
    }
    Some(Doc::list(docs))
}

/// Whether a block can be printed in running text: nothing in its source spans lines.
pub(super) fn is_inline_block(block: &BladeBlock<'_, Node<'_>>) -> bool {
    block.body.iter().all(|part| match part {
        BladeDirectiveOrChildren::Directive(directive) => {
            directive.args.is_none_or(|(args, _)| !args.contains('\n'))
        }
        BladeDirectiveOrChildren::Children(children) => {
            children.iter().all(|child| !child.raw.contains('\n'))
        }
    })
}

/// Whether `<script>`/`<style>` text uses Blade syntax anywhere, so it must stay as written.
pub(super) fn has_blade_syntax<'s, F>(text: &str, ctx: &Ctx<'s, F>) -> bool
where
    F: for<'a> FnMut(&'a str, Hints) -> Result<Cow<'a, str>, Error>,
{
    if text.contains("{{")
        || text.contains("{!!")
        || text.contains("<?=")
        || text
            .as_bytes()
            .windows("<?php".len())
            .any(|window| window.eq_ignore_ascii_case(b"<?php"))
    {
        return true;
    }
    let Some(registry) = ctx.blade.as_ref() else {
        return false;
    };
    text.match_indices('@').any(|(at, _)| {
        scan_directive(text, at).is_some_and(|token| {
            let args = token.args.map(|args| &text[args]);
            registry.role(&text[token.name], args) != DirectiveRole::Unknown
        })
    })
}

/// PHP swallows one newline directly after `?>`. When the whitespace after a closed PHP node
/// does not start with a newline it must stay a plain space: a line break there would be
/// swallowed and the space lost. (A leading newline is kept as a line break by the regular
/// whitespace handling.)
pub(super) fn php_close_gap<'s>(prev: Option<&Node<'s>>, raw: &str) -> Option<Doc<'s>> {
    let after_php_close = matches!(
        prev,
        Some(Node {
            kind: NodeKind::BladePhp(BladePhp { closed: true, .. }),
            ..
        })
    );
    (after_php_close
        && raw.starts_with(|c: char| c.is_ascii_whitespace() && c != '\n' && c != '\r'))
    .then(Doc::space)
}

/// [`php_close_gap`] for whitespace that ends an element or block body. Text with anything but
/// whitespace at its end is ordinary inline text: nothing there was swallowed by `?>`.
pub(super) fn php_close_trailing_gap<'s>(children: &[Node<'s>]) -> Option<Doc<'s>> {
    let [.., prev, last] = children else {
        return None;
    };
    match &last.kind {
        NodeKind::Text(text_node) if is_all_ascii_whitespace(text_node.raw) => {
            php_close_gap(Some(prev), text_node.raw)
        }
        _ => None,
    }
}

/// Leading spaces/tabs of the source line containing byte offset `at`.
fn source_line_indent(source: &str, at: usize) -> &str {
    let line_start = source[..at].rfind('\n').map_or(0, |newline| newline + 1);
    let line = &source[line_start..];
    &line[..line
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count()]
}

/// Multi-line PHP kept in its source layout (no formatter result). `text` starts right after the
/// node's opening delimiter, `suffix` is printed straight after its last line, and `base_indent`
/// is the source indentation of the line the node starts on.
///
/// Each continuation line keeps its indentation relative to that line and is placed relative to
/// the node's new indentation, so a second pass shifts by nothing. A line can't go left of the
/// node's own indentation. Lines starting inside a literal are not touched at all.
/// `None` when literals can't be told apart (heredoc, unterminated string or comment).
fn reflow_shifted<'s>(
    text: &str,
    base_indent: &str,
    suffix: &str,
    line_break: LineBreak,
) -> Option<Vec<Doc<'s>>> {
    let starts_inside = literal_line_starts(text)?;
    let lines = text
        .split('\n')
        .map(|line| match line.strip_suffix('\r') {
            Some(line) => (line, true),
            None => (line, false),
        })
        .collect::<Vec<_>>();
    let last = lines.len() - 1;
    let mut docs = Vec::with_capacity(lines.len() * 2);
    for (i, &(line, _)) in lines.iter().enumerate() {
        let tail = if i == last { suffix } else { "" };
        // Trailing spaces belong to the string when it continues on the next line.
        let line = if i == last || starts_inside[i + 1] {
            line
        } else {
            line.trim_end()
        };
        if i == 0 {
            docs.push(Doc::text(format!("{line}{tail}")));
        } else if starts_inside[i] {
            push_exact_break(&mut docs, lines[i - 1].1, line_break);
            docs.push(Doc::text(format!("{line}{tail}")));
        } else if line.trim_ascii().is_empty() && tail.is_empty() {
            docs.push(Doc::empty_line());
            docs.push(Doc::text(""));
        } else {
            let content = line.trim_start_matches([' ', '\t']);
            let indent = &line[..line.len() - content.len()];
            let relative = &indent[indent.len().min(base_indent.len())..];
            docs.push(Doc::hard_line());
            docs.push(Doc::text(format!("{relative}{content}{tail}")));
        }
    }
    Some(docs)
}

/// Raw text printed byte for byte, for spans whose content the compiler or a PHP literal sees
/// as written. Unlike `reflow_raw`, a `\r\n` stays `\r\n` and a bare `\n` stays `\n` whatever
/// the printer's line break is.
pub(super) fn reflow_exact(s: &str, line_break: LineBreak) -> Vec<Doc<'_>> {
    let mut docs = Vec::new();
    let mut rest = s;
    while let Some(at) = rest.find('\n') {
        let (line, cr) = match rest[..at].strip_suffix('\r') {
            Some(line) => (line, true),
            None => (&rest[..at], false),
        };
        docs.push(Doc::text(line));
        push_exact_break(&mut docs, cr, line_break);
        rest = &rest[at + 1..];
    }
    docs.push(Doc::text(rest));
    docs
}

/// A line break inside a span that must keep its bytes; `cr` is whether the source had `\r\n`.
fn push_exact_break(docs: &mut Vec<Doc<'_>>, cr: bool, line_break: LineBreak) {
    match (line_break, cr) {
        (LineBreak::Lf, false) | (LineBreak::Crlf, true) => docs.push(Doc::empty_line()),
        // The empty line only writes the `\n`.
        (LineBreak::Lf, true) => {
            docs.push(Doc::text("\r"));
            docs.push(Doc::empty_line());
        }
        // The printer can't write a bare `\n` itself, so the column isn't reset: a following
        // group on the last line may break a little early. The bytes are what matters here.
        (LineBreak::Crlf, false) => docs.push(Doc::text("\n")),
    }
}

/// Lines of a PHP snippet ready to be printed one level deeper than their surroundings.
///
/// Lines starting inside a string literal are kept exactly as written (their indentation is part
/// of the string); the others are re-indented relative to the snippet's own minimal indentation.
struct PhpText {
    /// Text, whether the line starts inside a literal, and whether the source line ended in
    /// `\r\n` (that terminator is part of the next line's literal when it starts inside one).
    lines: Vec<(String, bool, bool)>,
}

impl PhpText {
    /// `None` when the snippet can't be split safely (heredoc, unterminated literal).
    fn new(text: &str) -> Option<Self> {
        let text = text.trim_end_matches(|c: char| c.is_ascii_whitespace());
        let mut body = text;
        let mut skipped_blank_line = false;
        while let Some((first, rest)) = body.split_once('\n') {
            if !first.trim_ascii().is_empty() {
                break;
            }
            body = rest;
            skipped_blank_line = true;
        }
        if !skipped_blank_line {
            // The first line follows a delimiter, so its own indentation means nothing.
            body = body.trim_start_matches(|c: char| c.is_ascii_whitespace());
        }
        if body.trim_ascii().is_empty() {
            return None;
        }

        let starts_inside = literal_line_starts(body)?;
        let lines = body
            .split('\n')
            .map(|line| match line.strip_suffix('\r') {
                Some(line) => (line, true),
                None => (line, false),
            })
            .collect::<Vec<_>>();
        let indent = lines
            .iter()
            .zip(&starts_inside)
            .filter(|((line, _), inside)| !**inside && !line.trim_ascii().is_empty())
            .map(|((line, _), _)| {
                line.bytes()
                    .take_while(|byte| matches!(byte, b' ' | b'\t'))
                    .count()
            })
            .min()
            .unwrap_or(0);
        let lines = lines
            .iter()
            .enumerate()
            .map(|(i, &(line, cr))| {
                if starts_inside[i] {
                    (line.to_owned(), true, cr)
                } else if line.trim_ascii().is_empty() {
                    (String::new(), false, cr)
                } else {
                    let line = &line[indent..];
                    // Trailing spaces belong to the string when it continues on the next line.
                    let line = if starts_inside.get(i + 1).copied().unwrap_or(false) {
                        line
                    } else {
                        line.trim_end()
                    };
                    (line.to_owned(), false, cr)
                }
            })
            .collect();
        Some(Self { lines })
    }

    /// The first line has no separator; the caller puts it after its own line break. Breaks
    /// inside a literal keep the terminator the source had.
    fn docs<'s>(&self, line_break: LineBreak) -> Vec<Doc<'s>> {
        let mut docs = Vec::with_capacity(self.lines.len() * 2);
        for (i, (line, inside, _)) in self.lines.iter().enumerate() {
            if i > 0 {
                if *inside {
                    push_exact_break(&mut docs, self.lines[i - 1].2, line_break);
                } else if line.is_empty() {
                    docs.push(Doc::empty_line());
                } else {
                    docs.push(Doc::hard_line());
                }
            }
            docs.push(Doc::text(line.clone()));
        }
        docs
    }
}

/// For every line of `text`, whether it begins inside a `'…'`, `"…"` or `` `…` `` literal.
/// `None` for heredocs/nowdocs and unterminated literals or comments.
fn literal_line_starts(text: &str) -> Option<Vec<bool>> {
    let bytes = text.as_bytes();
    let mut literals = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' => {
                let end = skip_single_quoted(bytes, i)?;
                literals.push((i, end));
                i = end;
            }
            b'"' | b'`' => {
                let end = skip_interpolated(bytes, i)?;
                literals.push((i, end));
                i = end;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let close = bytes[i + 2..].windows(2).position(|pair| pair == b"*/")?;
                i += 2 + close + 2;
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => i = line_end(bytes, i),
            b'#' if bytes.get(i + 1) != Some(&b'[') => i = line_end(bytes, i),
            b'<' if bytes[i..].starts_with(b"<<<") => return None,
            _ => i += 1,
        }
    }
    let mut starts = vec![false];
    for (newline, _) in bytes.iter().enumerate().filter(|(_, byte)| **byte == b'\n') {
        let line_start = newline + 1;
        starts.push(
            literals
                .iter()
                .any(|&(start, end)| start < line_start && line_start <= end),
        );
    }
    Some(starts)
}

fn line_end(bytes: &[u8], from: usize) -> usize {
    bytes[from..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(bytes.len(), |offset| from + offset)
}

/// `i` points at the opening quote; returns the offset after the closing one.
fn skip_single_quoted(bytes: &[u8], i: usize) -> Option<usize> {
    let mut j = i + 1;
    loop {
        match *bytes.get(j)? {
            b'\\' => j += 2,
            b'\'' => return Some(j + 1),
            _ => j += 1,
        }
    }
}

/// Like [`skip_single_quoted`] for `"` and `` ` `` strings, whose `{$…}`/`${…}` parts may hold
/// quotes of their own.
fn skip_interpolated(bytes: &[u8], i: usize) -> Option<usize> {
    let quote = bytes[i];
    let mut j = i + 1;
    loop {
        match *bytes.get(j)? {
            b'\\' => j += 2,
            byte if byte == quote => return Some(j + 1),
            b'{' if bytes.get(j + 1) == Some(&b'$') => j = skip_braces(bytes, j + 1)?,
            b'$' if bytes.get(j + 1) == Some(&b'{') => j = skip_braces(bytes, j + 2)?,
            _ => j += 1,
        }
    }
}

/// `from` is just after an opening `{`; returns the offset after its matching `}`.
fn skip_braces(bytes: &[u8], from: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut j = from;
    loop {
        match *bytes.get(j)? {
            b'\'' => j = skip_single_quoted(bytes, j)?,
            b'"' | b'`' => j = skip_interpolated(bytes, j)?,
            b'{' => {
                depth += 1;
                j += 1;
            }
            b'}' => {
                depth -= 1;
                j += 1;
                if depth == 0 {
                    return Some(j);
                }
            }
            _ => j += 1,
        }
    }
}
