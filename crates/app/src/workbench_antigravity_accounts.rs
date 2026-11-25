//! macOS Antigravity profiles. Google owns the system entry and Codeyard keeps separate saved entries.

use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::process::{Command, Stdio};

const SYSTEM: (&str, &str) = ("gemini", "antigravity");
const SECURITY: &str = "/usr/bin/security";
const NOT_FOUND: i32 = 44;

pub(super) struct Accounts {
    service: String,
}

pub(super) struct Login {
    accounts: Accounts,
    previous: Option<Credential>,
    target: u32,
    count: u32,
}

struct Credential(Vec<u8>);

impl Credential {
    fn identity(&self) -> Option<String> {
        let text = std::str::from_utf8(&self.0).ok()?;
        let decoded;
        let json = if let Some(encoded) = text.strip_prefix("go-keyring-base64:") {
            decoded = crate::texttools::base64_decode(encoded)?;
            &decoded
        } else {
            text
        };
        let value = serde_json::from_str::<serde_json::Value>(json).ok()?;
        value["id_token_email"].as_str().filter(|email| !email.is_empty()).map(str::to_ascii_lowercase)
    }

    fn is_same_account(&self, other: &Self) -> bool {
        self.0 == other.0 || self.identity().zip(other.identity()).is_some_and(|(a, b)| a == b)
    }
}

impl Accounts {
    pub(super) fn new(scope: &str) -> Self {
        Self { service: format!("com.codeyard.antigravity.{scope}") }
    }

    fn saved(&self, profile: u32) -> io::Result<Option<Credential>> {
        read((&self.service, &profile.to_string()))
    }

    fn save(&self, profile: u32, data: &Credential) -> io::Result<()> {
        write((&self.service, &profile.to_string()), data)
    }

    /// History must come from the selected account even when another window changes the system login.
    pub(super) fn can_list(&self, profile: u32) -> io::Result<bool> {
        let Some(saved) = self.saved(profile)? else { return Ok(profile == 0) };
        Ok(read(SYSTEM)?.as_ref().is_some_and(|current| saved.is_same_account(current)))
    }

    /// Restore a saved profile if needed. The original system account is usable before enrollment.
    pub(super) fn activate(&self, profile: u32, count: u32) -> io::Result<()> {
        let Some(saved) = self.saved(profile)? else {
            return if profile == 0 && count == 1 {
                Ok(())
            } else {
                Err(io::Error::new(io::ErrorKind::NotFound, "The selected Antigravity account is missing from Keychain"))
            };
        };
        if read(SYSTEM)?.as_ref().is_some_and(|current| current.is_same_account(&saved)) {
            return Ok(());
        }
        write(SYSTEM, &saved)
    }

    /// Persist refreshed credentials only when they still belong to the selected profile.
    pub(super) fn refresh(&self, profile: u32) -> io::Result<()> {
        if let (Some(saved), Some(current)) = (self.saved(profile)?, read(SYSTEM)?)
            && saved.0 != current.0
            && saved.is_same_account(&current)
        {
            self.save(profile, &current)?;
        }
        Ok(())
    }

    /// Journal the current login before launching agy's own interactive OAuth flow.
    pub(super) fn begin_login(self, profile: u32, target: u32, count: u32) -> io::Result<Login> {
        let previous = read(SYSTEM)?;
        let existing = self.saved(profile)?;
        if let Some(current) = &previous {
            match &existing {
                Some(saved) if saved.is_same_account(current) && saved.0 != current.0 => self.save(profile, current)?,
                None if profile == 0 && count == 1 => self.save(profile, current)?,
                _ => {}
            }
        }
        let target = if count == 1 && previous.is_none() && existing.is_none() { 0 } else { target };
        Ok(Login { accounts: self, previous, target, count })
    }
}

impl Login {
    /// Save a completed login. Restore the previous active login if enrollment failed.
    pub(super) fn finish(self, success: bool) -> io::Result<Option<u32>> {
        let completed = if success { self.save_new_login().map(Some) } else { Ok(None) };
        if !matches!(completed, Ok(Some(_)))
            && let Some(previous) = &self.previous
            && read(SYSTEM)?.is_none_or(|current| current.0 != previous.0)
        {
            write(SYSTEM, previous)?;
        }
        completed
    }

    fn save_new_login(&self) -> io::Result<u32> {
        let current = read(SYSTEM)?.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Antigravity did not save a Google login in Keychain"))?;
        if current.identity().is_none() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Antigravity did not report the Google account identity"));
        }
        for profile in 0..self.count {
            if profile != self.target && self.accounts.saved(profile)?.is_some_and(|saved| saved.is_same_account(&current)) {
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, "This Google account is already linked to a Codeyard profile"));
            }
        }
        if self.target == self.count && self.accounts.saved(self.target)?.is_some_and(|saved| !saved.is_same_account(&current)) {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "An Antigravity account was already added in another window"));
        }
        self.accounts.save(self.target, &current)?;
        Ok(self.target)
    }
}

fn read((service, account): (&str, &str)) -> io::Result<Option<Credential>> {
    let output =
        Command::new(SECURITY).args(["find-generic-password", "-s", service, "-a", account, "-w"]).stdin(Stdio::null()).stderr(Stdio::null()).output()?;
    if output.status.code() == Some(NOT_FOUND) {
        return Ok(None);
    }
    let mut secret = output.stdout;
    if !output.status.success() || secret.pop() != Some(b'\n') || secret.is_empty() {
        return Err(io::Error::other("Could not read the Antigravity entry from Keychain"));
    }
    Ok(Some(Credential(secret)))
}

fn write((service, account): (&str, &str), data: &Credential) -> io::Result<()> {
    // Use the same Apple-signed writer as agy. Writing from Codeyard changes the
    // entry's code-signing partition and makes agy prompt on its next access.
    // The command goes through an anonymous pipe. Credentials never appear in arguments.
    let mut input = format!("add-generic-password -U -s {service} -a {account} -X ");
    if input.len() + data.0.len().saturating_mul(2) + 1 >= 4096 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "The Antigravity entry exceeds the Keychain tool's input limit"));
    }
    for byte in &data.0 {
        let _ = write!(&mut input, "{byte:02x}");
    }
    input.push('\n');

    let mut child = Command::new(SECURITY).args(["-q", "-i"]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    let sent =
        child.stdin.take().ok_or_else(|| io::Error::other("Could not open the Keychain tool input")).and_then(|mut pipe| pipe.write_all(input.as_bytes()));
    let status = child.wait()?;
    sent?;
    if !status.success() || read((service, account))?.is_none_or(|saved| saved.0 != data.0) {
        return Err(io::Error::other("Could not update the Antigravity entry in Keychain"));
    }
    Ok(())
}

#[cfg(test)]
#[test]
fn recognize_a_refreshed_login_without_confusing_different_accounts() {
    let credential = |email: &str, contents: &str| {
        let json = serde_json::json!({"id_token_email": email, "token": {"access_token": contents}});
        Credential(format!("go-keyring-base64:{}", crate::texttools::base64_encode(&json.to_string())).into_bytes())
    };
    let a = credential("one@example.invalid", "first");
    assert!(a.is_same_account(&credential("ONE@example.invalid", "refreshed")));
    assert!(!a.is_same_account(&credential("two@example.invalid", "first")));
}
