# Codeyard technical documentation

These documents describe how the editor is built **today**. Update them in the same change as the
code they describe.

| Document | Read it to learn |
| --- | --- |
| [user-guide.md](user-guide.md) | every feature and key, by area (the in-app shortcuts reference is complete) |
| [settings.md](settings.md) | every setting, its values and default (generated from the code) |
| [troubleshooting.md](troubleshooting.md) | common problems (SSH, language servers, schemas, `.http`) and how to report one |
| [environment.md](environment.md) | every `CODEYARD_*` environment variable |
| [architecture.md](architecture.md) | crates, how the workbench is organised, stack choices, budgets and the gates that enforce them |
| [remote.md](remote.md) | SSH, the agent and its protocol, reconnecting, askpass, file transfer, `~/.ssh/config` editing |
| [containers.md](containers.md) | optional container module, Docker workflows and research on Apple Container, WSL Containers and Windows guests |
| [editor-services.md](editor-services.md) | schemas, spell check, merge editor, pull requests, `.http`, log follow: how each one works and where it stops |
| [design-tokens.md](design-tokens.md) | colour roles, themes and how tokens become Rust constants |
| [accessibility.md](accessibility.md) | the AccessKit tree on macOS, what is verified, what is missing |
| [platforms.md](platforms.md) | what runs and what has been verified on macOS, Linux and Windows |
| [testing.md](testing.md) | test layers, hermetic rules, fuzzing, hardening practice |
| [production-readiness.md](production-readiness.md) | readiness findings, implementation plan, validation evidence and platform limits |
| [ui-readiness.md](ui-readiness.md) | workbench resize, status bar, terminal, Git and resource panel review and validation |
| [translating.md](translating.md) | adding a language, the translation file format, plurals, quality bar, adding UI text |
| [releasing.md](releasing.md) | the manual release procedure |

`settings.md` is generated (`UPDATE_DOCS=1 cargo test -p app docs_reference`); `scripts/docs-check.sh` and the `docs_*` tests fail
when a document and the code disagree. The project overview is the root `README.md`; the change history is `CHANGELOG.md`.
