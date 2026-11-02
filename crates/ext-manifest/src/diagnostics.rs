//! Author-facing errors. Every problem has a stable code (`CY0101`), says what is wrong in one sentence and, when there
//! is one, the exact fix. `cyx`, the Extensions view and the registry print them the same way.

use std::fmt;
use std::ops::Range;

/// One entry of the `CY` error catalogue (`docs/extensions/reference/errors.md`, `cyx explain`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ErrorDoc {
    pub code: &'static str,
    /// What the error means, in one sentence.
    pub summary: &'static str,
    /// What to change, as printed after `= fix:`.
    pub fix: &'static str,
}

/// Stable error codes. The number never changes meaning; a retired code is not reused. What each one means and how to
/// fix it is [`Code::summary`] and [`Code::fix`]; `docs/extensions/reference/errors.md` is generated from them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Code {
    Syntax,
    TooLarge,
    BadId,
    BadVersion,
    BadText,
    BadPath,
    BadPermission,
    NotDeclared,
    WhyRequired,
    BadContribution,
    DuplicateId,
    NeedsCode,
    NeverActivates,
    BadWhen,
    BadDependency,
    BadComponent,
    NeedsNewerCodeyard,
}

impl Code {
    pub fn as_str(self) -> &'static str {
        match self {
            Code::Syntax => "CY0101",
            Code::TooLarge => "CY0102",
            Code::BadId => "CY0103",
            Code::BadVersion => "CY0104",
            Code::BadText => "CY0105",
            Code::BadPath => "CY0106",
            Code::BadPermission => "CY0200",
            Code::NotDeclared => "CY0201",
            Code::WhyRequired => "CY0202",
            Code::BadContribution => "CY0300",
            Code::DuplicateId => "CY0301",
            Code::NeedsCode => "CY0302",
            Code::NeverActivates => "CY0400",
            Code::BadWhen => "CY0500",
            Code::BadDependency => "CY0600",
            Code::BadComponent => "CY0700",
            Code::NeedsNewerCodeyard => "CY0701",
        }
    }

    /// What the code means, in one sentence.
    pub fn summary(self) -> &'static str {
        match self {
            Code::Syntax => "The manifest is not valid TOML, or a field has the wrong type or an unknown name.",
            Code::TooLarge => "The manifest is larger than the limit for `codeyard-extension.toml`.",
            Code::BadId => "`extension.id` is not `publisher.name` in lowercase `a-z 0-9 -`.",
            Code::BadVersion => "`extension.version` is not `major.minor.patch`.",
            Code::BadText => "A text field is empty or too long.",
            Code::BadPath => "A path is absolute, leaves the package or uses `\\`.",
            Code::BadPermission => "A `[permissions]` entry is malformed: a glob, host, program or command.",
            Code::NotDeclared => "The extension uses a program, file or host that `[permissions]` does not declare.",
            Code::WhyRequired => "The permissions are high risk and `[permissions] why` is missing.",
            Code::BadContribution => "A contribution is malformed: a field is missing, out of range or names something unknown.",
            Code::DuplicateId => "Two contributions of one kind share an id.",
            Code::NeedsCode => "A contribution runs extension code, but the manifest has no `[code]` table.",
            Code::NeverActivates => "The manifest has `[code]`, but nothing would ever start it.",
            Code::BadWhen => "A `when` or `enablement` expression does not parse or names an unknown key.",
            Code::BadDependency => "A dependency is malformed or names the extension itself.",
            Code::BadComponent => "`extension.wasm` is missing, is not a component, or imports or exports something it may not.",
            Code::NeedsNewerCodeyard => "The component uses an API version newer than this Codeyard.",
        }
    }

    /// The general fix. A diagnostic's own `fix`, when it has one, is the exact change for that place.
    pub fn fix(self) -> &'static str {
        match self {
            Code::Syntax => "correct the line shown; field names are kebab-case and listed in reference/manifest.md",
            Code::TooLarge => "shorten the manifest: move long text, such as setting descriptions, into the README",
            Code::BadId => "use `<publisher>.<name>` in lowercase a-z, 0-9 and -, such as `you.midnight`",
            Code::BadVersion => "write major.minor.patch, such as `0.1.0`; a pre-release adds a suffix, such as `1.0.0-beta.1`",
            Code::BadText => "fill the field in and keep it within its length in reference/limits.md",
            Code::BadPath => "use a relative path inside the extension's folder, with `/` between its parts",
            Code::BadPermission => {
                "write workspace globs, host names without scheme, port or path (`api.example.com`), program names, and core commands as `core:<id>`"
            }
            Code::NotDeclared => "add the entry to [permissions], or run `cyx check --fix`",
            Code::WhyRequired => "add to [permissions] one honest sentence for the install sheet: why = \"…\"",
            Code::BadContribution => "change the entry as the message says; reference/contributions.md lists every field",
            Code::DuplicateId => "give each entry of this kind its own id",
            Code::NeedsCode => "add [code] entry = \"Cargo.toml\" (or a .ts/.js file), or remove the contribution",
            Code::NeverActivates => {
                "add a [[commands]], [[views]], provider or [[tasks]] entry, or [activation] workspace-contains = [\"some-file\"]; or remove [code]"
            }
            Code::BadWhen => "use ==, !=, &&, ||, ! and parentheses over the keys listed in developer-guide.md#the-manifest",
            Code::BadDependency => "name another extension by its full id with a minimum version: \"other.ext\" = \"1.2\"",
            Code::BadComponent => "build extension.wasm with the Codeyard SDK: `cyx package`",
            Code::NeedsNewerCodeyard => "update Codeyard, or rebuild the extension with the SDK of the API version this Codeyard has",
        }
    }

    pub fn doc(self) -> ErrorDoc {
        ErrorDoc { code: self.as_str(), summary: self.summary(), fix: self.fix() }
    }

    pub const ALL: [Code; 17] = [
        Code::Syntax,
        Code::TooLarge,
        Code::BadId,
        Code::BadVersion,
        Code::BadText,
        Code::BadPath,
        Code::BadPermission,
        Code::NotDeclared,
        Code::WhyRequired,
        Code::BadContribution,
        Code::DuplicateId,
        Code::NeedsCode,
        Code::NeverActivates,
        Code::BadWhen,
        Code::BadDependency,
        Code::BadComponent,
        Code::NeedsNewerCodeyard,
    ];
}

/// One problem in a manifest. `span` is a byte range of the manifest text when the problem has a place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: Code,
    pub message: String,
    pub fix: Option<String>,
    pub span: Option<Range<usize>>,
}

impl Diagnostic {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Diagnostic { code, message: message.into(), fix: None, span: None }
    }

    pub fn fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }

    pub fn at(mut self, span: Option<Range<usize>>) -> Self {
        self.span = span;
        self
    }

    /// The 1-based line and column of the start of `span` in `text`.
    pub fn line_col(&self, text: &str) -> Option<(usize, usize)> {
        let start = self.span.as_ref()?.start.min(text.len());
        let before = text.get(..start)?;
        let line = before.matches('\n').count() + 1;
        let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
        Some((line, col))
    }

    /// The form `cyx` prints, always with a fix line:
    ///
    /// ```text
    /// error[CY0201]: run access to "taplo" is not declared
    ///   --> codeyard-extension.toml:12:11
    ///    = fix: add to [permissions]:  run = ["taplo"]
    /// ```
    pub fn render(&self, file: &str, text: &str) -> String {
        let place = match self.line_col(text) {
            Some((l, c)) => format!("{file}:{l}:{c}"),
            None => file.to_string(),
        };
        // Every error says what to change: the exact fix for this place, or else the code's general one.
        let fix = self.fix.as_deref().unwrap_or(self.code.fix());
        format!("error[{}]: {}\n  --> {place}\n   = fix: {fix}", self.code.as_str(), self.message)
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "error[{}]: {}", self.code.as_str(), self.message)?;
        if let Some(fix) = &self.fix {
            write!(f, " (fix: {fix})")?;
        }
        Ok(())
    }
}
