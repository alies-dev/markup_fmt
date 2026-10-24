//! Corpus runner for markup_fmt's Blade support.
//!
//! ```text
//! corpus <dir>... [--php-formatter none|mago] [--out <dir>] [--print-width 180] [--indent-width 4]
//!                 [--examples 5] [--strict]
//! ```
//!
//! Formats every `*.blade.php` below the given directories, formats the result a second time to check
//! idempotency, and prints counts plus a few `file:line` examples per failure category.

use markup_fmt::{
    FormatError, Hints, Language, SyntaxErrorKind, config::FormatOptions, format_text,
};
use std::{
    borrow::Cow,
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    fs,
    panic::{self, AssertUnwindSafe},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Instant,
};

const USAGE: &str = "usage: corpus <dir>... [--php-formatter none|mago] [--out <dir>] \
[--print-width 180] [--indent-width 4] [--examples 5] [--strict]";

#[derive(Clone, Copy, PartialEq, Eq)]
enum PhpMode {
    None,
    Mago,
}

struct Args {
    dirs: Vec<PathBuf>,
    php: PhpMode,
    out: Option<PathBuf>,
    print_width: usize,
    indent_width: usize,
    examples: usize,
    strict: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        dirs: vec![],
        php: PhpMode::None,
        out: None,
        print_width: 180,
        indent_width: 4,
        examples: 5,
        strict: false,
    };

    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => {
                (flag.to_owned(), Some(value.to_owned()))
            }
            _ => (arg.clone(), None),
        };
        let mut value = |name: &str| {
            inline
                .clone()
                .or_else(|| iter.next())
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match flag.as_str() {
            "--php-formatter" => {
                args.php = match value("--php-formatter")?.as_str() {
                    "none" => PhpMode::None,
                    "mago" => PhpMode::Mago,
                    other => return Err(format!("unknown --php-formatter `{other}` (none|mago)")),
                }
            }
            "--out" => args.out = Some(PathBuf::from(value("--out")?)),
            "--print-width" => {
                args.print_width = value("--print-width")?
                    .parse()
                    .map_err(|_| "--print-width must be a number".to_owned())?
            }
            "--indent-width" => {
                args.indent_width = value("--indent-width")?
                    .parse()
                    .map_err(|_| "--indent-width must be a number".to_owned())?
            }
            "--examples" => {
                args.examples = value("--examples")?
                    .parse()
                    .map_err(|_| "--examples must be a number".to_owned())?
            }
            "--strict" => args.strict = true,
            "-h" | "--help" => return Err(String::new()),
            other if other.starts_with("--") => return Err(format!("unknown option `{other}`")),
            _ => args.dirs.push(PathBuf::from(arg)),
        }
    }

    if args.dirs.is_empty() {
        return Err("no input directory".to_owned());
    }
    if cfg!(not(feature = "mago")) && args.php == PhpMode::Mago {
        return Err("built without the `mago` feature".to_owned());
    }
    Ok(args)
}

/// Callback target for `format_text`; PHP snippets (ext `php`) go to mago or stay as they are.
struct Php {
    mode: PhpMode,
    indent_width: usize,
    /// (snippet, print width) -> formatted; `None` when mago rejected the snippet.
    cache: HashMap<(String, usize), Option<String>>,
}

impl Php {
    fn new(mode: PhpMode, indent_width: usize) -> Self {
        Self {
            mode,
            indent_width,
            cache: HashMap::new(),
        }
    }

    fn format<'a>(
        &mut self,
        code: &'a str,
        hints: Hints<'_>,
    ) -> Result<Cow<'a, str>, anyhow::Error> {
        if self.mode == PhpMode::None || hints.ext != "php" {
            return Ok(Cow::Borrowed(code));
        }
        let key = (code.to_owned(), hints.print_width);
        let indent_width = self.indent_width;
        let formatted = self
            .cache
            .entry(key)
            .or_insert_with(|| mago_format(code, hints.print_width, indent_width));
        match formatted {
            Some(formatted) => Ok(Cow::Owned(formatted.clone())),
            None => Err(anyhow::anyhow!("mago rejected the PHP snippet")),
        }
    }
}

#[cfg(feature = "mago")]
fn mago_format(code: &str, print_width: usize, indent_width: usize) -> Option<String> {
    use mago_formatter::{Formatter, settings::FormatSettings};
    use mago_php_version::PHPVersion;

    let arena = mago_allocator::LocalArena::new();
    let settings = FormatSettings {
        print_width: print_width.max(1),
        tab_width: indent_width,
        use_tabs: false,
        ..FormatSettings::default()
    };
    let formatter = Formatter::new(&arena, PHPVersion::PHP84, settings);
    let formatted = formatter
        .format_code(
            Cow::Borrowed(&b"snippet.php"[..]),
            Cow::Owned(code.as_bytes().to_vec()),
        )
        .ok()?;
    String::from_utf8(formatted.to_vec()).ok()
}

#[cfg(not(feature = "mago"))]
fn mago_format(_: &str, _: usize, _: usize) -> Option<String> {
    None
}

enum Outcome {
    Formatted {
        changed: bool,
        non_idempotent: Option<(usize, String)>,
    },
    Failed {
        category: String,
        line: usize,
        column: usize,
        message: String,
    },
    Crash {
        message: String,
    },
}

thread_local! {
    static PANIC_INFO: RefCell<String> = const { RefCell::new(String::new()) };
}

fn install_panic_hook() {
    panic::set_hook(Box::new(|info| {
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic payload>".to_owned());
        let location = info
            .location()
            .map(|l| format!(" at {}:{}", l.file(), l.line()))
            .unwrap_or_default();
        PANIC_INFO.with(|cell| *cell.borrow_mut() = format!("{message}{location}"));
    }));
}

fn take_panic_info() -> String {
    PANIC_INFO.with(|cell| std::mem::take(&mut *cell.borrow_mut()))
}

fn run_pass(
    src: &str,
    opts: &FormatOptions,
    php: &mut Php,
) -> Result<Result<String, FormatError>, String> {
    panic::catch_unwind(AssertUnwindSafe(|| {
        format_text(src, Language::Blade, opts, |code, hints| {
            php.format(code, hints)
        })
    }))
    .map_err(|_| take_panic_info())
}

fn category(kind: &SyntaxErrorKind) -> String {
    if let SyntaxErrorKind::BladeUnsupported { reason, .. } = kind {
        // Reasons may embed positions ("… opened at line 9 …"); group by the trailing "(cause)".
        let cause = reason
            .rsplit_once('(')
            .and_then(|(_, cause)| cause.strip_suffix(')'))
            .unwrap_or(reason);
        return format!("BladeUnsupported: {cause}");
    }
    format!("{kind:?}")
        .split(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn clip(s: &str, max: usize) -> String {
    let mut clipped: String = s.chars().take(max).collect();
    if clipped.len() < s.len() {
        clipped.push('…');
    }
    clipped
}

fn first_difference(a: &str, b: &str) -> (usize, String) {
    for (i, (left, right)) in a.lines().zip(b.lines()).enumerate() {
        if left != right {
            return (
                i + 1,
                format!(
                    "pass1: {:?} | pass2: {:?}",
                    clip(left, 100),
                    clip(right, 100)
                ),
            );
        }
    }
    let (n, m) = (a.lines().count(), b.lines().count());
    if n == m {
        (
            n,
            "only trailing whitespace / line ending differs".to_owned(),
        )
    } else {
        (n.min(m) + 1, format!("line count differs: {n} vs {m}"))
    }
}

/// Returns the outcome and the text to write into `--out` (the source itself unless formatting succeeded).
fn process(src: &str, opts: &FormatOptions, php: &mut Php) -> (Outcome, String) {
    let formatted = match run_pass(src, opts, php) {
        Err(message) => return (Outcome::Crash { message }, src.to_owned()),
        Ok(Err(FormatError::Syntax(e))) => {
            return (
                Outcome::Failed {
                    category: category(&e.kind),
                    line: e.line,
                    column: e.column,
                    message: e.kind.to_string(),
                },
                src.to_owned(),
            );
        }
        Ok(Err(e @ FormatError::External(_))) => {
            return (
                Outcome::Failed {
                    category: "External".to_owned(),
                    line: 0,
                    column: 0,
                    message: clip(&e.to_string().replace('\n', " "), 200),
                },
                src.to_owned(),
            );
        }
        Ok(Ok(formatted)) => formatted,
    };

    let non_idempotent = match run_pass(&formatted, opts, php) {
        Err(message) => {
            return (
                Outcome::Crash {
                    message: format!("(second pass) {message}"),
                },
                formatted,
            );
        }
        Ok(Err(e)) => Some((
            0,
            format!("second pass failed: {}", clip(&e.to_string(), 160)),
        )),
        Ok(Ok(second)) if second != formatted => Some(first_difference(&formatted, &second)),
        Ok(Ok(_)) => None,
    };

    (
        Outcome::Formatted {
            changed: formatted != src,
            non_idempotent,
        },
        formatted,
    )
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let mut entries = fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let file_type = entry.file_type()?;
        let path = entry.path();
        if file_type.is_dir() {
            collect_files(&path, out)?;
        } else if file_type.is_file()
            && entry
                .file_name()
                .to_string_lossy()
                .to_ascii_lowercase()
                .ends_with(".blade.php")
        {
            out.push(path);
        }
    }
    Ok(())
}

struct Report {
    path: String,
    outcome: Outcome,
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            if !message.is_empty() {
                eprintln!("error: {message}");
            }
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };

    // (input dir index, file)
    let mut files: Vec<(usize, PathBuf)> = vec![];
    for (index, dir) in args.dirs.iter().enumerate() {
        let mut found = vec![];
        if let Err(e) = collect_files(dir, &mut found) {
            eprintln!("error: cannot read {}: {e}", dir.display());
            return ExitCode::from(2);
        }
        files.extend(found.into_iter().map(|file| (index, file)));
    }

    // With several input dirs, `--out` mirrors each under its own directory name.
    let out_roots: Vec<Option<PathBuf>> = args
        .dirs
        .iter()
        .map(|dir| {
            args.out.as_ref().map(|out| {
                if args.dirs.len() == 1 {
                    out.clone()
                } else {
                    out.join(dir.file_name().unwrap_or_default())
                }
            })
        })
        .collect();
    if args.dirs.len() > 1 {
        let mut names: Vec<_> = args.dirs.iter().map(|d| d.file_name()).collect();
        names.sort();
        if names.windows(2).any(|pair| pair[0] == pair[1]) {
            eprintln!("error: input directories must have distinct names when several are given");
            return ExitCode::from(2);
        }
    }

    let mut options = FormatOptions::default();
    options.layout.print_width = args.print_width;
    options.layout.indent_width = args.indent_width;

    install_panic_hook();
    let started = Instant::now();
    let next = AtomicUsize::new(0);
    let reports: Mutex<Vec<(usize, Report)>> = Mutex::new(Vec::with_capacity(files.len()));
    let workers = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(files.len().max(1));

    thread::scope(|scope| {
        for _ in 0..workers {
            let (files, args, options, out_roots, next, reports) =
                (&files, &args, &options, &out_roots, &next, &reports);
            thread::Builder::new()
                .stack_size(256 * 1024 * 1024)
                .spawn_scoped(scope, move || {
                    let mut php = Php::new(args.php, args.indent_width);
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some((dir_index, path)) = files.get(i) else {
                            break;
                        };
                        let display = path.display().to_string();
                        let (outcome, text) = match fs::read_to_string(path) {
                            Ok(src) => process(&src, options, &mut php),
                            Err(e) => (
                                Outcome::Failed {
                                    category: "Unreadable".to_owned(),
                                    line: 0,
                                    column: 0,
                                    message: e.to_string(),
                                },
                                String::new(),
                            ),
                        };
                        if let Some(root) = &out_roots[*dir_index]
                            && !matches!(&outcome, Outcome::Failed { category, .. } if category == "Unreadable")
                        {
                            let rel = path.strip_prefix(&args.dirs[*dir_index]).unwrap_or(path);
                            let target = root.join(rel);
                            let written = target
                                .parent()
                                .map_or(Ok(()), fs::create_dir_all)
                                .and_then(|()| fs::write(&target, text));
                            if let Err(e) = written {
                                eprintln!("warning: cannot write {}: {e}", target.display());
                            }
                        }
                        reports.lock().unwrap().push((
                            i,
                            Report {
                                path: display,
                                outcome,
                            },
                        ));
                    }
                })
                .expect("failed to spawn worker thread");
        }
    });

    let mut reports = reports.into_inner().unwrap();
    reports.sort_by_key(|(i, _)| *i);

    let (mut changed, mut unchanged) = (0usize, 0usize);
    let mut failed: BTreeMap<String, Vec<(&Report, usize, usize, &str)>> = BTreeMap::new();
    let mut crashes: Vec<(&Report, &str)> = vec![];
    let mut non_idempotent: Vec<(&Report, usize, &str)> = vec![];
    for (_, report) in &reports {
        match &report.outcome {
            Outcome::Formatted {
                changed: was_changed,
                non_idempotent: issue,
            } => {
                if *was_changed {
                    changed += 1;
                } else {
                    unchanged += 1;
                }
                if let Some((line, detail)) = issue {
                    non_idempotent.push((report, *line, detail.as_str()));
                }
            }
            Outcome::Failed {
                category,
                line,
                column,
                message,
            } => failed.entry(category.clone()).or_default().push((
                report,
                *line,
                *column,
                message.as_str(),
            )),
            Outcome::Crash { message } => crashes.push((report, message.as_str())),
        }
    }

    let failed_total: usize = failed.values().map(Vec::len).sum();
    println!(
        "{} files in {:.1}s (php formatter: {}, print width {}, indent width {})",
        reports.len(),
        started.elapsed().as_secs_f32(),
        if args.php == PhpMode::Mago {
            "mago"
        } else {
            "none"
        },
        args.print_width,
        args.indent_width,
    );
    println!("formatted (changed):  {changed}");
    println!("unchanged:            {unchanged}");
    println!("failed:               {failed_total}");
    for (category, entries) in &failed {
        println!("  {category}: {}", entries.len());
        for (report, line, column, message) in entries.iter().take(args.examples) {
            println!(
                "    {}:{line}:{column}  {}",
                report.path,
                clip(message, 200)
            );
        }
    }
    println!("crashes:              {}", crashes.len());
    for (report, message) in crashes.iter().take(args.examples) {
        println!("    {}  {}", report.path, clip(message, 200));
    }
    println!("non-idempotent:       {}", non_idempotent.len());
    for (report, line, detail) in non_idempotent.iter().take(args.examples) {
        println!("    {}:{line}  {detail}", report.path);
    }

    if !crashes.is_empty() || !non_idempotent.is_empty() || (args.strict && failed_total > 0) {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
