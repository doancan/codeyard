//! `codeyard-extension`: write Codeyard extensions in Rust (docs/extensions/commands.md).
//!
//! ```ignore
//! use codeyard_extension::prelude::*;
//!
//! struct Hello;
//!
//! impl Extension for Hello {
//!     fn activate(ctx: Context) -> Result<()> {
//!         ctx.command("greet", async |ctx, _args| {
//!             if let Some(name) = ctx.input(InputOptions::prompt("Your name?")).await? {
//!                 ctx.notice(format!("Hello, {name}!"));
//!             }
//!             Ok(())
//!         });
//!         Ok(())
//!     }
//! }
//!
//! codeyard_extension::export!(Hello);
//! ```
//!
//! Build with plain cargo for `wasm32-wasip2` (`cyx package` does it). Calls that reach the app are `async`: each
//! returns a ticket at once, and a small executor driven by the guest's exports resumes the task when the ticket
//! completes, so an export never waits on I/O. On the host target the same code talks to the mock host of
//! `codeyard-extension-test`, so `cargo test` runs an extension's unit tests natively.
//!
//! Design: docs/plugin-architecture.md §5.2.

extern crate self as codeyard_extension;

mod context;
mod error;
mod guest;
pub mod net;
pub mod process;
pub mod protocol;
pub mod pulls;
mod runtime;
pub mod types;

#[cfg(not(target_arch = "wasm32"))]
pub mod native;
#[cfg(not(target_arch = "wasm32"))]
use native as sys;

#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
pub mod wasm;
#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
pub use wasm as __wasm;
#[cfg(target_arch = "wasm32")]
use wasm as sys;

pub use context::{Context, Progress, State, Storage, Timer};
pub use error::{Denied, Error, Result, Why};
pub use guest::{RELOAD, RELOAD_STATE_KEY};
pub use serde_json::{Value, json};

/// An extension. `activate` registers its commands, views and providers; Codeyard calls it once, before anything else.
pub trait Extension: 'static {
    fn activate(ctx: Context) -> Result<()>;

    /// Called before the extension stops. Keep it short.
    fn deactivate(_ctx: Context) {}
}

/// Everything most extensions use.
pub mod prelude {
    pub use crate::net::{Request, Response};
    pub use crate::process::{Chunk, Command, Output};
    pub use crate::pulls::{ChangedFile, Check, CheckState, Checkout, Comment, FileContents, FileStatus, PullRequest, PullRequestDetails, PullState, Remote};
    pub use crate::types::*;
    pub use crate::{Context, Denied, Error, Extension, Result, Value, Why, json};
}

/// Make `$ty` the extension this component runs. On the host target (unit tests) it expands to nothing: the mock host
/// starts the extension by type.
#[macro_export]
macro_rules! export {
    ($ty:ty) => {
        #[cfg(target_arch = "wasm32")]
        const _: () = {
            type __CodeyardExtension = $crate::__wasm::Glue<$ty>;
            $crate::__wasm::bindings::export! { __CodeyardExtension with_types_in $crate::__wasm::bindings }
        };
    };
}
