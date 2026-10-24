use anyhow::anyhow;
use markup_fmt::{
    FormatError, Language,
    config::{FormatOptions, LayoutOptions, LineBreak},
    format_text,
};
use std::borrow::Cow;

/// Everything the external formatter was called with.
#[derive(Debug)]
struct Call {
    code: String,
    print_width: usize,
    indent_level: u16,
    attr: bool,
    ext: String,
}

fn options() -> FormatOptions {
    FormatOptions::default()
}

/// Callback returns the received code borrowed: the "no external formatter" shape.
fn format_identity(src: &str) -> String {
    format_text(src, Language::Blade, &options(), |code, _| {
        Ok(Cow::Borrowed(code))
    })
    .unwrap()
}

/// Callback returns an owned copy of what it received.
fn format_identity_owned(src: &str) -> String {
    format_text(src, Language::Blade, &options(), |code, _| {
        Ok(Cow::Owned(code.to_owned()))
    })
    .unwrap()
}

/// `transform` is applied to every `php` snippet; other snippets are left untouched.
fn format_php_with(src: &str, transform: impl Fn(&str) -> String) -> String {
    format_text(src, Language::Blade, &options(), |code, hints| {
        if hints.ext == "php" {
            Ok(Cow::Owned(transform(code)))
        } else {
            Ok(Cow::Borrowed(code))
        }
    })
    .unwrap()
}

fn format_recording(src: &str, transform: impl Fn(&str) -> String) -> (String, Vec<Call>) {
    let mut calls = Vec::new();
    let output = format_text(src, Language::Blade, &options(), |code, hints| {
        calls.push(Call {
            code: code.to_owned(),
            print_width: hints.print_width,
            indent_level: hints.indent_level,
            attr: hints.attr,
            ext: hints.ext.to_owned(),
        });
        if hints.ext == "php" {
            Ok(Cow::Owned(transform(code)))
        } else {
            Ok(Cow::Borrowed(code))
        }
    })
    .unwrap();
    (output, calls)
}

/// A violating result must leave the whole file exactly as if no formatter had run.
#[track_caller]
fn assert_rejected(src: &str, transform: impl Fn(&str) -> String, absent: &[&str]) {
    let expected = format_identity(src);
    let actual = format_php_with(src, transform);
    for needle in absent {
        assert!(
            !actual.contains(needle),
            "rejected result leaked {needle:?}:\n{actual}"
        );
    }
    similar_asserts::assert_eq!(expected, actual);
}

#[track_caller]
fn assert_syntax_error(src: &str) {
    match format_text(src, Language::Blade, &options(), |code, _| {
        Ok(Cow::Borrowed(code))
    }) {
        Err(FormatError::Syntax(_)) => {}
        other => panic!("expected a syntax error for {src:?}, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Syntax errors: the file is left untouched (no output at all).
// ---------------------------------------------------------------------------

#[test]
fn error_html_unbalanced_across_branches() {
    assert_syntax_error("<div>\n@if ($l) <a href=\"#\"> @endif x @if ($l) </a> @endif\n</div>\n");
}

#[test]
fn error_unclosed_if() {
    assert_syntax_error("@if ($x)\n<p>a</p>\n");
}

#[test]
fn error_unclosed_nested_block() {
    assert_syntax_error("@foreach ($a as $b)\n@if ($b)\n<p>a</p>\n@endforeach\n");
}

#[test]
fn error_stray_closer() {
    assert_syntax_error("<p>a</p>\n@endif\n");
}

#[test]
fn error_stray_branch() {
    assert_syntax_error("<p>a</p>\n@else\n<p>b</p>\n");
}

#[test]
fn error_mismatched_closer() {
    assert_syntax_error("@if ($a)\n<p>a</p>\n@endforeach\n");
}

#[test]
fn error_unclosed_blade_comment() {
    assert_syntax_error("<p>a</p>\n{{-- never closed\n<p>b</p>\n");
}

#[test]
fn error_raw_marker_inside_blade_comment() {
    assert_syntax_error("{{-- @php --}} x @endphp\n");
    assert_syntax_error("{{-- @verbatim --}} x @endverbatim\n");
}

#[test]
fn error_raw_marker_inside_php_tag() {
    assert_syntax_error("<?php $x = \"@verbatim\"; ?>\n<p>a</p>\n@endverbatim\n");
}

#[test]
fn error_html_comment_hiding_an_opener() {
    assert_syntax_error("<!-- @if ($x) -->\n<p>a</p>\n");
}

#[test]
fn error_unclosed_element_inside_block() {
    assert_syntax_error("@if ($a)\n<div>\n@endif\n");
}

// ---------------------------------------------------------------------------
// (a) Identity callbacks: Blade parts stay as written, except echo spacing.
// ---------------------------------------------------------------------------

const MIXED: &str = r#"<div>
<p>{{$name}}</p>
<p>{!!$html!!}</p>
<p>{{{$legacy}}}</p>
@if($a > 1)
<span>@lang('messages.hi')</span>
@elseif ($b)
@include('partials.x', ['a' => 1])
@endif
@foreach($users as $user)
<li>{{ $user }}</li>
@endforeach
<x-alert :message="$message" />
@json($data)
</div>
"#;

#[test]
fn identity_keeps_blade_parts_except_echo_spacing() {
    for output in [format_identity(MIXED), format_identity_owned(MIXED)] {
        for expected in [
            "{{ $name }}",
            "{!! $html !!}",
            "{{{ $legacy }}}",
            "@if($a > 1)",
            "@elseif ($b)",
            "@include('partials.x', ['a' => 1])",
            "@foreach($users as $user)",
            "{{ $user }}",
            ":message=\"$message\"",
            "@json($data)",
            "@lang('messages.hi')",
            "@endif",
            "@endforeach",
        ] {
            assert!(
                output.contains(expected),
                "missing {expected:?} in:\n{output}"
            );
        }
    }
}

#[test]
fn identity_borrowed_and_owned_agree() {
    similar_asserts::assert_eq!(format_identity(MIXED), format_identity_owned(MIXED));
}

#[test]
fn identity_keeps_php_bodies_byte_for_byte() {
    let tag_body = "\n// weird   spacing\n$a   =  1;\n   $b = [1,\n 2];\n";
    let directive_body = "\n    $c  =   [1,\n  2];\n        /** @var string $d */\n";
    let src = format!("<?php{tag_body}?>\n<p>x</p>\n@php{directive_body}@endphp\n<p>y</p>\n");
    for output in [format_identity(&src), format_identity_owned(&src)] {
        assert!(
            output.contains(&format!("<?php{tag_body}?>")),
            "<?php body changed:\n{output}"
        );
        assert!(
            output.contains(&format!("@php{directive_body}@endphp")),
            "@php body changed:\n{output}"
        );
    }
}

#[test]
fn identity_keeps_echo_tag_and_unclosed_php() {
    let src = "<p><?=   $x  ?></p>\n<?php\n$a  =  1;\necho \"{{ eof }}\";\n";
    for output in [format_identity(src), format_identity_owned(src)] {
        assert!(output.contains("<?=   $x  ?>"), "{output}");
        assert!(
            output.contains("<?php\n$a  =  1;\necho \"{{ eof }}\";\n"),
            "{output}"
        );
    }
}

// ---------------------------------------------------------------------------
// (b) A formatter that succeeds: `$a+$b` → `$a + $b`.
// ---------------------------------------------------------------------------

fn spaced(code: &str) -> String {
    code.replace("$a+$b", "$a + $b")
}

#[test]
fn successful_formatter_result_is_applied_with_blade_delimiters() {
    let src = "<div>\n<p>{{ $a+$b }}</p>\n<p>{!! $a+$b !!}</p>\n<p>{{{ $a+$b }}}</p>\n@if ($a+$b > 1)\n<span>x</span>\n@elseif ($a+$b)\n<i>y</i>\n@endif\n@foreach ($a+$b as $c)\n<b>z</b>\n@endforeach\n<x-counter :count=\"$a+$b\" />\n</div>\n";
    let (output, calls) = format_recording(src, spaced);
    for expected in [
        "{{ $a + $b }}",
        "{!! $a + $b !!}",
        "{{{ $a + $b }}}",
        "@if ($a + $b > 1)",
        "@elseif ($a + $b)",
        "@foreach ($a + $b as $c)",
        ":count=\"$a + $b\"",
        "@endif",
        "@endforeach",
    ] {
        assert!(
            output.contains(expected),
            "missing {expected:?} in:\n{output}"
        );
    }
    assert!(
        !output.contains("$a+$b"),
        "unformatted expression left:\n{output}"
    );

    let sent: Vec<&Call> = calls.iter().filter(|c| c.code.contains("$a+$b")).collect();
    assert_eq!(sent.len(), 7, "{calls:#?}");
    for call in &sent {
        assert_eq!(call.ext, "php");
        assert!(call.code.starts_with("<?php"), "{call:#?}");
    }
    let attr_calls = sent.iter().filter(|c| c.attr).count();
    assert_eq!(attr_calls, 1, "only the component :attr is an attribute");
    let attr_call = sent.iter().find(|c| c.attr).unwrap();
    assert!(attr_call.code.contains("($a+$b)"), "{attr_call:#?}");
}

#[test]
fn successful_formatter_gets_frames_per_directive_kind() {
    let src =
        "{{ $a+$b }}\n@if ($a+$b)\n@endif\n@foreach ($a+$b as $c)\n@endforeach\n@json($a+$b)\n";
    let (_, calls) = format_recording(src, spaced);
    let by = |needle: &str| {
        calls
            .iter()
            .find(|c| c.code.contains(needle))
            .unwrap_or_else(|| panic!("no snippet containing {needle:?}: {calls:#?}"))
    };
    assert!(by("($a+$b);").code.starts_with("<?php ("), "echo frame");
    assert!(by("if (").code.starts_with("<?php if ("), "if frame");
    assert!(
        by("foreach (").code.starts_with("<?php foreach ("),
        "foreach frame"
    );
    assert!(
        by("json(").code.starts_with("<?php json("),
        "generic directive frame"
    );
    assert!(calls.iter().all(|c| c.ext == "php" && !c.attr));
}

#[test]
fn hints_reflect_nesting_and_attribute_position() {
    let src = "{{ $r+$r }}\n<div>\n<section>\n<p>{{ $n+$n }}</p>\n<x-counter :count=\"$c+$c\" />\n</section>\n</div>\n";
    let (output, calls) = format_recording(src, |code| {
        code.replace("$r+$r", "$r + $r")
            .replace("$n+$n", "$n + $n")
            .replace("$c+$c", "$c + $c")
    });
    assert!(output.contains("{{ $r + $r }}"), "{output}");
    assert!(output.contains("{{ $n + $n }}"), "{output}");
    assert!(output.contains(":count=\"$c + $c\""), "{output}");

    let find = |needle: &str| calls.iter().find(|c| c.code.contains(needle)).unwrap();
    let root = find("$r+$r");
    let nested = find("$n+$n");
    let attr = find("$c+$c");

    assert_eq!(root.indent_level, 0, "{root:#?}");
    assert!(nested.indent_level > root.indent_level, "{nested:#?}");
    assert!(attr.indent_level > root.indent_level, "{attr:#?}");
    assert!(
        root.print_width <= options().layout.print_width,
        "{root:#?}"
    );
    assert!(nested.print_width < root.print_width, "{nested:#?}");
    assert!(!root.attr && !nested.attr && attr.attr, "{calls:#?}");
    assert!(calls.iter().all(|c| c.ext == "php"));
}

#[test]
fn successful_formatter_result_is_applied_to_raw_php() {
    let src = "<?php\n$x = $a+$b;\n?>\n<p>y</p>\n@php\n$z = $a+$b;\n@endphp\n";
    let output = format_php_with(src, spaced);
    assert!(output.contains("$x = $a + $b;"), "{output}");
    assert!(output.contains("$z = $a + $b;"), "{output}");
    assert!(output.contains("<?php"), "{output}");
    assert!(output.contains("?>"), "{output}");
    assert!(output.contains("@php"), "{output}");
    assert!(output.contains("@endphp"), "{output}");
}

#[test]
fn opaque_snippets_are_never_sent() {
    let src = "@use('App\\Models\\User')\n@use(function App\\Helpers\\format)\n<p>@verbatim {{ $x+$y }} @endverbatim</p>\n@datetime($x+$y)\n";
    let (_, calls) = format_recording(src, |code| code.replace('+', " + "));
    assert!(calls.is_empty(), "{calls:#?}");
}

// ---------------------------------------------------------------------------
// (c) A failing formatter: source kept, formatting still succeeds.
// ---------------------------------------------------------------------------

#[test]
fn failing_formatter_keeps_source() {
    let src = "{{ $a+$b }}\n<?php\n$x  =  1;\n?>\n@php\n$y  =  2;\n@endphp\n@if ($a+$b)\n<x-c :v=\"$a+$b\" />\n@endif\n";
    let output = format_text(src, Language::Blade, &options(), |_, _| {
        Err(anyhow!("boom"))
    })
    .expect("an external PHP failure must not fail formatting");
    similar_asserts::assert_eq!(format_identity(src), output);
    assert!(output.contains("@if ($a+$b)"), "{output}");
    assert!(output.contains(":v=\"$a+$b\""), "{output}");
    assert!(output.contains("$x  =  1;"), "{output}");
    assert!(output.contains("$y  =  2;"), "{output}");
}

#[test]
fn unframing_failure_keeps_source() {
    let src = "<p>{{ $a+$b }}</p>\n@if ($a+$b)\n<i>x</i>\n@endif\n";
    assert_rejected(src, |_| "this is not php at all".to_owned(), &["not php"]);
    assert_rejected(src, |_| String::new(), &[]);
}

// ---------------------------------------------------------------------------
// (d) Invalid output: every postcondition rejects and keeps the source span.
// ---------------------------------------------------------------------------

#[test]
fn rejects_foreach_as_reflowed_to_multiple_lines() {
    let src = "<ul>\n@foreach ($users as $user)\n<li>{{ $user->name }}</li>\n@endforeach\n</ul>\n";
    assert_rejected(
        src,
        |code| code.replace(" as ", "\n    as "),
        &["\n    as "],
    );
    let src =
        "<ul>\n@forelse ($users as $user)\n<li>x</li>\n@empty\n<li>y</li>\n@endforelse\n</ul>\n";
    assert_rejected(
        src,
        |code| code.replace(" as ", "\n    as "),
        &["\n    as "],
    );
}

#[test]
fn rejects_foreach_without_single_spaces_around_as() {
    let src = "@foreach ($users as $user)\n<i>x</i>\n@endforeach\n";
    assert_rejected(src, |code| code.replace(" as ", "  as  "), &["  as  "]);
}

#[test]
fn rejects_lang_char_after_paren_changed() {
    let src = "@lang(['name' => 'Alies'])\nHello :name\n@endlang\n";
    assert_rejected(
        src,
        |code| code.replace("(['name'", "( ['name'"),
        &["@lang( ["],
    );
}

#[test]
fn rejects_trailing_comma_in_comma_splitting_directives() {
    assert_rejected(
        "<p>@json($data)</p>\n",
        |code| code.replace("$data)", "$data,)"),
        &["$data,)"],
    );
    assert_rejected(
        "@pushIf($a, 'scripts')\n<i>x</i>\n@endPushIf\n",
        |code| code.replace("'scripts')", "'scripts',)"),
        &["'scripts',)"],
    );
}

#[test]
fn rejects_changed_comma_count_in_comma_splitting_directives() {
    assert_rejected(
        "<p>@json($a, JSON_PRETTY_PRINT)</p>\n",
        |code| code.replace("$a, JSON_PRETTY_PRINT", "$a"),
        &[],
    );
}

#[test]
fn rejects_output_containing_closing_delimiters() {
    for (src, injected) in [
        ("<p>{{ $a }}</p>\n", "'}}'"),
        ("<p>{{ $a }}</p>\n", "'--}}'"),
        ("<p>{!! $a !!}</p>\n", "'!!}'"),
        ("<p>{{ $a }}</p>\n", "'@endphp'"),
        ("<p>{{ $a }}</p>\n", "'?>'"),
        ("<p>{{{ $a }}}</p>\n", "'}}}'"),
    ] {
        assert_rejected(
            src,
            |code| code.replace("$a", &format!("$a . {injected}")),
            &[injected],
        );
    }
}

#[test]
fn rejects_new_directive_token() {
    assert_rejected(
        "<p>{{ $a }}</p>\n",
        |code| code.replace("$a", "$a . ' @csrf'"),
        &["@csrf"],
    );
    assert_rejected(
        "@if ($a)\n<i>x</i>\n@endif\n",
        |code| code.replace("$a", "$a || '@endif'"),
        &["'@endif'"],
    );
}

#[test]
fn rejects_line_comment_swallowing_code() {
    let src = "@if ($a // note\n    && $b)\n<i>x</i>\n@endif\n";
    assert_rejected(src, |code| code.replace("\n    ", " "), &["// note &&"]);
}

#[test]
fn rejects_altered_string_literals() {
    assert_rejected(
        "@include('partials.x')\n",
        |code| code.replace("'partials.x'", "'partials.y'"),
        &["partials.y"],
    );
    assert_rejected(
        "<p>{{ 'a  b' }}</p>\n",
        |code| code.replace("'a  b'", "'a b'"),
        &["'a b'"],
    );
    assert_rejected(
        "@php\n$s = <<<EOT\n  keep   spacing\nEOT;\n@endphp\n",
        |code| code.replace("  keep   spacing", "keep spacing"),
        &["keep spacing"],
    );
    assert_rejected(
        "<?php\n$s = <<<'EOT'\n  keep   spacing\nEOT;\n?>\n",
        |code| code.replace("  keep   spacing", "keep spacing"),
        &["keep spacing"],
    );
}

// ---------------------------------------------------------------------------
// (e) Raw PHP and literals keep their own line terminators, whatever the layout's.
// ---------------------------------------------------------------------------

fn options_with_line_break(line_break: LineBreak) -> FormatOptions {
    FormatOptions {
        layout: LayoutOptions {
            line_break,
            ..LayoutOptions::default()
        },
        ..FormatOptions::default()
    }
}

/// Sources whose literal content spans lines, with the bytes that must come out unchanged.
/// `nl` is the terminator inside the literal; the layout newlines around it are always `\n`.
fn literal_cases(nl: &str) -> Vec<(String, String)> {
    vec![
        (
            format!("<?php $s = \"a{nl}b\"; $z=1; ?>\n"),
            format!("\"a{nl}b\""),
        ),
        (
            format!("<?php\n$h = <<<EOT{nl}line1{nl}  line2{nl}EOT;\n$z=1;\n?>\n"),
            format!("<<<EOT{nl}line1{nl}  line2{nl}EOT;"),
        ),
        (
            format!("@php $s = 'a{nl}b'; $z=1; @endphp\n"),
            format!("'a{nl}b'"),
        ),
        (
            format!("@php\n$h = <<<'EOT'{nl}line1{nl}  line2{nl}EOT;\n@endphp\n"),
            format!("<<<'EOT'{nl}line1{nl}  line2{nl}EOT;"),
        ),
        (
            format!("<p>{{{{ \"a{nl}b\".$z }}}}</p>\n"),
            format!("\"a{nl}b\""),
        ),
        (
            format!("@if ($s == \"a{nl}b\")\n<p>x</p>\n@endif\n"),
            format!("\"a{nl}b\""),
        ),
        (
            format!("<p>@verbatim a{nl}b @endverbatim</p>\n"),
            format!(" a{nl}b "),
        ),
        (
            format!("<script>\nvar a = \"{{{{ $x }}}}\";{nl}var b = 1;\n</script>\n"),
            format!("\";{nl}var b = 1;"),
        ),
    ]
}

#[test]
fn literal_line_terminators_survive_every_callback_and_line_break() {
    for line_break in [LineBreak::Lf, LineBreak::Crlf] {
        for nl in ["\r\n", "\n"] {
            for (src, literal) in literal_cases(nl) {
                let options = options_with_line_break(line_break);
                let borrowed = format_text(&src, Language::Blade, &options, |code, _| {
                    Ok(Cow::Borrowed(code))
                })
                .unwrap();
                let errored =
                    format_text(&src, Language::Blade, &options, |_, _| Err(anyhow!("boom")))
                        .unwrap();
                // An accepted result that changes code outside the literals.
                let reformatted = format_text(&src, Language::Blade, &options, |code, hints| {
                    if hints.ext == "php" {
                        Ok(Cow::Owned(
                            code.replace("$z=1", "$z = 1").replace(".$z", " . $z"),
                        ))
                    } else {
                        Ok(Cow::Borrowed(code))
                    }
                })
                .unwrap();
                for (callback, output) in [
                    ("borrowed", &borrowed),
                    ("error", &errored),
                    ("reformatted", &reformatted),
                ] {
                    assert!(
                        output.contains(&literal),
                        "{callback} callback, {line_break:?} layout, literal newline {nl:?} \
                         lost {literal:?} in {output:?}"
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// (f) Whitespace after `?>` only changes when there was whitespace to change.
// ---------------------------------------------------------------------------

#[test]
fn trailing_text_after_php_close_gets_no_extra_space() {
    let output = format_identity("<p>@php $a = 1; @endphp more</p>\n");
    assert!(output.contains("@endphp more</p>"), "{output}");
    let output = format_identity("<span><?= 1 ?> text</span>tail\n");
    assert!(output.contains("text</span>tail"), "{output}");
    // Whitespace that ends the body is still kept as one space.
    let output = format_identity("<p>a@php $a = 1; @endphp </p>\n");
    assert!(output.contains("@endphp </p>"), "{output}");
}

// ---------------------------------------------------------------------------
// (g) Each attribute is sent to the external formatter once.
// ---------------------------------------------------------------------------

#[test]
fn lone_blade_attribute_is_formatted_once_when_its_layout_breaks() {
    let src = "<div {{ $attributes->merge(['class' => 'alpha beta gamma delta', 'id' => 'epsilon']) }}>x</div>\n";
    let options = FormatOptions {
        layout: LayoutOptions {
            print_width: 30,
            ..LayoutOptions::default()
        },
        ..FormatOptions::default()
    };
    let mut merge_calls = 0;
    let output = format_text(src, Language::Blade, &options, |code, hints| {
        if hints.ext != "php" {
            return Ok(Cow::Borrowed(code));
        }
        if code.contains("merge") {
            merge_calls += 1;
        }
        // A formatter result on several lines makes the attribute multi-line.
        Ok(Cow::Owned(code.replace(", 'id'", ",\n    'id'")))
    })
    .unwrap();
    assert!(output.contains("'id' => 'epsilon'"), "{output}");
    assert_eq!(merge_calls, 1, "{output}");
}

// ---------------------------------------------------------------------------
// (h) Review fixes: textual directives, PHP tags in attributes, component `=` spelling, speed.
// ---------------------------------------------------------------------------

#[test]
fn multi_line_result_is_rejected_for_textually_read_directives() {
    // `$expression[1] === '['` decides between `startTranslation` and `translator->get`.
    assert_rejected(
        "@lang(['name' => 'Ada'])<b>Hello :name</b>@endlang\n",
        |code| code.replace('[', "[\n").replace(']', "\n]"),
        &["@lang(\n"],
    );
    // `trim($segments[0], " '\"")` leaves a newline in the variable name.
    assert_rejected(
        "@inject('service', stdClass::class)\n",
        |code| {
            code.replace("inject(", "inject(\n")
                .replace(", ", ",\n")
                .replace(");", "\n);")
        },
        &["@inject(\n"],
    );
}

#[test]
fn php_tag_in_attribute_value_is_printed_verbatim() {
    let src = "<div class=\"<?= strlen('a  b') ?>\"></div>\n";
    similar_asserts::assert_eq!(src, format_identity(src));
    let src = "<div class='<?php echo strlen(\"a  b\"); ?>'></div>\n";
    similar_asserts::assert_eq!(src, format_identity(src));
}

#[test]
fn component_attribute_keeps_spacing_around_equals() {
    let src = "<x-alert :message = \"$message\" />\n";
    similar_asserts::assert_eq!(src, format_identity(src));
    let src = "<x-alert type =\"error\" :message= '$message' />\n";
    similar_asserts::assert_eq!(src, format_identity(src));
}

#[test]
fn script_with_many_unclosed_unknown_directives_is_linear() {
    let src = format!("<script>// {}\n</script>\n", "@unknown(".repeat(30_000));
    let started = std::time::Instant::now();
    let output = format_identity(&src);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "took {:?}",
        started.elapsed()
    );
    assert!(output.contains("@unknown("));
}

#[test]
fn large_flat_argument_list_is_linear() {
    let src = format!("@props([\n{}])\n", "'x',\n".repeat(30_000));
    let started = std::time::Instant::now();
    let output = format_identity(&src);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "took {:?}",
        started.elapsed()
    );
    let squeeze = |text: &str| text.split_whitespace().collect::<String>();
    assert_eq!(squeeze(&src), squeeze(&output));
}
