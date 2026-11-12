//! Workbench: spell checking. Dictionaries load in the background (English ships with the app; others are downloaded with "Spelling: Install
//! Dictionary…" and cached) and are attached to all open editors. "Quick Fix" offers a suggestion list on a misspelled word,
//! "Add to user dictionary" and "Ignore in this session".

use std::path::PathBuf;

use super::*;

/// What a row in the suggested-fix list does.
#[derive(Clone, Debug)]
pub(super) enum SpellChoice {
    Replace(String),
    AddToDictionary,
    IgnoreOnce,
}

impl Workbench {
    fn spell_root(&self) -> Option<PathBuf> {
        self.lsp.spell_root.clone().or_else(|| dirs::cache_dir().map(|d| d.join(host::ssh::APP_DIR).join("spelling")))
    }

    fn user_words_path(&self) -> Option<PathBuf> {
        self.cfg.dir.as_ref().map(|d| d.join("spelling.txt"))
    }

    /// (Re)build the checker from the settings and attach it to editors. Does nothing if the language list hasn't changed.
    pub(super) fn refresh_speller(&mut self, cx: &mut Context<Self>) {
        let s = cx.global::<Settings>();
        if !s.spelling {
            if self.lsp.spell.take().is_some() {
                self.lsp.spell_langs.clear();
                for ed in self.all_editors() {
                    ed.update(cx, |e, cx| e.set_speller(None, cx));
                }
            }
            return;
        }
        let want = s.spell_languages.clone();
        if self.lsp.spell.is_some() && self.lsp.spell_langs == want || self.lsp.spell_loading == Some(want.clone()) {
            return;
        }
        self.lsp.spell_loading = Some(want.clone());
        let (root, words_path) = (self.spell_root(), self.user_words_path());
        let langs = want.clone();
        let task = cx.background_executor().spawn(async move {
            let mut dicts = Vec::new();
            let mut missing: Vec<String> = Vec::new();
            for id in &langs {
                if id == "en" || id == "en-US" {
                    dicts.push(spell::embedded_english());
                    continue;
                }
                let dir = root.as_ref().map(|r| spell::dir_for(r, id));
                match dir {
                    Some(dir) if dir.join("index.dic").exists() => match spell::load_dir(&dir) {
                        Ok(d) => dicts.push(d),
                        // Exists but unreadable (corrupt download, hand-edited): "not installed" would mislead.
                        Err(e) => {
                            missing.push(crate::i18n::tf("{0} could not be read ({1})", &[&id, &e.lines().next().unwrap_or(crate::i18n::t("unknown error"))]))
                        }
                    },
                    _ => missing.push(crate::i18n::tf("{} is not installed", &[&id])),
                }
            }
            let words: Vec<String> = words_path
                .and_then(|p| std::fs::read_to_string(p).ok())
                .map(|t| t.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
                .unwrap_or_default();
            (spell::Speller::new(dicts, words), missing)
        });
        cx.spawn(async move |this, cx| {
            let (speller, missing) = task.await;
            this.update(cx, |this, cx| {
                // Setting changed meanwhile: this result is stale, a newer load is running.
                if this.lsp.spell_loading.as_ref() != Some(&want) {
                    return;
                }
                this.lsp.spell_loading = None;
                let speller = Arc::new(speller);
                this.lsp.spell = Some(speller.clone());
                this.lsp.spell_langs = want;
                for ed in this.all_editors() {
                    let s = speller.clone();
                    ed.update(cx, |e, cx| e.set_speller(Some(s), cx));
                }
                if !missing.is_empty() && !matches!(this.notice(), Some(Notice::Error(_))) {
                    this.notify(
                        NoticeSource::Language,
                        Notice::Info(crate::i18n::tf("Spelling: {}. Run “Spelling: Install Dictionary…”.", &[&missing.join(", ")]).into()),
                    );
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Give the newly opened editor the current checker (start loading if there isn't one).
    pub(super) fn attach_speller(&mut self, ed: &Entity<EditorView>, cx: &mut Context<Self>) {
        match self.lsp.spell.clone() {
            Some(s) if cx.global::<Settings>().spelling => ed.update(cx, |e, cx| e.set_speller(Some(s), cx)),
            _ => self.refresh_speller(cx),
        }
    }

    /// Quick Fix: if the cursor is on a misspelled word, show the suggestion list. `true`: handled.
    pub(super) fn try_spell_fix(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let (Some(speller), Some(ed)) = (self.lsp.spell.clone(), self.active_editor().cloned()) else { return false };
        let Some((range, word)) = ed.read(cx).misspelling_at(ed.read(cx).cursor_char(), cx) else { return false };
        let suggestions = speller.suggest(&word);
        let mut choices: Vec<SpellChoice> = suggestions.iter().cloned().map(SpellChoice::Replace).collect();
        let mut items: Vec<PickItem> = suggestions.iter().map(|s| PickItem { label: s.clone().into(), detail: None, keys: None }).collect();
        choices.push(SpellChoice::AddToDictionary);
        items.push(PickItem {
            label: crate::i18n::tf("Add “{}” to the dictionary", &[&word]).into(),
            detail: Some(crate::i18n::t("your words").into()),
            keys: None,
        });
        choices.push(SpellChoice::IgnoreOnce);
        items.push(PickItem { label: crate::i18n::tf("Ignore “{}” in this session", &[&word]).into(), detail: None, keys: None });
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], &crate::i18n::tf("Spelling: “{}”", &[&word]), "", None, cx));
        self.show_overlay(picker, OverlayKind::SpellFix { editor: ed.downgrade(), range, word, choices }, window, cx);
        true
    }

    pub(super) fn spell_fix_action(&mut self, _: &FixSpelling, window: &mut Window, cx: &mut Context<Self>) {
        if !self.try_spell_fix(window, cx) {
            self.notify(NoticeSource::Language, Notice::Info(crate::i18n::t("No misspelled word at the cursor.").into()));
            cx.notify();
        }
    }

    pub(super) fn apply_spell_choice(
        &mut self,
        editor: gpui::WeakEntity<EditorView>,
        range: std::ops::Range<usize>,
        word: String,
        choice: SpellChoice,
        cx: &mut Context<Self>,
    ) {
        match choice {
            SpellChoice::Replace(text) => {
                // Case: if the misspelled word is Capitalized, the suggestion should be too.
                let text = match word.chars().next() {
                    Some(c) if c.is_uppercase() && text.chars().next().is_some_and(char::is_lowercase) => {
                        text.chars().next().map(|f| f.to_uppercase().collect::<String>() + &text[f.len_utf8()..]).unwrap_or(text)
                    }
                    _ => text,
                };
                editor.update(cx, |e, cx| e.replace_range_with(range, &text, cx)).ok();
            }
            SpellChoice::AddToDictionary => {
                if let Some(s) = &self.lsp.spell {
                    s.add_user_word(&word);
                }
                if self.user_words_path().is_none() {
                    // No settings folder: the word is session-only, don't make it look permanent.
                    self.notify(
                        NoticeSource::Language,
                        Notice::Info(
                            crate::i18n::tf("Added “{}” for this session only: there is no settings folder to keep your dictionary in.", &[&word]).into(),
                        ),
                    );
                }
                if let Some(path) = self.user_words_path() {
                    let result = (|| -> std::io::Result<()> {
                        use std::io::Write;
                        if let Some(dir) = path.parent() {
                            std::fs::create_dir_all(dir)?;
                        }
                        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
                        writeln!(f, "{word}")
                    })();
                    if let Err(e) = result {
                        self.notify(NoticeSource::Language, Notice::Error(crate::i18n::tf("Could not save your dictionary: {}", &[&e]).into()));
                    } else {
                        self.notify(NoticeSource::Language, Notice::Info(crate::i18n::tf("Added “{}” to your dictionary.", &[&word]).into()));
                    }
                }
                self.repaint_editors(cx);
            }
            SpellChoice::IgnoreOnce => {
                if let Some(s) = &self.lsp.spell {
                    s.add_user_word(&word);
                }
                self.repaint_editors(cx);
            }
        }
        cx.notify();
    }

    fn repaint_editors(&mut self, cx: &mut Context<Self>) {
        for ed in self.all_editors() {
            ed.update(cx, |_, cx| cx.notify());
        }
    }

    /// "Spelling: Install Dictionary…": pick a language, download, add to the language list.
    pub(super) fn install_dictionary(&mut self, _: &InstallDictionary, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.spell_root() else { return };
        let have = spell::installed(&root);
        let items: Vec<PickItem> = spell::AVAILABLE
            .iter()
            .map(|a| PickItem {
                label: a.name.into(),
                detail: Some(
                    if have.iter().any(|h| h == a.id) {
                        crate::i18n::tf("installed · {}", &[&a.license])
                    } else {
                        crate::i18n::tf("about {0} MB · {1}", &[&a.megabytes, &a.license])
                    }
                    .into(),
                ),
                keys: None,
            })
            .collect();
        let picker = cx.new(|cx| {
            Picker::new(vec![PickSource::List { prefix: "", items }], crate::i18n::t("Install which dictionary? (English is built in)"), "", None, cx)
        });
        self.show_overlay(picker, OverlayKind::Dictionaries, window, cx);
    }

    pub(super) fn run_install_dictionary(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(a), Some(root)) = (spell::AVAILABLE.get(index), self.spell_root()) else { return };
        let (id, name) = (a.id, a.name);
        self.notify(NoticeSource::Language, Notice::Info(crate::i18n::tf("Downloading the {0} dictionary (about {1} MB)…", &[&name, &a.megabytes]).into()));
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
            spell::install(&spell::Curl, &root, id)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    let mut langs = cx.global::<Settings>().spell_languages.clone();
                    if !langs.iter().any(|l| l == id) {
                        langs.push(id.to_string());
                    }
                    this.write_setting("spelling.languages", serde_json::json!(langs), window, cx);
                    this.notify(NoticeSource::Language, Notice::Info(crate::i18n::tf("The {} dictionary is installed and on.", &[&name]).into()));
                    cx.notify();
                }
                Err(e) => {
                    let first = e.lines().next().unwrap_or(crate::i18n::t("download failed"));
                    this.notify(
                        NoticeSource::Language,
                        Notice::Error(
                            if first.contains("resolve") || first.contains("connect") || first.contains("curl") {
                                crate::i18n::tf("Could not install the {0} dictionary: {1}. Check your internet connection and try again.", &[&name, &first])
                            } else {
                                crate::i18n::tf("Could not install the {0} dictionary: {1}.", &[&name, &first])
                            }
                            .into(),
                        ),
                    );
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn toggle_spelling(&mut self, _: &ToggleSpelling, window: &mut Window, cx: &mut Context<Self>) {
        let on = cx.global::<Settings>().spelling;
        self.write_setting("spelling.enabled", serde_json::json!(!on), window, cx);
    }
}
