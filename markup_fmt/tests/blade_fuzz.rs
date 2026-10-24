//! Deterministic, dependency-free fuzzing of the Blade formatter.
//!
//! Inputs are glued together from a weighted alphabet of Blade-ish tokens. Every input must not
//! panic, and whenever formatting succeeds the output must format again to itself.
//! A failure prints the seed, case index and escaped input so it can become a fixture.

use markup_fmt::{Language, config::FormatOptions, format_text};
use std::{
    borrow::Cow,
    panic::{AssertUnwindSafe, catch_unwind},
};

type Formatter = fn(&str) -> Result<String, markup_fmt::FormatError>;

const SEEDS: [u64; 4] = [
    0x9E37_79B9_7F4A_7C15,
    0xD1B5_4A32_D192_ED03,
    0x0123_4567_89AB_CDEF,
    0xDEAD_BEEF_CAFE_F00D,
];
const CASES_PER_SEED: usize = 2_000;
const MAX_INPUT_LEN: usize = 400;
const MAX_TOKENS: u64 = 60;
const MAX_REPORTED_FAILURES: usize = 10;

/// `(weight, token)`
const ALPHABET: &[(u32, &str)] = &[
    (6, "{{"),
    (6, "}}"),
    (4, "{!!"),
    (4, "!!}"),
    (2, "{{{"),
    (2, "}}}"),
    (3, "{{--"),
    (3, "--}}"),
    (5, "@if("),
    (4, "@endif"),
    (3, "@else"),
    (3, "@foreach($a as $b)"),
    (3, "@endforeach"),
    (4, "@php"),
    (4, "@endphp"),
    (3, "@verbatim"),
    (3, "@endverbatim"),
    (3, "<?php "),
    (3, "?>"),
    (2, "<?="),
    (3, "@@"),
    (3, "@{{"),
    (6, "<div>"),
    (6, "</div>"),
    (3, "<x-card :a=\"$b\">"),
    (3, "</x-card>"),
    (2, "<x-slot:t>"),
    (2, "</x-slot>"),
    (2, "<pre>"),
    (2, "</pre>"),
    (2, "<script>"),
    (2, "</script>"),
    (4, "'"),
    (4, "\""),
    (5, "("),
    (5, ")"),
    (2, "//"),
    (2, "#"),
    (2, "\r\n"),
    (6, "\n"),
    (8, " "),
    (2, "  "),
    (4, "$x"),
    (6, "foo"),
    (4, "bar baz"),
    (2, "é"),
    (2, "€"),
    (2, "😀"),
];

/// splitmix64: tiny, well-distributed, and seedable with any value.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }
}

fn generate(rng: &mut Rng) -> String {
    let total: u32 = ALPHABET.iter().map(|(weight, _)| weight).sum();
    let tokens = 1 + rng.below(MAX_TOKENS);
    let mut src = String::new();
    for _ in 0..tokens {
        let mut pick = rng.below(total as u64) as u32;
        let token = ALPHABET
            .iter()
            .find(|(weight, _)| {
                if pick < *weight {
                    true
                } else {
                    pick -= weight;
                    false
                }
            })
            .map(|(_, token)| *token)
            .unwrap();
        if src.len() + token.len() > MAX_INPUT_LEN {
            break;
        }
        src.push_str(token);
    }
    src
}

/// Mimics a PHP formatter that rewrites the code. Runs of spaces are collapsed to a fixed point
/// so the stub itself is idempotent; otherwise idempotency failures would blame the formatter
/// for the stub's own behaviour.
fn fake_php(code: &str) -> String {
    let mut code = code.replace("  ", " ");
    while code.contains("  ") {
        code = code.replace("  ", " ");
    }
    code
}

fn format_identity(src: &str) -> Result<String, markup_fmt::FormatError> {
    format_text(src, Language::Blade, &FormatOptions::default(), |c, _| {
        Ok(Cow::Borrowed(c))
    })
}

fn format_with_fake_php(src: &str) -> Result<String, markup_fmt::FormatError> {
    format_text(
        src,
        Language::Blade,
        &FormatOptions::default(),
        |c, hints| {
            if hints.ext == "php" {
                Ok(Cow::Owned(fake_php(c)))
            } else {
                Ok(Cow::Borrowed(c))
            }
        },
    )
}

/// `Some(reason)` if the input misbehaves with the given formatting function.
fn check(src: &str, format: Formatter) -> Option<String> {
    let first = match catch_unwind(AssertUnwindSafe(|| format(src))) {
        Ok(Ok(out)) => out,
        // Syntax errors are legitimate for adversarial input.
        Ok(Err(_)) => return None,
        Err(_) => return Some("panic on first pass".into()),
    };
    match catch_unwind(AssertUnwindSafe(|| format(&first))) {
        Ok(Ok(second)) if second == first => None,
        Ok(Ok(second)) => Some(format!(
            "not idempotent\n  first : {first:?}\n  second: {second:?}"
        )),
        Ok(Err(err)) => Some(format!(
            "output does not reparse: {err}\n  output: {first:?}"
        )),
        Err(_) => Some(format!("panic on second pass\n  output: {first:?}")),
    }
}

#[test]
fn blade_fuzz_no_panic_and_idempotent() {
    let mut failures = Vec::new();
    let mut total_failures = 0usize;

    for seed in SEEDS {
        let mut rng = Rng(seed);
        for case in 0..CASES_PER_SEED {
            let src = generate(&mut rng);
            assert!(src.len() <= MAX_INPUT_LEN);
            for (name, format) in [
                ("identity", format_identity as Formatter),
                ("fake-php", format_with_fake_php as Formatter),
            ] {
                if let Some(reason) = check(&src, format) {
                    total_failures += 1;
                    if failures.len() < MAX_REPORTED_FAILURES {
                        failures.push(format!(
                            "seed {seed:#x}, case {case}, formatter {name}: {reason}\n  input : {src:?}"
                        ));
                    }
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{total_failures} failing (input, formatter) pairs; first {}:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}
