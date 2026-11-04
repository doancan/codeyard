//! The build script's own tests (`cargo test -p app --test agent_build`). rustc alone cannot compile them: the script uses `sha2`.

#[path = "../build.rs"]
mod build;
