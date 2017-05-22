//! Spawns cargo sub-commands and parses its JSON output.

use std::borrow::Cow;
use std::cmp;
use std::io::{self, BufRead, BufReader, Write};
use std::mem;
use std::path::Path;
use std::process::{self, Child, ChildStdout, Stdio};
use std::str;

use serde_json;
use terminal_size::terminal_size;

use errors::*;
pub use self::diagnostic::Diagnostic;
pub use self::target::Kind;

#[derive(Debug, Deserialize)]
#[serde(tag = "reason")]
pub enum Output<'a> {
    #[serde(rename = "compiler-artifact")]
    Artifact(
        #[serde(borrow)]
        Artifact<'a>
    ),
    #[serde(rename = "compiler-message")]
    Message(
        #[serde(borrow)]
        Message<'a>
    ),
    #[serde(rename = "build-script-executed")]
    BuildStep(
        #[serde(borrow)]
        BuildStep<'a>
    ),
}

#[derive(Debug, Deserialize)]
pub struct Artifact<'a> {
    pub features: Vec<&'a str>,
    pub filenames: Vec<&'a Path>,
    pub fresh: bool,
    #[serde(borrow)]
    pub package_id: Cow<'a, str>,
    #[serde(borrow)]
    pub profile: Profile<'a>,
    #[serde(borrow)]
    pub target: Target<'a>,
}

#[derive(Debug, Deserialize)]
pub struct BuildStep<'a> {
    #[serde(borrow)]
    pub cfgs: Vec<Cow<'a, str>>,
    #[serde(borrow)]
    pub linked_libs: Vec<Cow<'a, str>>,
    #[serde(borrow)]
    pub linked_paths: Vec<&'a Path>,
    #[serde(borrow)]
    pub package_id: Cow<'a, str>,
}

#[derive(Debug, Deserialize)]
pub struct Profile<'a> {
    pub debug_assertions: bool,
    pub debuginfo: Option<u32>,
    pub opt_level: &'a str,
    pub overflow_checks: bool,
    pub test: bool,
}

#[derive(Debug, Deserialize)]
pub struct Target<'a> {
    pub crate_types: Vec<&'a str>,
    pub kind: Kind<'a>,
    #[serde(borrow)]
    pub name: Cow<'a, str>,
    #[serde(borrow)]
    pub src_path: &'a Path,
}

pub mod target {
    use std::fmt;
    use serde::de;

    #[derive(Debug)]
    pub enum Kind<'a> {
        Lib(Vec<&'a str>),
        Bin,
        Test,
        Bench,
        Example,
        CustomBuild,
    }

    impl<'a> Kind<'a> {
        pub fn is_bin(&self) -> bool {
            match *self {
                Kind::Bin => true,
                _ => false,
            }
        }
    }

    impl<'de: 'a, 'a> de::Deserialize<'de> for Kind<'a> {
        fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            deserializer.deserialize_seq(KindVisitor)
        }
    }

    /// Deserializes values like `["bin"]` or `["rlib", "dylib"]`.
    struct KindVisitor;

    impl<'de> de::Visitor<'de> for KindVisitor {
        type Value = Kind<'de>;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("an array containing target-kind strings")
        }

        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let first = seq.next_element::<&'de str>()?
                .ok_or_else(|| de::Error::invalid_length(0, &self))?;
            let kind = match first {
                "bin" => Kind::Bin,
                "test" => Kind::Test,
                "bench" => Kind::Bench,
                "example" => Kind::Example,
                "custom-build" => Kind::CustomBuild,
                first_lib => {
                    let mut libs = vec![first_lib];
                    while let Some(lib) = seq.next_element::<&'de str>()? {
                        libs.push(lib);
                    }
                    Kind::Lib(libs)
                }
            };
            Ok(kind)
        }
    }
}


#[derive(Debug, Deserialize)]
pub struct Message<'a> {
    #[serde(borrow)]
    pub message: Diagnostic<'a>,
    #[serde(borrow)]
    pub package_id: Cow<'a, str>,
    #[serde(borrow)]
    pub target: Target<'a>,
}

pub mod diagnostic {
    use std::borrow::Cow;
    use std::fmt;
    use std::path::Path;

    #[derive(Debug, Deserialize)]
    pub struct Diagnostic<'a> {
        #[serde(borrow)]
        pub message: Cow<'a, str>,
        #[serde(borrow)]
        pub code: Option<Code<'a>>,
        pub level: Level,
        #[serde(borrow)]
        pub spans: Vec<Span<'a>>,
        #[serde(borrow)]
        pub children: Vec<Diagnostic<'a>>,
        #[serde(borrow)]
        pub rendered: Option<Cow<'a, str>>,
    }

    #[derive(Debug, Deserialize)]
    pub struct Code<'a> {
        pub code: &'a str,
        #[serde(borrow)]
        pub explanation: Option<Cow<'a, str>>,
    }

    #[derive(Debug, Deserialize)]
    pub enum Level {
        #[serde(rename = "error: internal compiler error")]
        InternalCompilerError,
        #[serde(rename = "error")]
        Error,
        #[serde(rename = "warning")]
        Warning,
        #[serde(rename = "note")]
        Note,
        #[serde(rename = "help")]
        Help,
    }

    impl Level {
        pub fn is_show_stopper(&self) -> bool {
            use self::Level::*;
            match *self {
                InternalCompilerError | Error => true,
                Warning | Note | Help => false,
            }
        }
    }

    impl fmt::Display for Level {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            use self::Level::*;
            f.write_str(
                match *self {
                    InternalCompilerError => "error: internal compiler error",
                    Error => "error",
                    Warning => "warning",
                    Note => "note",
                    Help => "help",
                }
            )
        }
    }

    #[derive(Debug, Deserialize)]
    pub struct Span<'a> {
        #[serde(borrow)]
        pub file_name: &'a Path,
        pub byte_start: u32,
        pub byte_end: u32,
        pub line_start: u32,
        pub line_end: u32,
        pub column_start: u32,
        pub column_end: u32,
        pub is_primary: bool,
        #[serde(borrow)]
        pub text: Vec<SpanLine<'a>>,
        #[serde(borrow)]
        pub label: Option<Cow<'a, str>>,
        #[serde(borrow)]
        pub suggested_replacement: Option<Cow<'a, str>>,
        #[serde(borrow)]
        pub expansion: Option<Box<SpanMacroExpansion<'a>>>,
    }

    #[derive(Debug, Deserialize)]
    pub struct SpanLine<'a> {
        #[serde(borrow)]
        pub text: Cow<'a, str>,
        pub highlight_start: usize,
        pub highlight_end: usize,
    }

    #[derive(Debug, Deserialize)]
    pub struct SpanMacroExpansion<'a> {
        #[serde(borrow)]
        span: Span<'a>,
        #[serde(borrow)]
        macro_decl_name: Cow<'a, str>,
        #[serde(borrow)]
        def_site_span: Option<Span<'a>>,
    }
}

#[derive(Default)]
pub struct Command<'a> {
    path: Option<&'a Path>,
    features: Option<&'a [&'a str]>,
    bin_only: Option<&'a str>,
}

impl<'a> Command<'a> {
    pub fn new() -> Command<'a> {
        Default::default()
    }

    /// Set the location of Cargo.toml.
    pub fn manifest_path(&'a mut self, path: &'a Path) -> &'a mut Command<'a> {
        self.path = Some(path);
        self
    }

    pub fn features(&'a mut self, features: &'a [&'a str]) -> &'a mut Command<'a> {
        assert!(
            features.iter().all(|feat| !feat.contains(" ")),
            "{:?} contains spaces",
            features
        );
        self.features = Some(features);
        self
    }

    /// Build only the specified binary.
    pub fn bin_only(&'a mut self, bin: &'a str) -> &'a mut Command<'a> {
        self.bin_only = Some(bin);
        self
    }

    pub fn spawn(&self, cmd: &str) -> Result<JsonStream> {
        let mut spawner = process::Command::new("cargo");
        spawner
            .stdin(Stdio::null())
            .stderr(Stdio::inherit())
            .stdout(Stdio::piped())
            .args(&[cmd, "--message-format", "json"]);

        if let Some(ref path) = self.path {
            spawner.arg("--manifest-path");
            spawner.arg(path);
        }
        if let Some(ref features) = self.features {
            spawner.arg("--features");
            spawner.arg(features.join(" "));
        }
        if let Some(ref bin) = self.bin_only {
            spawner.args(&["--bin", bin]);
        }

        let mut child = spawner.spawn().chain_err(|| "couldn't run cargo")?;

        let stdout = mem::replace(&mut child.stdout, None).expect("child stdout");
        let stdout = BufReader::new(stdout);

        let stream = JsonStream { child, stdout, len_guess: 500 };
        Ok(stream)
    }
}

pub struct JsonStream {
    child: Child,
    stdout: BufReader<ChildStdout>,
    len_guess: usize,
}

impl JsonStream {
    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }
}

impl Drop for JsonStream {
    fn drop(&mut self) {
        self.kill()
    }
}

pub struct JsonLine(pub String);

impl JsonLine {
    pub fn decode<'a>(&'a self) -> serde_json::Result<Output<'a>> {
        serde_json::from_str(&self.0)
    }
}

impl AsRef<str> for JsonLine {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

impl Iterator for JsonStream {
    type Item = Result<JsonLine>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut line = String::with_capacity(self.len_guess);
        match self.stdout.read_line(&mut line) {
            Ok(0) => {
                self.len_guess = 0;
                None
            }
            Ok(n) => {
                self.len_guess = cmp::max(self.len_guess, n + (n / 2));
                if line.chars().rev().next() == Some('\n') {
                    line.pop();
                }
                Some(Ok(JsonLine(line)))
            }
            Err(e) => {
                self.kill();
                Some(Err(e).chain_err(|| "couldn't parse `cargo --message-format json`"))
            }
        }
    }
}

pub fn log_json_error<S: AsRef<str>>(error: &serde_json::Error, contents: S) {
    debug_assert!(error.line() < 2);

    let mut column = error.column();
    if column > 0 {
        column -= 1;
    }
    let mut window = contents.as_ref();
    let mut left = 0;
    let mut right = false;

    let width = terminal_size().map(|(w, _)| w.0 as usize).unwrap_or(100);
    let limit = cmp::max(width, 50);
    if window.len() > limit || column >= limit {
        // focus on the error site
        if column > (limit / 2) {
            left = column - (limit / 2);
            column -= left;
            window = &window[left..];
        }
        if window.len() > limit {
            right = true;
            window = &window[..limit - 1];
        }
    }

    let stderr = io::stderr();
    let mut log = stderr.lock();
    let _ = if left > 0 || right {
        writeln!(log, "While parsing JSON:\n{}\n", contents.as_ref())
    } else {
        writeln!(log, "While parsing JSON:")
    };
    if left > 0 {
        let _ = write!(log, "…");
        window = &window[1..];
    }
    let _ = writeln!(log, "{}{}", window, if right { "…" } else { "" });
    let _ = log.write_all(&vec![b' '; column]);
    let _ = writeln!(log, "^\n    {}\n", error);
}

pub mod util {
    use std::env;
    use std::path::{Path, PathBuf};

    /// Returns something like "~/.cargo" (but fully expanded).
    pub fn homedir_here() -> Option<PathBuf> {
        env::current_dir().ok().and_then(|cwd| homedir(&cwd))
    }

    /// Lifted directly from `cargo::util::config::homedir`.
    pub fn homedir(cwd: &Path) -> Option<PathBuf> {
        let cargo_home = env::var_os("CARGO_HOME").map(|home| cwd.join(home));
        if cargo_home.is_some() {
            return cargo_home;
        }

        // Windows homedir weirdness workaround follows.
        let home_dir_with_env = env::home_dir().map(|p| p.join(".cargo"));
        let home_dir = env::var_os("HOME");
        env::remove_var("HOME");
        let home_dir_without_env = env::home_dir().map(|p| p.join(".cargo"));
        if let Some(home_dir) = home_dir {
            env::set_var("HOME", home_dir);
        }

        match (home_dir_with_env, home_dir_without_env) {
            (None, None) => None,
            (None, Some(p)) | (Some(p), None) => Some(p),
            (Some(a), Some(b)) => {
                if cfg!(windows) && !a.exists() {
                    Some(b)
                } else {
                    Some(a)
                }
            }
        }
    }
}
