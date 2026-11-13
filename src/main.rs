//! TraceDNA freezes a failing run into an editable capsule, then asks the failure
//! which parts of the code it actually needs.
//!
//! `capture` searches for a seed that breaks the subject and writes the conditions down.
//! A capsule may name a `check`: a second shell line that reads the subject's output and
//! decides whether it was wrong, so a failure no longer has to be a crash.
//! `replay` puts those conditions back. `analyze` removes one component at a time and
//! replays under the same conditions: a component whose removal makes the failure
//! disappear is a component the failure depends on.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, fs};

const ATTEMPTS: u32 = 200;
const OUTPUT: &str = "failure.capsule";
const MODIFIERS: [&str; 5] = ["public", "private", "protected", "static", "final"];
/// Words that open a block which is not a method, and so must never be knocked out.
const KEYWORDS: [&str; 11] = [
    "if", "else", "for", "while", "do", "switch", "catch", "try", "synchronized", "new", "return",
];

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let outcome = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["capture", capsule] => capture(capsule, OUTPUT),
        ["capture", capsule, output] => capture(capsule, output),
        ["replay", capsule] => replay(capsule),
        ["analyze", capsule] => analyze(capsule),
        _ => Err("usage:\n  tracedna capture <capsule> [output]\n  tracedna replay <capsule>\n  tracedna analyze <capsule>".to_string()),
    };
    match outcome {
        Ok(code) => code,
        Err(message) => {
            eprintln!("tracedna: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Everything needed to reproduce one failure, and what that failure turned out to need.
#[derive(Clone, Default, Debug, PartialEq)]
struct Capsule {
    build: String,
    command: String,
    check: String,
    source: String,
    seed: u64,
    input: String,
    env: BTreeMap<String, String>,
    exit: i32,
    error: String,
    needs: BTreeMap<String, String>,
}

/// What one execution produced.
struct Run {
    exit: i32,
    stdout: String,
    stderr: String,
}

impl Capsule {
    /// Capsules are edited by hand, so a typo is reported instead of silently ignored.
    fn parse(text: &str) -> Result<Capsule, String> {
        let mut capsule = Capsule::default();
        for (number, line) in text.lines().map(str::trim).enumerate() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| format!("line {}: `{line}` is not a field", number + 1))?;
            let (key, value) = (key.trim(), value.trim());
            let line_number = number + 1;
            let bad = |what: &str| format!("line {line_number}: `{key}` must be {what}, got `{value}`");
            match key {
                "build" => capsule.build = value.to_string(),
                "command" => capsule.command = value.to_string(),
                "check" => capsule.check = value.to_string(),
                "source" => capsule.source = value.to_string(),
                "input" => capsule.input = value.to_string(),
                "error" => capsule.error = value.to_string(),
                "seed" => capsule.seed = value.parse().map_err(|_| bad("a number"))?,
                "exit" => capsule.exit = value.parse().map_err(|_| bad("an exit code"))?,
                other => {
                    if let Some(name) = other.strip_prefix("env.") {
                        capsule.env.insert(name.to_string(), value.to_string());
                    } else if let Some(name) = other.strip_prefix("needs.") {
                        capsule.needs.insert(name.to_string(), value.to_string());
                    } else {
                        return Err(format!("line {line_number}: unknown field `{key}`"));
                    }
                }
            }
        }
        Ok(capsule)
    }

    /// One field per line, sorted, so a capsule diffs cleanly in Git.
    fn render(&self) -> String {
        let mut text = format!("# what runs\nbuild={}\ncommand={}\n", self.build, self.command);
        if !self.check.is_empty() {
            text += &format!("check={}\n", self.check);
        }
        text += &format!("source={}\n", self.source);
        text += &format!(
            "\n# under which conditions\nseed={}\ninput={}\n",
            self.seed, self.input
        );
        for (name, value) in &self.env {
            text += &format!("env.{name}={value}\n");
        }
        text += &format!("\n# what happened\nexit={}\nerror={}\n", self.exit, self.error);
        if !self.needs.is_empty() {
            text += "\n# what the failure needs (regenerate with `tracedna analyze`)\n";
            for (name, value) in &self.needs {
                text += &format!("needs.{name}={value}\n");
            }
        }
        text
    }

    /// Runs one shell line under exactly the conditions this capsule describes.
    /// `feed` hands a previous run to the line: its stdout on stdin, its exit code as `EXIT`.
    fn shell(&self, line: &str, feed: Option<&Run>) -> Result<Run, String> {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(line)
            .env("SEED", self.seed.to_string())
            .env("INPUT", &self.input)
            .envs(&self.env)
            .stdin(if feed.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(run) = feed {
            command.env("EXIT", run.exit.to_string());
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("could not start `sh`: {error}"))?;
        // Written from another thread: a check that prints before it finishes reading
        // would otherwise fill its stdout pipe while we are still blocked on its stdin.
        let writer = match (feed, child.stdin.take()) {
            (Some(run), Some(mut stdin)) => {
                let text = run.stdout.clone();
                Some(thread::spawn(move || stdin.write_all(text.as_bytes()).ok()))
            }
            _ => None,
        };
        let output = child
            .wait_with_output()
            .map_err(|error| format!("`sh` did not finish: {error}"))?;
        if let Some(writer) = writer {
            writer.join().ok(); // a check that stops reading early closes the pipe, which is fine
        }
        Ok(Run {
            exit: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    fn build(&self) -> Result<(), String> {
        if self.build.is_empty() {
            return Ok(());
        }
        let run = self.shell(&self.build, None)?;
        match run.exit {
            0 => Ok(()),
            _ => Err(format!("build failed:\n{}{}", run.stdout, run.stderr)),
        }
    }

    /// Runs the subject, then the check on what it printed. With a check, the check's
    /// exit code and stderr are the outcome: the subject can exit `0` and still be wrong.
    fn observe(&self) -> Result<Run, String> {
        let run = self.shell(&self.command, None)?;
        if self.check.is_empty() {
            return Ok(run);
        }
        let checked = self.shell(&self.check, Some(&run))?;
        Ok(judged(run, checked))
    }

    /// A capsule that records a clean exit and no error line describes no failure, so
    /// there is nothing for `replay` to bring back or for `analyze` to take apart.
    fn records_a_failure(&self) -> bool {
        self.exit != 0 || !self.error.is_empty()
    }

    /// The failure is the recorded exit code plus the recorded error line, matched whole:
    /// a short `error` value must not pass just by appearing inside a different message.
    fn reproduces(&self, run: &Run) -> bool {
        run.exit == self.exit
            && (self.error.is_empty() || run.stderr.lines().any(|line| line.trim() == self.error))
    }
}

/// The subject's output stays visible, but only the check decides what happened.
fn judged(run: Run, checked: Run) -> Run {
    Run {
        exit: checked.exit,
        stdout: format!("{}{}{}", run.stdout, run.stderr, checked.stdout),
        stderr: checked.stderr,
    }
}

/// Runs the subject with a fresh seed until it fails, then writes that run down.
fn capture(file: &str, output: &str) -> Result<ExitCode, String> {
    let mut capsule = read(file)?;
    capsule.build()?;
    for attempt in 0..ATTEMPTS {
        capsule.seed = seed(attempt);
        let run = capsule.observe()?;
        if run.exit != 0 {
            capsule.exit = run.exit;
            capsule.error = error_line(&run.stderr);
            capsule.needs.clear();
            write(output, &capsule.render())?;
            print!("{}{}", run.stdout, run.stderr);
            println!("\ncaptured after {} runs -> {output}", attempt + 1);
            return Ok(ExitCode::SUCCESS);
        }
    }
    Err(format!("no failure in {ATTEMPTS} runs"))
}

/// The line of stderr worth recording.
///
/// The first line is not it: toolchains print warnings before the failure, and a JVM prints
/// its `Picked up JAVA_TOOL_OPTIONS` banner before anything else. Recording that line makes
/// `replay` look for a warning that comes back on every run, so a failure that is still there
/// reads as `NOT REPRODUCED`. Take the first line that names the error, and fall back to the
/// last line that is not a stack frame.
fn error_line(stderr: &str) -> String {
    const SIGNALS: [&str; 6] = ["Exception", "Error", "error:", "error ", "cannot", "failed"];
    let lines: Vec<&str> = stderr.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    if let Some(line) = lines
        .iter()
        .find(|line| SIGNALS.iter().any(|signal| line.contains(signal)))
    {
        return line.to_string();
    }
    if let Some(line) = lines.iter().rev().find(|line| !line.starts_with("at ")) {
        return line.to_string();
    }
    lines.first().map(|line| line.to_string()).unwrap_or_default()
}

/// Puts the recorded conditions back and reports whether the failure returned.
fn replay(file: &str) -> Result<ExitCode, String> {
    let capsule = read(file)?;
    nothing_to_reproduce(&capsule, file)?;
    capsule.build()?;
    let run = capsule.observe()?;
    print!("{}{}", run.stdout, run.stderr);
    if capsule.reproduces(&run) {
        println!("REPRODUCED (exit {})", run.exit);
        Ok(ExitCode::SUCCESS)
    } else {
        println!(
            "NOT REPRODUCED (exit {}, the capsule records exit {})",
            run.exit, capsule.exit
        );
        Ok(ExitCode::FAILURE)
    }
}

/// Removes one component at a time and replays the frozen conditions against what is left.
/// The knockouts happen on a copy: the file named by `source` is read and never written,
/// and the capsule on disk keeps naming that file.
fn analyze(file: &str) -> Result<ExitCode, String> {
    let mut capsule = read(file)?;
    nothing_to_reproduce(&capsule, file)?;
    if capsule.source.is_empty() {
        return Err("this capsule has no `source=` field, so there is nothing to take apart".to_string());
    }
    let original = fs::read_to_string(&capsule.source)
        .map_err(|error| format!("cannot read {}: {error}", capsule.source))?;
    let (working, _copy) = staged(&capsule, &original)?;

    capsule.needs.clear();
    for method in methods(&original) {
        write(&working.source, &knockout(&original, &method))?;
        let verdict = verdict(&working)?;
        println!("{:<12} {verdict}", method.name);
        capsule.needs.insert(method.name, verdict);
    }
    write(file, &capsule.render())?;
    println!("\nwritten to {file}");
    Ok(ExitCode::SUCCESS)
}

/// A throwaway directory holding the copy `analyze` mutates. Dropping it removes the copy,
/// including when a knockout is cut short by an error or a panic.
struct Staged {
    dir: PathBuf,
}

impl Drop for Staged {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.dir).ok();
    }
}

/// A copy of the capsule pointed at a copy of the source kept in a temporary directory. The
/// copy is named in `build`, `command` and `check`, so nothing downstream can reach the file
/// the user is working on. Keeping the file name is what lets a public Java class still
/// compile: `javac` matches the class name to the file name, so `Invoice.java` has to stay
/// `Invoice.java`. Paths are written with forward slashes because the shell lines run under
/// `sh`, which reads a backslash as an escape.
fn staged(capsule: &Capsule, original: &str) -> Result<(Capsule, Staged), String> {
    let name = Path::new(&capsule.source)
        .file_name()
        .ok_or_else(|| format!("`{}` is not a file path", capsule.source))?
        .to_string_lossy()
        .into_owned();
    let dir = env::temp_dir().join(format!("tracedna-{}-{}", std::process::id(), seed(0)));
    fs::create_dir_all(&dir).map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    let copy = Staged { dir: dir.clone() };
    let path = dir.join(name);
    fs::write(&path, original)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    let staged = path.to_string_lossy().replace('\\', "/");
    let mut working = capsule.clone();
    let replaced = capsule.source.clone();
    for line in [&mut working.build, &mut working.command, &mut working.check] {
        *line = line.replace(&replaced, &staged);
    }
    working.source = staged;
    Ok((working, copy))
}

/// A subject capsule is written by hand before anything has failed, so it is easy to hand
/// one to `replay` and read a clean run back as the failure returning.
fn nothing_to_reproduce(capsule: &Capsule, file: &str) -> Result<(), String> {
    if capsule.records_a_failure() {
        return Ok(());
    }
    Err(format!(
        "{file} records exit 0 and no error, so it holds no failure yet: run `tracedna capture {file}` first"
    ))
}

/// Does the failure survive without this component?
fn verdict(capsule: &Capsule) -> Result<String, String> {
    if capsule.build().is_err() {
        return Ok("inconclusive".to_string());
    }
    let run = capsule.observe()?;
    Ok(if capsule.reproduces(&run) {
        "no"
    } else if run.exit == 0 {
        "yes"
    } else {
        "inconclusive"
    }
    .to_string())
}

/// One removable component: a Java method, and where its body sits in the source.
#[derive(Debug)]
struct Method {
    name: String,
    body: std::ops::Range<usize>,
    stub: &'static str,
}

fn methods(code: &str) -> Vec<Method> {
    let mut found = Vec::new();
    let mut offset = 0;
    for line in code.split_inclusive('\n') {
        if let Some(method) = signature(line, offset, code) {
            found.push(method);
        }
        offset += line.len();
    }
    found
}

/// A declaration looks like `<annotation..> <modifier..> <type> <name>(..) {` and is not
/// `main`. The modifiers are optional: a package-private method is still a method.
fn signature(line: &str, offset: usize, code: &str) -> Option<Method> {
    let trimmed = line.trim_end();
    if !trimmed.ends_with('{') {
        return None;
    }
    let declaration = without_annotations(trimmed);
    let mut words: Vec<&str> = declaration[..declaration.find('(')?].split_whitespace().collect();
    let name = words.pop()?;
    let returns = words.pop()?; // a constructor has no return type, and nothing to stub
    if name == "main" || KEYWORDS.contains(&name) || KEYWORDS.contains(&returns) {
        return None;
    }
    // Whatever is left has to be modifiers or a generic parameter. Without this,
    // `public Demo(int size) {` reads as a method named `Demo` returning `public`.
    if MODIFIERS.contains(&returns)
        || !words
            .iter()
            .all(|word| MODIFIERS.contains(word) || word.starts_with('<'))
    {
        return None;
    }
    let brace = offset + trimmed.rfind('{')?;
    Some(Method {
        name: name.to_string(),
        body: brace..body_end(code, brace)?,
        stub: stub(returns),
    })
}

/// Annotations sit before the declaration and may carry parentheses of their own, which
/// would otherwise be mistaken for the parameter list and hide the method entirely.
fn without_annotations(line: &str) -> &str {
    let mut rest = line.trim_start();
    while let Some(after_at) = rest.strip_prefix('@') {
        let name_end = after_at
            .find(|character: char| !character.is_alphanumeric() && character != '_' && character != '.')
            .unwrap_or(after_at.len());
        let after_name = after_at[name_end..].trim_start();
        rest = match after_name.strip_prefix('(') {
            Some(arguments) => match after_arguments(arguments) {
                Some(tail) => tail.trim_start(),
                None => return "",
            },
            None => after_name,
        };
    }
    rest
}

/// What follows the `)` that closes an annotation's argument list.
fn after_arguments(text: &str) -> Option<&str> {
    let mut depth = 1;
    for (index, character) in text.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[index + 1..]);
                }
            }
            _ => {}
        }
    }
    None
}

fn body_end(code: &str, brace: usize) -> Option<usize> {
    let mut depth = 0;
    for (index, character) in code[brace..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(brace + index + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// A body that does nothing but still compiles.
fn stub(returns: &str) -> &'static str {
    match returns {
        "void" => "",
        "boolean" => " return false; ",
        "byte" | "short" | "int" | "long" | "float" | "double" => " return 0; ",
        "char" => " return (char) 0; ",
        _ => " return null; ",
    }
}

fn knockout(code: &str, method: &Method) -> String {
    format!(
        "{}{{{}}}{}",
        &code[..method.body.start],
        method.stub,
        &code[method.body.end..]
    )
}

fn read(file: &str) -> Result<Capsule, String> {
    let text = fs::read_to_string(file).map_err(|error| format!("cannot read {file}: {error}"))?;
    Capsule::parse(&text).map_err(|problem| format!("{file}: {problem}"))
}

fn write(file: &str, text: &str) -> Result<(), String> {
    fs::write(file, text).map_err(|error| format!("cannot write {file}: {error}"))
}

/// A short, readable, unpredictable seed.
fn seed(attempt: u32) -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos() as u64)
        .unwrap_or_default();
    (now ^ u64::from(attempt).wrapping_mul(0x9E37_79B9_7F4A_7C15)) % 100_000
}

#[cfg(test)]
mod tests {
    use super::*;

    const JAVA: &str = "public class Demo {\n\
        \x20   static int fee(int amount) {\n\
        \x20       if (amount > 0) {\n\
        \x20           return amount / 2;\n\
        \x20       }\n\
        \x20       return 0;\n\
        \x20   }\n\
        \n\
        \x20   public static void main(String[] args) {\n\
        \x20       System.out.println(fee(10));\n\
        \x20   }\n\
        }\n";

    fn sample() -> Capsule {
        Capsule {
            build: "javac -d build X.java".to_string(),
            command: "java -cp build X".to_string(),
            check: String::new(),
            source: "X.java".to_string(),
            seed: 48291,
            input: "payment:100".to_string(),
            env: BTreeMap::from([("CURRENCY".to_string(), "BRL".to_string())]),
            exit: 1,
            error: "no BRL rate".to_string(),
            needs: BTreeMap::from([("rate".to_string(), "yes".to_string())]),
        }
    }

    #[test]
    fn capsule_survives_a_round_trip() {
        assert_eq!(Capsule::parse(&sample().render()).unwrap(), sample());
    }

    #[test]
    fn a_check_survives_a_round_trip_and_stays_out_of_capsules_without_one() {
        let checked = Capsule { check: "sh check.sh".to_string(), ..sample() };
        assert_eq!(Capsule::parse(&checked.render()).unwrap(), checked);
        assert!(!sample().render().contains("check="));
    }

    #[test]
    fn with_a_check_a_clean_exit_can_still_be_the_failure() {
        let capsule = Capsule { exit: 1, error: "negative invoice total".to_string(), ..sample() };
        let subject = Run { exit: 0, stdout: "total=-20\n".to_string(), stderr: String::new() };
        let check = Run { exit: 1, stdout: String::new(), stderr: "negative invoice total\n".to_string() };
        let run = judged(subject, check);
        assert!(capsule.reproduces(&run));
        assert!(run.stdout.contains("total=-20"));
    }

    #[test]
    fn a_capsule_that_records_no_failure_is_refused() {
        let subject = Capsule { exit: 0, error: String::new(), ..sample() };
        assert!(!subject.records_a_failure());
        assert!(nothing_to_reproduce(&subject, "subject.capsule").is_err());
        // Either half on its own is still a failure worth replaying.
        assert!(sample().records_a_failure());
        assert!(Capsule { exit: 0, ..sample() }.records_a_failure());
        assert!(Capsule { error: String::new(), ..sample() }.records_a_failure());
    }

    #[test]
    fn comments_are_not_fields() {
        let capsule = Capsule::parse("# needs.rate=yes\nseed=7\n").expect("a valid capsule");
        assert!(capsule.needs.is_empty());
        assert_eq!(capsule.seed, 7);
    }

    #[test]
    fn a_typo_is_reported_instead_of_ignored() {
        assert!(Capsule::parse("seed=abc\n").is_err());
        assert!(Capsule::parse("sed=7\n").is_err());
        assert!(Capsule::parse("seed 7\n").is_err());
    }

    #[test]
    fn methods_skips_main_and_control_flow() {
        let names: Vec<String> = methods(JAVA).into_iter().map(|method| method.name).collect();
        assert_eq!(names, ["fee"]);
    }

    #[test]
    fn knockout_empties_the_body_and_keeps_everything_else() {
        let mutated = knockout(JAVA, &methods(JAVA)[0]);
        assert!(mutated.contains("static int fee(int amount) { return 0; }"));
        assert!(!mutated.contains("amount / 2"));
        assert!(mutated.contains("public static void main"));
    }

    #[test]
    fn reproducing_needs_both_the_exit_code_and_the_error_line() {
        let capsule = sample();
        let same = Run { exit: 1, stdout: String::new(), stderr: "warming up\nno BRL rate\n".to_string() };
        let other_error = Run { exit: 1, stdout: String::new(), stderr: "disk full\n".to_string() };
        let clean = Run { exit: 0, stdout: String::new(), stderr: String::new() };
        assert!(capsule.reproduces(&same));
        assert!(!capsule.reproduces(&other_error));
        assert!(!capsule.reproduces(&clean));
    }

    #[test]
    fn a_short_error_does_not_match_a_longer_message() {
        let capsule = Capsule { error: "rate".to_string(), ..sample() };
        let longer = Run { exit: 1, stdout: String::new(), stderr: "no BRL rate for 98\n".to_string() };
        assert!(!capsule.reproduces(&longer));
    }

    #[test]
    fn an_annotation_with_arguments_does_not_hide_the_method() {
        let java = "public class Demo {\n    @SuppressWarnings(\"unchecked\") public int old() {\n        return 1;\n    }\n}\n";
        let names: Vec<String> = methods(java).into_iter().map(|method| method.name).collect();
        assert_eq!(names, ["old"]);
    }

    #[test]
    fn a_method_without_a_modifier_is_still_a_method() {
        let java = "public class Demo {\n    int fee(int amount) {\n        return amount / 2;\n    }\n}\n";
        let names: Vec<String> = methods(java).into_iter().map(|method| method.name).collect();
        assert_eq!(names, ["fee"]);
    }

    #[test]
    fn constructors_and_blocks_are_not_methods() {
        let java = "public class Demo {\n    public Demo(int size) {\n        if (size > 0) {\n        } else if (size < 0) {\n        }\n        new Thread(() -> {\n        });\n    }\n}\n";
        assert!(methods(java).is_empty());
    }

    #[test]
    fn annotated_methods_are_found() {
        let java = "public class Demo {\n    @Override\n    public String toString() {\n        return \"x\";\n    }\n\n    @Deprecated public int old() {\n        return 1;\n    }\n}\n";
        let names: Vec<String> = methods(java).into_iter().map(|method| method.name).collect();
        assert_eq!(names, ["toString", "old"]);
    }

    #[test]
    fn analyze_works_on_a_copy_and_leaves_the_users_source_alone() {
        let dir = env::temp_dir().join(format!("tracedna-test-{}-{}", std::process::id(), seed(0)));
        fs::create_dir_all(&dir).expect("a temporary directory");
        let user_source = dir.join("Demo.java");
        fs::write(&user_source, JAVA).expect("the subject");
        let user_path = user_source.to_string_lossy().into_owned();

        let capsule = Capsule {
            source: user_path.clone(),
            build: format!("javac -d build {user_path}"),
            command: "java -cp build Demo".to_string(),
            ..sample()
        };
        let (working, staged) = staged(&capsule, JAVA).expect("a staged copy");

        // The working capsule is pointed elsewhere, and so is the line that compiles it.
        assert_eq!(capsule.source, user_path, "the capsule on disk keeps the real path");
        assert_ne!(working.source, user_path);
        assert!(working.build.contains(&working.source));
        assert!(!working.build.contains(&user_path));
        assert!(Path::new(&working.source).exists());

        // A knockout lands on the copy, and the file the user is working on stays intact.
        write(&working.source, &knockout(JAVA, &methods(JAVA)[0])).expect("a knockout");
        assert!(fs::read_to_string(&working.source).expect("the copy").contains("return 0;"));
        assert_eq!(fs::read_to_string(&user_path).expect("the subject"), JAVA);

        // Dropping the staged copy takes its temporary directory with it.
        let copy = working.source.clone();
        drop(staged);
        assert!(!Path::new(&copy).exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_staged_source_path_survives_a_shell_line() {
        // The shell lines run under `sh`, where a backslash is an escape: a Windows path has
        // to reach it with forward slashes, or `C:\Users` arrives as `C:Users`.
        let capsule = Capsule {
            source: "examples/Invoice.java".to_string(),
            build: "javac -d build examples/Invoice.java".to_string(),
            ..sample()
        };
        let (working, _staged) = staged(&capsule, "class Invoice {}\n").expect("a staged copy");
        assert!(!working.source.contains('\\'));
        assert!(working.build.contains(&working.source));
    }

    #[test]
    fn a_warning_before_the_failure_is_not_recorded_as_the_error() {
        let stderr = "Picked up JAVA_TOOL_OPTIONS: -Dfile.encoding=UTF-8\n\
            Exception in thread \"main\" java.lang.IllegalStateException: no BRL rate\n\
            \tat Payments.rate(Payments.java:41)\n";
        let recorded = error_line(stderr);
        assert!(recorded.starts_with("Exception in thread"));
        // The point of recording that line: the failure still reproduces against it.
        let capsule = Capsule { error: recorded, ..sample() };
        let run = Run { exit: 1, stdout: String::new(), stderr: stderr.to_string() };
        assert!(capsule.reproduces(&run));
    }

    #[test]
    fn a_plain_message_is_recorded_instead_of_a_stack_frame() {
        let stderr = "warming up\nnegative invoice total\n\tat Check.main(Check.java:9)\n";
        assert_eq!(error_line(stderr), "negative invoice total");
        assert_eq!(error_line(""), "");
        assert_eq!(error_line("\n\n  \n"), "");
        assert_eq!(error_line("just one line\n"), "just one line");
    }
}
