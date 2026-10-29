//! Author-facing errors. Every problem has a stable code (`CY0101`), says what is wrong in one sentence and, when there
//! is one, the exact fix. `cyx`, the Extensions view and the registry print them the same way.

use std::fmt;
use std::ops::Range;

/// Stable error codes. The number never changes meaning; a retired code is not reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Code {
    /// The file is not valid TOML, or a field has the wrong type or an unknown name.
    Syntax,
    /// The manifest is larger than `limits::MANIFEST_BYTES`.
    TooLarge,
    /// `extension.id` is not `publisher.name` in lowercase `a-z 0-9 -`.
    BadId,
    /// `extension.version` (or a dependency version) is not `major.minor.patch`.
    BadVersion,
    /// A text field is empty or too long.
    BadText,
    /// A path is absolute, leaves the package or uses `\`.
    BadPath,
    /// A permission entry is malformed (glob, host, program, command).
    BadPermission,
    /// A program, file or host is used but not declared in `[permissions]`.
    NotDeclared,
    /// The permission set is high risk and has no `why`.
    WhyRequired,
    /// A contribution is malformed.
    BadContribution,
    /// Two contributions of one kind share an id.
    DuplicateId,
    /// A contribution that runs extension code, but the manifest has no `[code]`.
    NeedsCode,
    /// `[code]` is present but nothing would ever start it.
    NeverActivates,
    /// A `when` or `enablement` expression does not parse or names an unknown key.
    BadWhen,
    /// A dependency is malformed or names the extension itself.
    BadDependency,
    /// `extension.wasm` is missing, not a component, or imports or exports something it may not.
    BadComponent,
    /// The component uses an API version newer than this Codeyard.
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

    /// The form `cyx` prints:
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
        let mut out = format!("error[{}]: {}\n  --> {place}", self.code.as_str(), self.message);
        if let Some(fix) = &self.fix {
            out.push_str("\n   = fix: ");
            out.push_str(fix);
        }
        out
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
