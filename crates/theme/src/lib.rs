//! Design tokens, generated at build time from `design/tokens*.json`.

include!(concat!(env!("OUT_DIR"), "/tokens.rs"));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_generated() {
        assert_eq!(DARK.bg, 0x120f0e);
        assert_ne!(DARK.bg, LIGHT.bg);
        assert_eq!(layout::ROW_H, 28.0);
        assert_eq!((text::CODE_MACOS.0, text::CODE_MACOS.1), (13.0, 20.0), "editor and terminal text on macOS");
        assert_eq!((text::CODE_OTHER.0, text::CODE_OTHER.1), (14.0, 20.0), "on Windows and Linux");
        assert_eq!(text::CODE, if cfg!(target_os = "macos") { text::CODE_MACOS } else { text::CODE_OTHER });
        assert_eq!(SPACE[4], 8.0);
    }
}

pub mod themes;
