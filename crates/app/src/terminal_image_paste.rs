//! Store a clipboard image on the terminal's remote host and return its path.
//!
//! The file remains available after pasting because a terminal program may read
//! it later. It is created privately and the system's temporary-file cleanup can
//! remove it after use.

use gpui::{Image, ImageFormat};
use host::{ExecOutput, Host};

// Leave room for protocol framing and bound memory and transfer time.
const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;

// All arguments are fixed by this module. Image bytes are passed via stdin,
// never interpolated into a shell command. mktemp atomically creates mode 0600.
const SAVE_SCRIPT: &str = r#"umask 077
f=$(mktemp "/tmp/codeyard-paste-XXXXXXXX.$1") || exit 1
if ! cat > "$f"; then rm -f -- "$f"; exit 1; fi
printf '%s' "$f""#;

/// Runs on a background executor; Host calls are blocking.
pub fn save_remote_image(host: &dyn Host, image: Image) -> Result<String, String> {
    save_with(image, |extension, data| host.exec("sh", &["-c", SAVE_SCRIPT, "codeyard", extension], None, Some(data)).map_err(|e| e.to_string()))
}

fn save_with(image: Image, exec: impl FnOnce(&str, Vec<u8>) -> Result<ExecOutput, String>) -> Result<String, String> {
    if image.bytes.is_empty() || image.bytes.len() > MAX_IMAGE_BYTES {
        return Err("Clipboard image is empty or exceeds the 32 MiB limit".into());
    }
    // Codex and other tools can consume these image types. Do not upload SVG or
    // unknown types as a mislabeled image.
    let ext = match image.format {
        ImageFormat::Png if image.bytes.starts_with(b"\x89PNG\r\n\x1a\n") => "png",
        ImageFormat::Jpeg if image.bytes.starts_with(b"\xff\xd8\xff") => "jpg",
        ImageFormat::Webp if image.bytes.starts_with(b"RIFF") && image.bytes.get(8..12) == Some(b"WEBP") => "webp",
        ImageFormat::Gif if image.bytes.starts_with(b"GIF87a") || image.bytes.starts_with(b"GIF89a") => "gif",
        _ => return Err("Unsupported or invalid clipboard image format".into()),
    };
    let output = exec(ext, image.bytes)?;
    if !output.ok() {
        return Err(format!("Could not save clipboard image on the remote host: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let path = std::str::from_utf8(&output.stdout).map_err(|_| "Invalid image path returned by remote host")?;
    // Do not feed arbitrary terminal control characters or shell syntax into a
    // terminal, even if the remote process returns an unexpected result.
    let name = path.strip_prefix("/tmp/codeyard-paste-").and_then(|s| s.strip_suffix(&format!(".{ext}")));
    if !name.is_some_and(|name| name.len() == 8 && name.bytes().all(|b| b.is_ascii_alphanumeric())) {
        return Err("Invalid image path returned by remote host".into());
    }
    Ok(path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(code: Option<i32>, stdout: &[u8], stderr: &[u8]) -> ExecOutput {
        ExecOutput { code, stdout: stdout.to_vec(), stderr: stderr.to_vec() }
    }

    #[test]
    fn sends_image_bytes_and_returns_safe_path() {
        let data = b"\x89PNG\r\n\x1a\nimage".to_vec();
        let image = Image::from_bytes(ImageFormat::Png, data.clone());
        let path = save_with(image, |ext, sent| {
            assert_eq!(ext, "png");
            assert_eq!(sent, data);
            Ok(output(Some(0), b"/tmp/codeyard-paste-Abc123XY.png", b""))
        })
        .unwrap();
        assert_eq!(path, "/tmp/codeyard-paste-Abc123XY.png");
    }

    #[test]
    fn rejects_bad_formats_and_unsafe_remote_paths() {
        let invalid = Image::from_bytes(ImageFormat::Png, b"not png".to_vec());
        assert!(save_with(invalid, |_, _| panic!("invalid image must not be sent")).is_err());
        let oversized = Image::from_bytes(ImageFormat::Png, vec![0; MAX_IMAGE_BYTES + 1]);
        assert!(save_with(oversized, |_, _| panic!("oversized image must not be sent")).unwrap_err().contains("32 MiB"));
        let image = Image::from_bytes(ImageFormat::Png, b"\x89PNG\r\n\x1a\n".to_vec());
        assert!(save_with(image.clone(), |_, _| Ok(output(Some(0), b"/tmp/codeyard-paste-Abc123XY.png\x1b", b""))).is_err());
        assert!(save_with(image, |_, _| Ok(output(Some(1), b"", b"disk full"))).unwrap_err().contains("disk full"));
    }

    #[cfg(unix)]
    #[test]
    fn stores_image_through_host_with_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let bytes = b"\x89PNG\r\n\x1a\ntest image".to_vec();
        let path = save_remote_image(&host::local(), Image::from_bytes(ImageFormat::Png, bytes.clone())).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        std::fs::remove_file(path).unwrap();
    }
}
