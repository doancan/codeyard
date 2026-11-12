//! Workbench: what rust-analyzer shows about the code at the cursor: its documentation (docs.rs or the local copy), the
//! HIR and MIR of a function and the syntax tree of the file. The text opens as a read-only page; nothing here waits on
//! the server on the UI thread.

use std::sync::Arc;

use gpui::{Context, Window};
use serde_json::{Value, json};

use super::super::*;
#[cfg(not(test))]
use super::RUST_REQUEST_TIMEOUT;
use crate::i18n::{t, tf};
use crate::lsp::{self, METHOD_NOT_FOUND};
use crate::rust_toolchain::views;

gpui::actions!(workbench, [RustOpenDocs, RustViewHir, RustViewMir, RustViewSyntaxTree]);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    Docs,
    Hir,
    Mir,
    SyntaxTree,
}

impl View {
    fn method(self) -> &'static str {
        match self {
            Self::Docs => lsp::EXTERNAL_DOCS,
            Self::Hir => lsp::VIEW_HIR,
            Self::Mir => lsp::VIEW_MIR,
            Self::SyntaxTree => lsp::VIEW_SYNTAX_TREE,
        }
    }

    /// The page's title for the file it is about.
    fn title(self, file: &str) -> String {
        match self {
            Self::Hir => tf("HIR of {}", &[&file]),
            Self::Mir => tf("MIR of {}", &[&file]),
            Self::SyntaxTree => tf("Syntax tree of {}", &[&file]),
            Self::Docs => file.to_string(),
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            Self::Hir => "HIR",
            Self::Mir => "MIR",
            _ => "Syntax tree",
        }
    }
}

/// The server, the cursor, the file and its text, for a request about the active Rust file.
struct Asked {
    client: Arc<lsp::Client>,
    position: (u32, u32),
    path: String,
    text: String,
}

impl Workbench {
    pub(in super::super) fn rust_open_docs(&mut self, _: &RustOpenDocs, window: &mut Window, cx: &mut Context<Self>) {
        self.rust_view(View::Docs, window, cx);
    }

    pub(in super::super) fn rust_view_hir(&mut self, _: &RustViewHir, window: &mut Window, cx: &mut Context<Self>) {
        self.rust_view(View::Hir, window, cx);
    }

    pub(in super::super) fn rust_view_mir(&mut self, _: &RustViewMir, window: &mut Window, cx: &mut Context<Self>) {
        self.rust_view(View::Mir, window, cx);
    }

    pub(in super::super) fn rust_view_syntax_tree(&mut self, _: &RustViewSyntaxTree, window: &mut Window, cx: &mut Context<Self>) {
        self.rust_view(View::SyntaxTree, window, cx);
    }

    /// The active Rust file with its server, or a notice that says why not.
    fn rust_asked(&mut self, cx: &mut Context<Self>) -> Option<Asked> {
        let Some(editor) = self.active_editor().cloned().filter(|ed| ed.read(cx).language() == "Rust") else {
            self.notify(NoticeSource::Language, Notice::Info(t("Open a Rust file first").into()));
            cx.notify();
            return None;
        };
        // The text sent with `didChange` is what the server answers about.
        let asked = editor.update(cx, |e, _| e.lsp_context().map(|(client, position, ..)| (client, position, e.path.clone(), e.buffer_text())));
        match asked.filter(|(client, ..)| client.config.language == "rust") {
            Some((client, position, path, text)) => Some(Asked { client, position, path, text }),
            None => {
                let path = editor.read(cx).path.clone();
                self.notify(NoticeSource::Language, Notice::Info(self.rust_unavailable_message(&path, cx).into()));
                cx.notify();
                None
            }
        }
    }

    fn rust_view(&mut self, view: View, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Asked { client, position, path, text }) = self.rust_asked(cx) else { return };
        let document = json!({ "uri": lsp::path_to_uri(&path) });
        let params = match view {
            View::SyntaxTree => json!({ "textDocument": document, "range": Value::Null }),
            _ => json!({ "textDocument": document, "position": { "line": position.0, "character": position.1 } }),
        };
        #[cfg(test)]
        let timeout = self.rust_request_timeout;
        #[cfg(not(test))]
        let timeout = RUST_REQUEST_TIMEOUT;
        let task = cx.background_executor().spawn(async move { client.request_raw(view.method(), params, timeout) });
        let local = !self.remote.host.is_remote();
        let generation = self.lsp.generation;
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                if this.lsp.generation != generation {
                    return;
                }
                match result {
                    Ok(answer) => this.show_rust_answer(view, &answer, &path, &text, local, window, cx),
                    Err(e) => {
                        let message =
                            if e.code == Some(METHOD_NOT_FOUND) { t("The Rust language server does not support this command").into() } else { e.message };
                        this.notify(NoticeSource::Language, Notice::Info(message.into()));
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn show_rust_answer(&mut self, view: View, answer: &Value, path: &str, text: &str, local: bool, window: &mut Window, cx: &mut Context<Self>) {
        if view == View::Docs {
            return self.open_rust_docs(lsp::parse_external_docs(answer), local, cx);
        }
        let Some(shown) = answer.as_str().filter(|s| !s.trim().is_empty()) else {
            self.notify(NoticeSource::Language, Notice::Info(t("Nothing to show for the item at the cursor").into()));
            return cx.notify();
        };
        let file = ui::file_name(path);
        let body = match view {
            View::SyntaxTree => views::syntax_tree_listing(shown, text).unwrap_or_else(|| shown.to_string()),
            _ => shown.to_string(),
        };
        let page = views::text_document(&view.title(file), &body);
        self.show_pull_page(views::view_file_name(view.prefix(), file), &page, window, cx);
    }

    /// The local copy when this machine has it, otherwise the web page.
    fn open_rust_docs(&mut self, links: lsp::DocsLinks, local: bool, cx: &mut Context<Self>) {
        let local_copy = links.local.as_deref().filter(|url| local && lsp::local_docs_path(url).is_some_and(|p| std::path::Path::new(&p).is_file()));
        match local_copy.or(links.web.as_deref()) {
            Some(url) => cx.open_url(url),
            None if links.rejected => {
                self.notify(
                    NoticeSource::Language,
                    Notice::Error(t("The language server sent a documentation link that is not a web page or a local HTML file, so it was not opened").into()),
                );
                cx.notify();
            }
            None => {
                self.notify(NoticeSource::Language, Notice::Info(t("No documentation found for the item at the cursor").into()));
                cx.notify();
            }
        }
    }
}
