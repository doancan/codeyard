//! Refactoring preview: a language server's `WorkspaceEdit` is read (`edit`), made into a plan that can be reviewed and
//! partly selected (`plan`), and written as one transaction (`apply`). Nothing here is applied before the user chooses it,
//! except the one case the setting `editor.refactorPreview` allows: a change inside a single open file.

pub mod apply;
pub mod edit;
pub mod env;
pub mod journal;
#[cfg(test)]
mod lsp_tests;
pub mod plan;
pub mod preview;
#[cfg(test)]
mod workbench_tests;

/// The keys of the review page (see `preview`).
pub fn key_bindings() -> [gpui::KeyBinding; 8] {
    preview::key_bindings()
}
