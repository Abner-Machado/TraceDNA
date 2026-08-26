//! TraceDNA freezes a failing run into an editable capsule, then asks the failure
//! which parts of the code it actually needs.
//!
//! `capture` searches for a seed that breaks the subject and writes the conditions down.
//! `replay` puts those conditions back. `analyze` removes one component at a time and
//! replays under the same conditions: a component whose removal makes the failure
//! disappear is a component the failure depends on.

use std::collections::BTreeMap;
use std::process::{Command, ExitCode};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, fs};

const ATTEMPTS: u32 = 200;
const OUTPUT: &str = "failure.capsule";
const MODIFIERS: [&str; 5] = ["public", "private", "protected", "static", "final"];

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let outcome = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["capture", capsule] => capture(capsule),
        ["replay", capsule] => replay(capsule),
        ["analyze", capsule] => analyze(capsule),
        _ => Err("usage:\n  tracedna capture <capsule>\n  tracedna replay <capsule>\n  tracedna analyze <capsule>".to_string()),
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
#[derive(Default, Debug, PartialEq)]
struct Capsule {
    build: String,
    command: String,
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
        let mut text = format!(
            "# what runs\nbuild={}\ncommand={}\nsource={}\n",
            self.build, self.command, self.source
        );
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
    fn shell(&self, line: &str) -> Result<Run, String> {
        let output = Command::new("sh")
            .arg("-c")
            .arg(line)
            .env("SEED", self.seed.to_string())
            .env("INPUT", &self.input)
            .envs(&self.env)
            .output()
            .map_err(|error| format!("could not start `sh`: {error}"))?;
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
        let run = self.shell(&self.build)?;
        match run.exit {
            0 => Ok(()),
            _ => Err(format!("build failed:\n{}{}", run.stdout, run.stderr)),
        }
    }

    /// The failure is the recorded exit code plus the recorded error line, matched whole:
    /// a short `error` value must not pass just by appearing inside a different message.
    fn reproduces(&self, run: &Run) -> bool {
        run.exit == self.exit
            && (self.error.is_empty() || run.stderr.lines().any(|line| line.trim() == self.error))
    }
}

/// Runs the subject with a fresh seed until it fails, then writes that run down.
fn capture(file: &str) -> Result<ExitCode, String> {
    let mut capsule = read(file)?;
    capsule.build()?;
    for attempt in 0..ATTEMPTS {
        capsule.seed = seed(attempt);
        let run = capsule.shell(&capsule.command)?;
        if run.exit != 0 {
            capsule.exit = run.exit;
            capsule.error = run.stderr.lines().next().unwrap_or_default().trim().to_string();
            capsule.needs.clear();
            write(OUTPUT, &capsule.render())?;
            print!("{}{}", run.stdout, run.stderr);
            println!("\ncaptured after {} runs -> {OUTPUT}", attempt + 1);
            return Ok(ExitCode::SUCCESS);
        }
    }
    Err(format!("no failure in {ATTEMPTS} runs"))
}

/// Puts the recorded conditions back and reports whether the failure returned.
fn replay(file: &str) -> Result<ExitCode, String> {
    let capsule = read(file)?;
    capsule.build()?;
    let run = capsule.shell(&capsule.command)?;
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
fn analyze(file: &str) -> Result<ExitCode, String> {
    let mut capsule = read(file)?;
    if capsule.source.is_empty() {
        return Err("this capsule has no `source=` field, so there is nothing to take apart".to_string());
    }
    let backup = format!("{}.tracedna-backup", capsule.source);
    if let Ok(interrupted) = fs::read_to_string(&backup) {
        write(&capsule.source, &interrupted)?;
    }
    let original = fs::read_to_string(&capsule.source)
        .map_err(|error| format!("cannot read {}: {error}", capsule.source))?;
    write(&backup, &original)?;

    capsule.needs.clear();
    for method in methods(&original) {
        write(&capsule.source, &knockout(&original, &method))?;
        let verdict = verdict(&capsule);
        write(&capsule.source, &original)?;
        let verdict = verdict?;
        println!("{:<12} {verdict}", method.name);
        capsule.needs.insert(method.name, verdict);
    }
    fs::remove_file(&backup).ok();
    write(file, &capsule.render())?;
    println!("\nwritten to {file}");
    Ok(ExitCode::SUCCESS)
}

/// Does the failure survive without this component?
fn verdict(capsule: &Capsule) -> Result<String, String> {
    if capsule.build().is_err() {
        return Ok("inconclusive".to_string());
    }
    let run = capsule.shell(&capsule.command)?;
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

/// A declaration looks like `<modifier..> <type> <name>(..) {` and is not `main`.
fn signature(line: &str, offset: usize, code: &str) -> Option<Method> {
    let trimmed = line.trim_end();
    if !trimmed.ends_with('{') {
        return None;
    }
    let mut words: Vec<&str> = trimmed[..trimmed.find('(')?].split_whitespace().collect();
    words.retain(|word| !word.starts_with('@')); // annotations sit before the modifiers
    let name = words.pop()?;
    let returns = words.pop()?;
    if name == "main" || !MODIFIERS.contains(words.first()?) {
        return None;
    }
    let brace = offset + trimmed.rfind('{')?;
    Some(Method {
        name: name.to_string(),
        body: brace..body_end(code, brace)?,
        stub: stub(returns),
    })
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
    fn annotated_methods_are_found() {
        let java = "public class Demo {\n    @Override\n    public String toString() {\n        return \"x\";\n    }\n\n    @Deprecated public int old() {\n        return 1;\n    }\n}\n";
        let names: Vec<String> = methods(java).into_iter().map(|method| method.name).collect();
        assert_eq!(names, ["toString", "old"]);
    }
}
