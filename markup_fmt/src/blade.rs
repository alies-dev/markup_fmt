//! Blade directive classification and lexical helpers.
//!
//! The built-in table is pinned to Laravel v13.38.0 (`BladeCompiler` `compile*` methods).

pub(crate) mod php;

use crate::{
    ast::BladeDirective,
    config::{BladeCustomBlockDirective, BladePreset, LanguageOptions},
};
use std::{collections::HashMap, ops::Range};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DirectiveRole {
    Unknown,
    Inline,
    Opener,
    Branch,
    Closer,
}

/// Branches and closers of a built-in block (lowercase names).
#[derive(Clone, Copy)]
struct Block {
    branches: &'static [&'static str],
    closers: &'static [&'static str],
    /// Compiled to a PHP `if`, so `@elseif` and `@else` are valid branches too.
    if_like: bool,
}

/// Rule deciding at runtime whether a directive opens a block or stands alone.
#[derive(Clone, Copy)]
enum Arity {
    /// `@section`: one argument, or `null` as the second one.
    NullSecond,
    /// `@slot`: one argument opens a capture, exactly two never do (`ManagesComponents::slot`
    /// stores the second one, `null` included), three or more only with a literal `null` second.
    Slot,
    /// `@push`, `@prepend`: one argument, or `''`/`""` as the second one.
    EmptyStringSecond,
    /// `@lang`: no arguments, or arguments starting right after `(` with `[`.
    Lang,
    /// `@empty`: opener with arguments, `@forelse` branch without.
    Empty,
}

#[derive(Clone, Copy)]
enum Kind {
    Inline,
    Branch,
    Closer,
    Opener(Block),
    Conditional(Arity, Block),
}

struct Builtin {
    /// Lowercase.
    name: &'static str,
    kind: Kind,
    /// Whether the arguments are PHP expressions that may go to the external PHP formatter.
    formats_args: bool,
}

const fn inline(name: &'static str) -> Builtin {
    Builtin {
        name,
        kind: Kind::Inline,
        formats_args: true,
    }
}

const fn opaque(name: &'static str) -> Builtin {
    Builtin {
        name,
        kind: Kind::Inline,
        formats_args: false,
    }
}

const fn branch(name: &'static str) -> Builtin {
    Builtin {
        name,
        kind: Kind::Branch,
        formats_args: true,
    }
}

const fn closer(name: &'static str) -> Builtin {
    Builtin {
        name,
        kind: Kind::Closer,
        formats_args: true,
    }
}

const fn block(
    name: &'static str,
    branches: &'static [&'static str],
    closers: &'static [&'static str],
) -> Builtin {
    Builtin {
        name,
        kind: Kind::Opener(Block {
            branches,
            closers,
            if_like: false,
        }),
        formats_args: true,
    }
}

const fn if_block(
    name: &'static str,
    branches: &'static [&'static str],
    closers: &'static [&'static str],
) -> Builtin {
    Builtin {
        name,
        kind: Kind::Opener(Block {
            branches,
            closers,
            if_like: true,
        }),
        formats_args: true,
    }
}

const fn conditional(
    name: &'static str,
    arity: Arity,
    branches: &'static [&'static str],
    closers: &'static [&'static str],
    if_like: bool,
) -> Builtin {
    Builtin {
        name,
        kind: Kind::Conditional(
            arity,
            Block {
                branches,
                closers,
                if_like,
            },
        ),
        formats_args: true,
    }
}

/// Longest name in `BUILTINS`; longer names can't match, which lets lookup lowercase on the stack.
const MAX_BUILTIN_NAME_LEN: usize = 17;

/// Sorted by `name` (binary search); a unit test enforces this.
///
/// Besides the Laravel inventory it holds `endphp`, `endverbatim` and `verbatim`: the compiler
/// extracts those spans before directives are recognized, so a stray marker is inert and the parser
/// keeps it as an opaque directive.
static BUILTINS: &[Builtin] = &[
    closer("append"),
    if_block("auth", &["elseauth"], &["endauth"]),
    inline("aware"),
    inline("bool"),
    opaque("break"),
    if_block("can", &["elsecan"], &["endcan"]),
    if_block("canany", &["elsecanany"], &["endcanany"]),
    if_block("cannot", &["elsecannot"], &["endcannot"]),
    branch("case"),
    inline("checked"),
    inline("choice"),
    inline("class"),
    block("component", &[], &["endcomponent"]),
    block("componentfirst", &[], &["endcomponentfirst"]),
    if_block("context", &[], &["endcontext"]),
    opaque("continue"),
    inline("csrf"),
    inline("dd"),
    branch("default"),
    inline("disabled"),
    inline("dump"),
    inline("each"),
    branch("else"),
    branch("elseauth"),
    branch("elsecan"),
    branch("elsecanany"),
    branch("elsecannot"),
    branch("elseguest"),
    branch("elseif"),
    branch("elsepush"),
    branch("elsepushif"),
    conditional("empty", Arity::Empty, &[], &["endempty"], true),
    closer("endauth"),
    closer("endcan"),
    closer("endcanany"),
    closer("endcannot"),
    closer("endcomponent"),
    closer("endcomponentfirst"),
    closer("endcontext"),
    closer("endempty"),
    closer("endenv"),
    closer("enderror"),
    closer("endfor"),
    closer("endforeach"),
    closer("endforelse"),
    closer("endfragment"),
    closer("endguest"),
    closer("endif"),
    closer("endisset"),
    closer("endlang"),
    closer("endonce"),
    opaque("endphp"),
    closer("endprepend"),
    closer("endprependonce"),
    closer("endproduction"),
    closer("endpush"),
    closer("endpushif"),
    closer("endpushonce"),
    closer("endsection"),
    closer("endsession"),
    closer("endslot"),
    closer("endswitch"),
    closer("endunless"),
    opaque("endverbatim"),
    closer("endwhile"),
    if_block("env", &[], &["endenv"]),
    if_block("error", &[], &["enderror"]),
    inline("extends"),
    inline("extendsfirst"),
    inline("fonts"),
    block("for", &[], &["endfor"]),
    block("foreach", &[], &["endforeach"]),
    block("forelse", &["empty"], &["endforelse"]),
    block("fragment", &[], &["endfragment"]),
    if_block("guest", &["elseguest"], &["endguest"]),
    if_block("hassection", &[], &["endif"]),
    if_block("hasstack", &[], &["endif"]),
    if_block("if", &[], &["endif"]),
    inline("include"),
    inline("includefirst"),
    inline("includeif"),
    inline("includeisolated"),
    inline("includeunless"),
    inline("includewhen"),
    inline("inject"),
    if_block("isset", &[], &["endisset"]),
    inline("js"),
    inline("json"),
    conditional("lang", Arity::Lang, &[], &["endlang"], false),
    inline("method"),
    block("once", &[], &["endonce"]),
    closer("overwrite"),
    inline("parent"),
    inline("php"),
    conditional(
        "prepend",
        Arity::EmptyStringSecond,
        &[],
        &["endprepend"],
        false,
    ),
    block("prependonce", &[], &["endprependonce"]),
    if_block("production", &[], &["endproduction"]),
    inline("props"),
    conditional("push", Arity::EmptyStringSecond, &[], &["endpush"], false),
    block("pushif", &["elsepushif", "elsepush"], &["endpushif"]),
    block("pushonce", &[], &["endpushonce"]),
    inline("readonly"),
    inline("required"),
    conditional(
        "section",
        Arity::NullSecond,
        &[],
        &["endsection", "show", "stop", "append", "overwrite"],
        false,
    ),
    if_block("sectionmissing", &[], &["endif"]),
    inline("selected"),
    if_block("session", &[], &["endsession"]),
    closer("show"),
    conditional("slot", Arity::Slot, &[], &["endslot"], false),
    inline("stack"),
    closer("stop"),
    inline("style"),
    block("switch", &["case", "default"], &["endswitch"]),
    if_block("unless", &[], &["endunless"]),
    inline("unset"),
    opaque("use"),
    opaque("verbatim"),
    inline("vite"),
    inline("vitereactrefresh"),
    block("while", &[], &["endwhile"]),
    inline("yield"),
];

fn builtin(name: &str) -> Option<&'static Builtin> {
    if name.len() > MAX_BUILTIN_NAME_LEN {
        return None;
    }
    let mut buf = [0u8; MAX_BUILTIN_NAME_LEN];
    let buf = &mut buf[..name.len()];
    buf.copy_from_slice(name.as_bytes());
    buf.make_ascii_lowercase();
    // ASCII lowercasing keeps valid UTF-8 valid.
    let key = std::str::from_utf8(buf).ok()?;
    BUILTINS
        .binary_search_by(|builtin| builtin.name.cmp(key))
        .ok()
        .map(|index| &BUILTINS[index])
}

impl Arity {
    fn opens(self, args: Option<&str>) -> bool {
        match self {
            Arity::Lang => args.is_none_or(|args| args.starts_with('[')),
            Arity::Empty => args.is_some(),
            Arity::Slot => {
                let Some(args) = args else {
                    return false;
                };
                let args = split_top_level_args(args);
                match args.as_slice() {
                    [] | [_] => true,
                    [_, _] => false,
                    [_, second, ..] => second.eq_ignore_ascii_case("null"),
                }
            }
            Arity::NullSecond | Arity::EmptyStringSecond => {
                // Without parentheses the compiler emits invalid PHP; never open a block for it.
                let Some(args) = args else {
                    return false;
                };
                let args = split_top_level_args(args);
                match args.get(1) {
                    None => true,
                    Some(second) => match self {
                        Arity::NullSecond => second.eq_ignore_ascii_case("null"),
                        _ => matches!(*second, "''" | "\"\""),
                    },
                }
            }
        }
    }
}

enum Custom {
    Inline,
    /// Index into `BladeRegistry::blocks`.
    Opener(usize),
    Branch,
    Closer,
}

struct CustomBlock {
    branches: Vec<String>,
    closers: Vec<String>,
    raw: bool,
    if_like: bool,
}

enum BlockRef<'a> {
    Builtin(&'static Block),
    Custom(&'a CustomBlock),
}

pub(crate) struct BladeRegistry {
    custom: HashMap<String, Custom>,
    blocks: Vec<CustomBlock>,
}

impl BladeRegistry {
    pub(crate) fn new(options: &LanguageOptions) -> Self {
        let mut registry = Self {
            custom: HashMap::new(),
            blocks: Vec::new(),
        };
        for name in &options.blade_custom_directives {
            registry.custom.insert(name.clone(), Custom::Inline);
        }
        for preset in &options.blade_presets {
            registry.add_preset(preset);
        }
        for name in &options.blade_custom_if_directives {
            let else_name = format!("else{name}");
            let end_name = format!("end{name}");
            let unless_name = format!("unless{name}");
            registry.add_block(
                &[name, &unless_name],
                &[&else_name],
                &[&end_name],
                false,
                true,
            );
        }
        for BladeCustomBlockDirective {
            name,
            branches,
            closers,
            raw,
        } in &options.blade_custom_block_directives
        {
            let branches = branches.iter().map(String::as_str).collect::<Vec<_>>();
            let closers = closers.iter().map(String::as_str).collect::<Vec<_>>();
            registry.add_block(&[name], &branches, &closers, *raw, false);
        }
        registry
    }

    fn add_preset(&mut self, preset: &BladePreset) {
        match preset {
            BladePreset::Livewire => {
                for name in [
                    "livewire",
                    "livewireStyles",
                    "livewireScripts",
                    "livewireScriptConfig",
                    "entangle",
                    "this",
                ] {
                    self.custom.insert(name.into(), Custom::Inline);
                }
                self.add_block(&["persist"], &[], &["endpersist"], false, false);
                self.add_block(&["teleport"], &[], &["endteleport"], false, false);
                self.add_block(&["assets"], &[], &["endassets"], false, false);
                self.add_block(&["script"], &[], &["endscript"], true, false);
            }
            BladePreset::Volt => self.add_block(&["volt"], &[], &["endvolt"], false, false),
            BladePreset::Inertia => {
                for name in ["inertia", "inertiaHead"] {
                    self.custom.insert(name.into(), Custom::Inline);
                }
            }
            BladePreset::Ziggy => {
                self.custom.insert("routes".into(), Custom::Inline);
            }
            BladePreset::Filament => {
                for name in ["filamentStyles", "filamentScripts"] {
                    self.custom.insert(name.into(), Custom::Inline);
                }
            }
        }
    }

    /// Later registrations win over earlier ones for the same opener name; a branch or closer
    /// name never replaces an opener.
    fn add_block<S: AsRef<str>>(
        &mut self,
        openers: &[S],
        branches: &[&str],
        closers: &[&str],
        raw: bool,
        if_like: bool,
    ) {
        let index = self.blocks.len();
        self.blocks.push(CustomBlock {
            branches: branches.iter().map(|name| name.to_string()).collect(),
            closers: closers.iter().map(|name| name.to_string()).collect(),
            raw,
            if_like,
        });
        for name in openers {
            self.custom
                .insert(name.as_ref().to_owned(), Custom::Opener(index));
        }
        for name in branches {
            self.add_secondary(name, Custom::Branch);
        }
        for name in closers {
            self.add_secondary(name, Custom::Closer);
        }
    }

    fn add_secondary(&mut self, name: &str, custom: Custom) {
        if !matches!(self.custom.get(name), Some(Custom::Opener(_))) {
            self.custom.insert(name.to_owned(), custom);
        }
    }

    /// Role of `@name(args)` in isolation. `args` = Some(inner text) when parens present.
    pub(crate) fn role(&self, name: &str, args: Option<&str>) -> DirectiveRole {
        if let Some(custom) = self.custom.get(name) {
            return match custom {
                Custom::Inline => DirectiveRole::Inline,
                Custom::Opener(_) => DirectiveRole::Opener,
                Custom::Branch => DirectiveRole::Branch,
                Custom::Closer => DirectiveRole::Closer,
            };
        }
        let Some(builtin) = builtin(name) else {
            return DirectiveRole::Unknown;
        };
        match builtin.kind {
            Kind::Inline => DirectiveRole::Inline,
            Kind::Branch => DirectiveRole::Branch,
            Kind::Closer => DirectiveRole::Closer,
            Kind::Opener(_) => DirectiveRole::Opener,
            Kind::Conditional(Arity::Empty, _) => {
                if args.is_some() {
                    DirectiveRole::Opener
                } else {
                    DirectiveRole::Branch
                }
            }
            Kind::Conditional(arity, _) => {
                if arity.opens(args) {
                    DirectiveRole::Opener
                } else {
                    DirectiveRole::Inline
                }
            }
        }
    }

    fn block_of(&self, name: &str) -> Option<BlockRef<'_>> {
        if let Some(custom) = self.custom.get(name) {
            return match custom {
                Custom::Opener(index) => Some(BlockRef::Custom(&self.blocks[*index])),
                _ => None,
            };
        }
        match &builtin(name)?.kind {
            Kind::Opener(block) | Kind::Conditional(_, block) => Some(BlockRef::Builtin(block)),
            _ => None,
        }
    }

    /// Whether `candidate` (a directive) may appear as a branch of the block opened by `opener`.
    pub(crate) fn is_branch_of(&self, opener: &BladeDirective, candidate: &BladeDirective) -> bool {
        let role = self.role(candidate.name, candidate.args.map(|(args, _)| args));
        if role != DirectiveRole::Branch {
            return false;
        }
        let name = candidate.name;
        let is_else = name.eq_ignore_ascii_case("else") || name.eq_ignore_ascii_case("elseif");
        match self.block_of(opener.name) {
            Some(BlockRef::Builtin(block)) => {
                block
                    .branches
                    .iter()
                    .any(|branch| branch.eq_ignore_ascii_case(name))
                    || (block.if_like && is_else)
            }
            Some(BlockRef::Custom(block)) => {
                block.branches.iter().any(|branch| branch == name) || (block.if_like && is_else)
            }
            None => false,
        }
    }

    /// Whether `candidate` closes the block opened by `opener`. Every block compiled to a PHP
    /// `if` also ends at `@endif`, which compiles to the same `endif;`.
    pub(crate) fn is_closer_of(&self, opener: &BladeDirective, candidate: &BladeDirective) -> bool {
        let role = self.role(candidate.name, candidate.args.map(|(args, _)| args));
        if role != DirectiveRole::Closer {
            return false;
        }
        let name = candidate.name;
        match self.block_of(opener.name) {
            Some(BlockRef::Builtin(block)) => {
                block
                    .closers
                    .iter()
                    .any(|closer| closer.eq_ignore_ascii_case(name))
                    || (block.if_like && name.eq_ignore_ascii_case("endif"))
            }
            Some(BlockRef::Custom(block)) => {
                block.closers.iter().any(|closer| closer == name)
                    || (block.if_like && name.eq_ignore_ascii_case("endif"))
            }
            None => false,
        }
    }

    /// Raw-body custom block (`@script…@endscript`): body kept verbatim, returns closer names.
    pub(crate) fn raw_body_closers(&self, name: &str) -> Option<&[String]> {
        match self.custom.get(name)? {
            Custom::Opener(index) if self.blocks[*index].raw => Some(&self.blocks[*index].closers),
            _ => None,
        }
    }

    /// Directive whose args are sent to the PHP formatter (built-ins only; never custom/`@use`).
    pub(crate) fn formats_args(&self, name: &str) -> bool {
        if self.custom.contains_key(name) {
            return false;
        }
        builtin(name).is_some_and(|builtin| builtin.formats_args)
    }
}

/// ASCII `[A-Za-z0-9_]`, the `\w` of the (non-Unicode) PCRE patterns the compiler uses.
pub(crate) fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// A directive token found by [`scan_directive`]; all ranges are byte offsets into the source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DirectiveToken {
    /// `@?\w+(::\w+)?` after the `@`. A leading second `@` (`@@name`) is part of the name.
    pub name: Range<usize>,
    /// Spaces/tabs between the name and `(`. Empty when there are no args.
    pub space: Range<usize>,
    /// Text strictly inside the outer parentheses.
    pub args: Option<Range<usize>>,
    /// Offset just past the token.
    pub end: usize,
}

/// Lexes `@name(::sub)?[ \t]*(args)?` at `at`, which must point at `@`.
///
/// Mirrors the compiler's `\B@(@?\w+(?:::\w+)?)([ \t]*)(\(...\))?` pattern: the previous char must
/// not be a word char. The parentheses are balanced token-aware (PHP strings and comments are
/// skipped); when they are not balanced the token is just `@name`. `index` remembers failed
/// parenthesis searches and belongs to one `src`: without it every `@name(` of a source with
/// unbalanced parentheses rescans up to the end of the input.
pub(crate) fn scan_directive(
    src: &str,
    at: usize,
    index: &mut ParenIndex,
) -> Option<DirectiveToken> {
    let bytes = src.as_bytes();
    if bytes.get(at) != Some(&b'@') {
        return None;
    }
    if src[..at].chars().next_back().is_some_and(is_word_char) {
        return None;
    }

    let name_start = at + 1;
    let mut i = name_start;
    if bytes.get(i) == Some(&b'@') {
        i += 1;
    }
    let word_start = i;
    while bytes.get(i).copied().is_some_and(is_word_byte) {
        i += 1;
    }
    if i == word_start {
        return None;
    }
    if bytes[i..].starts_with(b"::") {
        let mut j = i + 2;
        while bytes.get(j).copied().is_some_and(is_word_byte) {
            j += 1;
        }
        if j > i + 2 {
            i = j;
        }
    }
    let name_end = i;

    let mut paren = name_end;
    while matches!(bytes.get(paren), Some(b' ' | b'\t')) {
        paren += 1;
    }
    if bytes.get(paren) == Some(&b'(')
        && let Some(close) = matching_paren(bytes, paren, index)
    {
        return Some(DirectiveToken {
            name: name_start..name_end,
            space: name_end..paren,
            args: Some(paren + 1..close),
            end: close + 1,
        });
    }
    Some(DirectiveToken {
        name: name_start..name_end,
        space: name_end..name_end,
        args: None,
        end: name_end,
    })
}

/// `open` points at `(`; returns the offset of the matching `)`.
fn find_closing_paren(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        if let Some(next) = skip_php_literal(bytes, i) {
            i = next;
            continue;
        }
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// For the `(` of every failed [`find_closing_paren`] scan: the match of each `(` that scan
/// passed. Lexing from a `(` that the earlier scan saw as code is the tail of that scan, so these
/// answers equal a fresh search.
#[derive(Default)]
pub(crate) struct ParenIndex {
    /// One sorted list of `(open, close)` per failed scan.
    scans: Vec<Vec<(usize, Option<usize>)>>,
}

fn matching_paren(bytes: &[u8], open: usize, index: &mut ParenIndex) -> Option<usize> {
    for scan in &index.scans {
        if let Ok(found) = scan.binary_search_by_key(&open, |&(paren, _)| paren) {
            return scan[found].1;
        }
    }
    let close = find_closing_paren(bytes, open);
    if close.is_none() {
        index.scans.push(index_parens(bytes, open));
    }
    close
}

/// Same lexing as [`find_closing_paren`], matching every `(` up to the end of input.
fn index_parens(bytes: &[u8], open: usize) -> Vec<(usize, Option<usize>)> {
    let mut entries: Vec<(usize, Option<usize>)> = vec![];
    let mut stack: Vec<usize> = vec![];
    let mut i = open;
    while i < bytes.len() {
        if let Some(next) = skip_php_literal(bytes, i) {
            i = next;
            continue;
        }
        match bytes[i] {
            b'(' => {
                stack.push(entries.len());
                entries.push((i, None));
            }
            b')' => {
                if let Some(entry) = stack.pop() {
                    entries[entry].1 = Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    entries
}

/// If a PHP string or comment starts at `i`, returns the offset just past it (the end of input
/// when unterminated; line comments stop before the newline).
fn skip_php_literal(bytes: &[u8], i: usize) -> Option<usize> {
    match bytes[i] {
        quote @ (b'\'' | b'"') => {
            let mut j = i + 1;
            while j < bytes.len() {
                match bytes[j] {
                    b'\\' => j += 2,
                    byte if byte == quote => return Some(j + 1),
                    _ => j += 1,
                }
            }
            Some(bytes.len())
        }
        b'/' => match bytes.get(i + 1) {
            Some(b'*') => Some(
                bytes[i + 2..]
                    .windows(2)
                    .position(|window| window == b"*/")
                    .map_or(bytes.len(), |pos| i + 2 + pos + 2),
            ),
            Some(b'/') => Some(line_end(bytes, i + 2)),
            _ => None,
        },
        // `#[` starts an attribute, not a comment.
        b'#' if bytes.get(i + 1) != Some(&b'[') => Some(line_end(bytes, i + 1)),
        _ => None,
    }
}

fn line_end(bytes: &[u8], from: usize) -> usize {
    bytes[from.min(bytes.len())..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(bytes.len(), |pos| from + pos)
}

/// Splits `args` at commas outside strings, comments and `()`/`[]`/`{}`; pieces are trimmed.
/// Blank input yields no pieces and a trailing comma yields no trailing empty piece.
pub(crate) fn split_top_level_args(args: &str) -> Vec<&str> {
    let bytes = args.as_bytes();
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        if let Some(next) = skip_php_literal(bytes, i) {
            i = next;
            continue;
        }
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                parts.push(args[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    let last = args[start..].trim();
    if !last.is_empty() {
        parts.push(last);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> BladeRegistry {
        BladeRegistry::new(&LanguageOptions::default())
    }

    fn directive<'s>(name: &'s str, args: Option<&'s str>) -> BladeDirective<'s> {
        BladeDirective {
            name,
            space: "",
            args: args.map(|args| (args, 0)),
        }
    }

    fn scan(src: &str, at: usize) -> Option<(&str, &str, Option<&str>, usize)> {
        scan_directive(src, at, &mut ParenIndex::default()).map(|token| {
            (
                &src[token.name],
                &src[token.space],
                token.args.map(|args| &src[args]),
                token.end,
            )
        })
    }

    #[test]
    fn builtin_table_is_sorted_lowercase_and_bounded() {
        for pair in BUILTINS.windows(2) {
            assert!(
                pair[0].name < pair[1].name,
                "{} / {}",
                pair[0].name,
                pair[1].name
            );
        }
        for builtin in BUILTINS {
            assert_eq!(builtin.name, builtin.name.to_ascii_lowercase());
            assert!(builtin.name.len() <= MAX_BUILTIN_NAME_LEN);
        }
        assert_eq!(
            BUILTINS.iter().map(|builtin| builtin.name.len()).max(),
            Some(MAX_BUILTIN_NAME_LEN)
        );
    }

    #[test]
    fn every_pinned_laravel_directive_is_known() {
        // Shared with `scripts/blade/drift.php`, which compares it with Laravel's compile* methods.
        let inventory = include_str!("../../scripts/blade/known-compile-methods.txt");
        let registry = registry();
        let names = inventory
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(|line| line.strip_prefix("compile").unwrap_or(line));
        for name in names {
            for args in [None, Some("$x")] {
                assert_ne!(
                    registry.role(name, args),
                    DirectiveRole::Unknown,
                    "{name} {args:?}"
                );
            }
        }
    }

    #[test]
    fn builtins_ignore_case_customs_do_not() {
        let options = LanguageOptions {
            blade_custom_directives: vec!["Foo".into(), "if".into()],
            ..Default::default()
        };
        let registry = BladeRegistry::new(&options);
        assert_eq!(registry.role("ENDPushOnce", None), DirectiveRole::Closer);
        assert_eq!(registry.role("ElseIf", Some("$a")), DirectiveRole::Branch);
        assert_eq!(registry.role("Foo", None), DirectiveRole::Inline);
        assert_eq!(registry.role("foo", None), DirectiveRole::Unknown);
        // A custom directive overrides the built-in with the identical spelling only.
        assert_eq!(registry.role("if", Some("$a")), DirectiveRole::Inline);
        assert_eq!(registry.role("If", Some("$a")), DirectiveRole::Opener);
        assert!(!registry.formats_args("if"));
        assert!(registry.formats_args("If"));
        assert!(!registry.formats_args("Foo"));
        assert!(!registry.formats_args("use"));
        assert!(!registry.formats_args("verbatim"));
        assert!(!registry.formats_args("unknown"));
    }

    /// Laravel classifies `@break(N)`/`@continue(N)` args by regex on the raw text (integer =
    /// nesting level, else condition), so redundant-paren removal would change semantics.
    #[test]
    fn break_and_continue_args_are_not_formatted() {
        let registry = registry();
        assert!(!registry.formats_args("break"));
        assert!(!registry.formats_args("Continue"));
        assert!(registry.formats_args("dd"));
    }

    #[test]
    fn arity_rules() {
        let registry = registry();
        let role = |name, args| registry.role(name, args);
        use DirectiveRole::*;

        assert_eq!(role("section", Some("'a'")), Opener);
        assert_eq!(role("section", Some("'a', 'b'")), Inline);
        assert_eq!(role("section", Some("'a', $content")), Inline);
        assert_eq!(role("section", Some("'a', null")), Opener);
        assert_eq!(role("section", Some("'a', NULL")), Opener);
        assert_eq!(role("section", Some("'a', null, 1")), Opener);
        assert_eq!(role("section", Some("'a, b'")), Opener);
        assert_eq!(role("section", Some("foo('a', 'b')")), Opener);
        assert_eq!(role("section", None), Inline);

        assert_eq!(role("slot", Some("'a'")), Opener);
        assert_eq!(role("slot", Some("'a', 'Text'")), Inline);
        assert_eq!(role("slot", Some("'a', null, ['class' => 'x']")), Opener);
        // Exactly two arguments is the inline form even when the second is `null`.
        assert_eq!(role("slot", Some("'a', null")), Inline);
        assert_eq!(role("slot", Some("'a', NULL")), Inline);
        assert_eq!(role("slot", Some("'a', $content")), Inline);
        assert_eq!(role("slot", Some("'a', null,")), Inline);
        assert_eq!(role("slot", Some("'a', null, []")), Opener);
        assert_eq!(role("slot", Some("'a', 'Text', []")), Inline);
        assert_eq!(role("slot", Some("'a', $x, ['class' => 'x']")), Inline);
        assert_eq!(role("slot", Some("'a',")), Opener);
        assert_eq!(role("slot", None), Inline);

        assert_eq!(role("push", Some("'s'")), Opener);
        assert_eq!(role("push", Some("'s', ''")), Opener);
        assert_eq!(role("push", Some("'s', \"\"")), Opener);
        assert_eq!(role("push", Some("'s', 'x'")), Inline);
        assert_eq!(role("push", Some("'s', $x")), Inline);
        assert_eq!(role("push", Some("'s', null")), Inline);
        assert_eq!(role("prepend", Some("'s'")), Opener);
        assert_eq!(role("prepend", Some("'s', 'x'")), Inline);
        assert_eq!(role("pushOnce", Some("'s', 'id'")), Opener);

        assert_eq!(role("lang", None), Opener);
        assert_eq!(role("lang", Some("['k' => 'v']")), Opener);
        assert_eq!(role("lang", Some(" ['k' => 'v']")), Inline);
        assert_eq!(role("lang", Some("array('k')")), Inline);
        assert_eq!(role("lang", Some("'key'")), Inline);

        assert_eq!(role("empty", None), Branch);
        assert_eq!(role("empty", Some("$x")), Opener);
        assert_eq!(role("empty", Some("")), Opener);

        assert_eq!(role("php", Some("$x = 1")), Inline);
        assert_eq!(role("php", None), Inline);
        assert_eq!(role("endphp", None), Inline);
        assert_eq!(role("endverbatim", None), Inline);
    }

    #[test]
    fn paren_index_answers_like_a_fresh_scan() {
        let sources = [
            "@a( @b(x) @c( ')' @d(y) ",
            "@a( '@b(' @c(x) '@d(' )",
            "@a( /* ( */ @b() // (\n @c(z",
            "@a(((( @b() )) @c(",
            "@a( @b( @c( @d(1) ) ",
        ];
        for src in sources {
            let mut index = ParenIndex::default();
            for (at, _) in src.match_indices('@') {
                assert_eq!(
                    scan_directive(src, at, &mut index),
                    scan_directive(src, at, &mut ParenIndex::default()),
                    "{src:?} at {at}"
                );
            }
        }
    }

    #[test]
    fn unbalanced_directive_parens_scan_in_linear_time() {
        let src = "@if( ".repeat(200_000);
        let mut index = ParenIndex::default();
        let started = std::time::Instant::now();
        for (at, _) in src.match_indices('@') {
            let token = scan_directive(&src, at, &mut index).unwrap();
            assert!(token.args.is_none());
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn builtin_block_shapes() {
        let registry = registry();
        let branch_of = |opener: &str, candidate: &str, args| {
            registry.is_branch_of(&directive(opener, Some("$x")), &directive(candidate, args))
        };
        let closer_of = |opener: &str, candidate: &str| {
            registry.is_closer_of(&directive(opener, Some("$x")), &directive(candidate, None))
        };

        assert!(branch_of("if", "else", None));
        assert!(branch_of("if", "ELSEIF", Some("$y")));
        assert!(branch_of("unless", "else", None));
        assert!(branch_of("hasSection", "elseif", Some("$y")));
        assert!(branch_of("auth", "elseauth", None));
        assert!(branch_of("auth", "else", None));
        assert!(!branch_of("auth", "elsecan", None));
        assert!(!branch_of("for", "else", None));
        assert!(!branch_of("once", "else", None));
        assert!(branch_of("pushIf", "elsePushIf", Some("$c, 's'")));
        assert!(branch_of("pushIf", "elsePush", Some("'s'")));
        assert!(!branch_of("pushIf", "else", None));
        assert!(branch_of("switch", "case", Some("1")));
        assert!(branch_of("switch", "default", None));
        assert!(!branch_of("switch", "break", None));
        assert!(branch_of("forelse", "empty", None));
        assert!(!branch_of("forelse", "empty", Some("$x")));
        assert!(!branch_of("if", "empty", None));

        assert!(closer_of("if", "endif"));
        assert!(closer_of("if", "ENDIF"));
        assert!(!closer_of("if", "endunless"));
        for opener in [
            "unless",
            "isset",
            "empty",
            "auth",
            "guest",
            "can",
            "cannot",
            "canany",
            "env",
            "production",
            "error",
            "session",
            "context",
        ] {
            assert!(closer_of(opener, "endif"), "{opener}");
        }
        assert!(closer_of("unless", "endunless"));
        assert!(closer_of("auth", "endauth"));
        assert!(!closer_of("auth", "endguest"));
        assert!(!closer_of("for", "endif"));
        assert!(!closer_of("push", "endif"));
        assert!(!closer_of("lang", "endif"));
        assert!(closer_of("hasSection", "endif"));
        assert!(closer_of("sectionMissing", "endif"));
        assert!(closer_of("hasstack", "endif"));
        assert!(closer_of("forelse", "endforelse"));
        assert!(!closer_of("forelse", "endforeach"));
        assert!(closer_of("empty", "endempty"));
        assert!(closer_of("pushOnce", "endPushOnce"));
        for name in ["endsection", "show", "stop", "append", "overwrite"] {
            assert!(closer_of("section", name), "{name}");
        }
        assert!(!closer_of("slot", "show"));
    }

    #[test]
    fn custom_blocks_and_presets() {
        let options = LanguageOptions {
            blade_custom_if_directives: vec!["admin".into()],
            blade_custom_block_directives: vec![BladeCustomBlockDirective {
                name: "Wrap".into(),
                branches: vec!["mid".into()],
                closers: vec!["endWrap".into()],
                raw: false,
            }],
            blade_presets: vec![BladePreset::Livewire, BladePreset::Ziggy],
            ..Default::default()
        };
        let registry = BladeRegistry::new(&options);
        use DirectiveRole::*;

        assert_eq!(registry.role("admin", Some("$u")), Opener);
        assert_eq!(registry.role("unlessadmin", None), Opener);
        assert_eq!(registry.role("elseadmin", None), Branch);
        assert_eq!(registry.role("endadmin", None), Closer);
        assert_eq!(registry.role("Admin", None), Unknown);
        let admin = directive("admin", None);
        assert!(registry.is_branch_of(&admin, &directive("elseadmin", None)));
        assert!(registry.is_branch_of(&admin, &directive("else", None)));
        assert!(registry.is_branch_of(&admin, &directive("elseif", Some("$a"))));
        assert!(registry.is_closer_of(&admin, &directive("endadmin", None)));
        assert!(registry.is_closer_of(&admin, &directive("endif", None)));
        let unless = directive("unlessadmin", None);
        assert!(registry.is_closer_of(&unless, &directive("endadmin", None)));
        assert!(registry.is_closer_of(&unless, &directive("endif", None)));
        assert!(!registry.is_closer_of(&directive("Wrap", None), &directive("endif", None)));

        let wrap = directive("Wrap", None);
        assert_eq!(registry.role("wrap", None), Unknown);
        assert!(registry.is_branch_of(&wrap, &directive("mid", None)));
        assert!(!registry.is_branch_of(&wrap, &directive("else", None)));
        assert!(registry.is_closer_of(&wrap, &directive("endWrap", None)));
        assert!(!registry.is_closer_of(&wrap, &directive("endwrap", None)));
        assert_eq!(registry.raw_body_closers("Wrap"), None);

        assert_eq!(registry.role("livewire", Some("'x'")), Inline);
        assert_eq!(registry.role("entangle", Some("'x'")), Inline);
        assert_eq!(registry.role("routes", None), Inline);
        assert_eq!(registry.role("persist", None), Opener);
        assert_eq!(
            registry.raw_body_closers("script"),
            Some(&["endscript".to_owned()][..])
        );
        assert_eq!(registry.raw_body_closers("persist"), None);
        assert_eq!(registry.role("endscript", None), Closer);
        assert_eq!(registry.role("volt", None), Unknown);
    }

    #[test]
    fn scan_requires_non_word_char_before_at() {
        assert_eq!(scan("user@example.com", 4), None);
        assert_eq!(scan("a_@if", 2), None);
        assert_eq!(scan("é@if", 2), Some(("if", "", None, 5)));
        assert_eq!(scan("x @if", 2), Some(("if", "", None, 5)));
        assert_eq!(scan("@if", 0), Some(("if", "", None, 3)));
        assert_eq!(scan("@", 0), None);
        assert_eq!(scan("@ if", 0), None);
        assert_eq!(scan("if", 0), None);
    }

    #[test]
    fn scan_names() {
        assert_eq!(scan("@endPushOnce", 0), Some(("endPushOnce", "", None, 12)));
        assert_eq!(
            scan("@name::sub(1)", 0),
            Some(("name::sub", "", Some("1"), 13))
        );
        assert_eq!(scan("@a::", 0), Some(("a", "", None, 2)));
        assert_eq!(scan("@a:b", 0), Some(("a", "", None, 2)));
        assert_eq!(scan("@@php", 0), Some(("@php", "", None, 5)));
        assert_eq!(scan("@@", 0), None);
        assert_eq!(scan("@123", 0), Some(("123", "", None, 4)));
    }

    #[test]
    fn scan_args() {
        assert_eq!(scan("@if (x)", 0), Some(("if", " ", Some("x"), 7)));
        assert_eq!(scan("@if   (x)", 0), Some(("if", "   ", Some("x"), 9)));
        assert_eq!(scan("@if\t(x)", 0), Some(("if", "\t", Some("x"), 7)));
        assert_eq!(scan("@if()", 0), Some(("if", "", Some(""), 5)));
        // Newlines are not part of `[ \t]*`.
        assert_eq!(scan("@if\n(x)", 0), Some(("if", "", None, 3)));
        // Whitespace not followed by `(` is not part of the token.
        assert_eq!(scan("@else x", 0), Some(("else", "", None, 5)));
        assert_eq!(
            scan("@include('a', foo(1, (2))) tail", 0),
            Some(("include", "", Some("'a', foo(1, (2))"), 26))
        );
    }

    #[test]
    fn scan_skips_strings_and_comments() {
        assert_eq!(
            scan("@if ($x === ')') y", 0),
            Some(("if", " ", Some("$x === ')'"), 16))
        );
        assert_eq!(
            scan("@if ($a /* ) */) y", 0),
            Some(("if", " ", Some("$a /* ) */"), 16))
        );
        assert_eq!(
            scan("@if ($a // )\n) y", 0),
            Some(("if", " ", Some("$a // )\n"), 14))
        );
        assert_eq!(
            scan("@if ($a # )\n) y", 0),
            Some(("if", " ", Some("$a # )\n"), 13))
        );
        // `#[` is an attribute opener, not a comment.
        assert_eq!(
            scan("@if ($a #[) y", 0),
            Some(("if", " ", Some("$a #["), 11))
        );
        assert_eq!(
            scan(r#"@if ($x == 'it\'s)') y"#, 0),
            Some(("if", " ", Some(r#"$x == 'it\'s)'"#), 20))
        );
        assert_eq!(
            scan(r#"@if ($x == "a\")") y"#, 0),
            Some(("if", " ", Some(r#"$x == "a\")""#), 18))
        );
    }

    #[test]
    fn scan_unbalanced_parens_yield_bare_name() {
        assert_eq!(scan("@if (unclosed", 0), Some(("if", "", None, 3)));
        assert_eq!(scan("@if ((x)", 0), Some(("if", "", None, 3)));
        assert_eq!(scan("@if ($a /* ) ", 0), Some(("if", "", None, 3)));
        assert_eq!(scan("@if ('abc)", 0), Some(("if", "", None, 3)));
    }

    #[test]
    fn split_args() {
        assert_eq!(split_top_level_args(""), Vec::<&str>::new());
        assert_eq!(split_top_level_args("  "), Vec::<&str>::new());
        assert_eq!(split_top_level_args(" 'a' "), ["'a'"]);
        assert_eq!(split_top_level_args("'a', 'b'"), ["'a'", "'b'"]);
        assert_eq!(
            split_top_level_args("'a,b', \"c,\\\",d\""),
            ["'a,b'", "\"c,\\\",d\""]
        );
        assert_eq!(
            split_top_level_args("f(1, 2), [3, 4], {5, 6}, 7"),
            ["f(1, 2)", "[3, 4]", "{5, 6}", "7"]
        );
        assert_eq!(
            split_top_level_args("a, /* , */ b // ,\n, c"),
            ["a", "/* , */ b // ,", "c"]
        );
        assert_eq!(split_top_level_args("a,"), ["a"]);
        assert_eq!(split_top_level_args("a, , b"), ["a", "", "b"]);
        assert_eq!(split_top_level_args(","), [""]);
    }
}
