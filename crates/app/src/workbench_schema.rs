//! Workbench: validate JSON and YAML files against their schema. When a file is opened and when it is edited (after a short delay),
//! the schema is found in the background (declaration in the document, `json.schemas`/`yaml.schemas`, SchemaStore) and the document is validated;
//! diagnostics go to the Problems panel and the editor, and the schema is attached to the editor (for hover and completion).

use super::*;

/// In a remote workspace the schema files live on the server: relative `$schema` and `json.schemas` paths are read from there.
struct HostFiles(Arc<dyn Host>);

impl schema::store::FileSource for HostFiles {
    fn read(&self, path: &str) -> Result<String, String> {
        self.0.read(path).map(|(d, _)| String::from_utf8_lossy(&d).into_owned()).map_err(|e| format!("{path}: {e}"))
    }

    fn mtime(&self, path: &str) -> Option<i64> {
        self.0.stat(path).ok().map(|m| m.mtime_ms)
    }
}

impl Workbench {
    pub(super) fn schema_service(&mut self, cx: &App) -> Arc<schema::ide::Service> {
        if let Some(s) = &self.lsp.schema {
            return s.clone();
        }
        // Tests don't go out to the network and don't write to the real cache.
        let download = cx.global::<Settings>().schema_download && !cfg!(test);
        let cache = dirs::cache_dir().map(|d| d.join(host::ssh::APP_DIR).join("schemas")).filter(|_| !cfg!(test));
        let svc = schema::ide::Service::new(cache, download);
        self.lsp.schema = Some(svc.clone());
        svc
    }

    /// Schedule a (re)validation for the editor; `delay`: wait after an edit.
    pub(super) fn schedule_schema(&mut self, ed: &Entity<EditorView>, delay: Duration, cx: &mut Context<Self>) {
        let path = ed.read(cx).path.clone();
        let Some(format) = schema::ide::format_of(&path) else { return };
        let settings = cx.global::<Settings>();
        if !settings.schema_validate {
            self.lsp.schema_tasks.remove(&path);
            if self.lsp.diagnostics.remove(&path).is_some() {
                ed.update(cx, |e, cx| {
                    e.set_schema(None);
                    e.set_diagnostics(Vec::new(), cx);
                });
            }
            return;
        }
        // The user's own mappings first (the first match wins), then what installed extensions contribute.
        let mut user = settings.schema_associations.clone();
        user.extend(crate::ext::schema_entries());
        let root = self.root(cx);
        let host = ed.read(cx).host_arc();
        let files: Option<Arc<dyn schema::store::FileSource>> = host.is_remote().then(|| Arc::new(HostFiles(host)) as Arc<dyn schema::store::FileSource>);
        let svc = self.schema_service(cx);
        svc.set_local_only(crate::ext::package_dirs());
        svc.store.set_download(settings.schema_download && !cfg!(test));
        // A `$schema` in an untrusted folder must not make us fetch from an arbitrary server.
        svc.store.set_restricted(root.as_deref().is_some_and(|r| !self.is_trusted_root(r, cx)));
        svc.store.gate().set_mode(schema::gate::Confirm::parse(&settings.http_confirm).unwrap_or_default());
        let service = svc.clone();
        let weak = ed.downgrade();
        let path_key = path.clone();
        let slot = path.clone();
        let task = cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let Ok(Some((text, version))) = weak.update(cx, |e, _| {
                let rope = e.rope_snapshot();
                (rope.len_bytes() <= schema::ide::MAX_BYTES).then(|| (rope.to_string(), e.version()))
            }) else {
                return;
            };
            let p = path.clone();
            let t = text.clone();
            let analysis = cx.background_executor().spawn(async move { svc.analyze(&p, &t, format, &user, root.as_deref(), files) }).await;
            this.update(cx, |this, cx| {
                let Some(ed) = weak.upgrade() else { return };
                // Edited meanwhile: this result is stale, a newer run is already scheduled.
                if ed.read(cx).version() != version {
                    return;
                }
                let diags: Vec<crate::lsp::Diagnostic> = analysis
                    .diags
                    .iter()
                    .map(|d| crate::lsp::Diagnostic {
                        start: schema::doc::position(&text, d.start),
                        end: schema::doc::position(&text, d.end),
                        severity: d.severity,
                        message: d.message.clone(),
                        source: Some("schema".into()),
                        ..Default::default()
                    })
                    .collect();
                if diags.is_empty() {
                    this.lsp.diagnostics.remove(&path_key);
                } else {
                    this.lsp.diagnostics.insert(path_key.clone(), diags);
                }
                let merged = this.diagnostics_for(&path_key);
                let binding = analysis.set.map(|set| Arc::new(crate::editor::SchemaBinding { set, format }));
                ed.update(cx, |e, cx| {
                    e.set_schema(binding);
                    e.set_diagnostics(merged, cx);
                });
                this.ask_schema_hosts(&service, cx);
                cx.notify();
            })
            .ok();
        });
        self.lsp.schema_tasks.insert(slot, task);
    }

    /// A background download wanted a host that has not been approved (`http.confirmRequests`): ask once per host. Allowing it
    /// re-checks every open file; declining is remembered for the session and the file keeps its "could not load" note.
    pub(super) fn ask_schema_hosts(&mut self, svc: &Arc<schema::ide::Service>, cx: &mut Context<Self>) {
        let gate = svc.store.gate().clone();
        for host in gate.take_pending() {
            if !self.lsp.schema_asking.insert(host.clone()) {
                continue;
            }
            let (handle, gate) = (self.window_handle, gate.clone());
            cx.spawn(async move |this, cx| {
                let answer = cx.update_window(handle, |_, window, cx| crate::questions::schema_download(&host).ask(window, cx)).ok();
                let allowed = match answer {
                    Some(a) => a.await == Some(0),
                    None => false,
                };
                this.update(cx, |this, cx| {
                    this.lsp.schema_asking.remove(&host);
                    if allowed {
                        gate.approve(&host);
                    } else {
                        gate.refuse(&host);
                    }
                    for ed in this.all_editors() {
                        this.schedule_schema(&ed, Duration::ZERO, cx);
                    }
                })
                .ok();
            })
            .detach();
        }
    }

    /// "Schemas: Clear Cache": downloaded schemas and the catalog are deleted, open files are re-validated.
    pub(super) fn clear_schema_cache(&mut self, _: &ClearSchemaCache, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = &self.lsp.schema {
            s.clear();
        }
        for ed in self.all_editors() {
            self.schedule_schema(&ed, Duration::ZERO, cx);
        }
        self.notice = Some(Notice::Info(crate::i18n::t("Schema cache cleared. Schemas are downloaded again when needed.").into()));
        cx.notify();
    }
}
