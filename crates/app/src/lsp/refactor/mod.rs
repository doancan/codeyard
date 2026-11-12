//! Refactoring preview: a language server's `WorkspaceEdit` is read (`edit`), made into a plan that can be reviewed and
//! partly selected (`plan`), and written as one transaction (`apply`). Nothing here is applied before the user chooses it,
//! except the one case the setting `editor.refactorPreview` allows: a change inside a single open file.

pub mod apply;
pub mod edit;
pub mod env;
pub mod plan;
