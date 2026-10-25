//! Workbench: applying replacements to many files. Replace in Files and structural replace differ only in how they find
//! the edits, so both hand a function to `apply_file_edits`, which then does what is the same for both: open files get one
//! undo step, closed files are read, edited as bytes (mixed line endings survive) and written back against the hash that was read.

use std::ops::Range;

use super::*;

/// Edits for one file: char ranges in the buffer and the text that replaces each. Gets the path too, because a structural
/// replace picks the grammar by file name. The function runs on the UI thread for open files and on a background thread
/// for the others.
pub(crate) type EditsFor = Arc<dyn Fn(&str, &buffer::Buffer) -> Vec<(Range<usize>, String)> + Send + Sync>;

impl Workbench {
    pub(crate) fn apply_file_edits(&mut self, files: Vec<String>, edits_for: EditsFor, cx: &mut Context<Self>) {
        let mut closed = Vec::new();
        let mut changed = 0;
        for path in files {
            match self.all_editors().into_iter().find(|e| e.read(cx).path == path) {
                Some(ed) => {
                    let f = edits_for.clone();
                    ed.update(cx, |e, cx| changed += e.apply_buffer_edits(|b| f(&path, b), cx));
                }
                None => closed.push(path),
            }
        }
        let host = self.remote.host.clone();
        let f = edits_for.clone();
        let task = cx.background_executor().spawn(async move {
            let (mut done, mut errors, mut written) = (0, Vec::new(), Vec::new());
            for path in closed {
                let r = (|| -> Result<usize, String> {
                    let (data, hash) = host.read(&path).map_err(|e| e.to_string())?;
                    let mut n = 0;
                    let edited = buffer::Buffer::edit_bytes(&data, |b| {
                        let edits = f(&path, b);
                        n = edits.len();
                        edits
                    })
                    .map_err(|_| "not UTF-8".to_string())?;
                    if let Some(bytes) = edited {
                        host.write(&path, bytes, Some(hash)).map_err(|e| e.to_string())?;
                    }
                    Ok(n)
                })();
                match r {
                    Ok(n) if n > 0 => {
                        done += n;
                        written.push(path);
                    }
                    Ok(_) => {}
                    Err(e) => errors.push(format!("{}: {e}", ui::file_name(&path))),
                }
            }
            (done, errors, written)
        });
        cx.spawn(async move |this, cx| {
            let (done, errors, written) = task.await;
            this.update(cx, |this, cx| {
                let total = changed + done;
                this.notice = Some(if errors.is_empty() {
                    Notice::Info(crate::i18n::tn("Replaced {} match", "Replaced {} matches", total, &[]).into())
                } else {
                    Notice::Error(crate::i18n::tf("Replaced {0}; could not change {1}", &[&total, &errors.join(", ")]).into())
                });
                this.notify_files_changed(&written);
                this.refresh_git(300, cx);
                this.search.update(cx, |s, cx| s.rerun(cx));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
