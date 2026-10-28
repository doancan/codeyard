use super::*;

fn with_context(test: impl FnOnce(Context<'_>)) {
    let folders = BTreeMap::from([("client".into(), "/project/client".into()), ("root".into(), "/project".into())]);
    let environment = BTreeMap::from([("HOST_VALUE".into(), "remote-value".into())]);
    let config = BTreeMap::from([("editor.fontSize".into(), Value::from(14))]);
    let results = BTreeMap::from([("input:secret".into(), "şifre ${env:HOST_VALUE}; $(touch x)".into())]);
    test(Context {
        platform: Platform::Linux,
        root: "/project",
        folders: &folders,
        file: Some("/project/client/şablon.test.ts"),
        cursor: Some((3, 8)),
        selected: Some("selected"),
        cwd: "/caller",
        home: "/remote/home",
        executable: "/editor",
        default_build: Some("build"),
        environment: &environment,
        config: &config,
        results: &results,
    });
}

#[test]
fn task_variables_use_host_environment_and_expand_dynamic_results_only_once() {
    with_context(|cx| {
        assert_eq!(expand("${env:HOST_VALUE} ${input:secret} ${config:editor.fontSize}", &cx).unwrap(), "remote-value şifre ${env:HOST_VALUE}; $(touch x) 14");
        assert_eq!(expand("${env:UNSET}", &cx).unwrap(), "");
        assert_eq!(references("${input:secret} ${input:secret} ${env:HOST_VALUE}").unwrap(), BTreeSet::from(["env:HOST_VALUE", "input:secret"]));
        assert!(matches!(expand("${command:missing}", &cx), Err(Error::Missing(_))));
    });
}

#[test]
fn task_variables_resolve_named_roots_file_paths_positions_and_dotfiles() {
    with_context(|mut cx| {
        let fields = [
            ("workspaceFolder:client", "/project/client"),
            ("workspaceFolderBasename:client", "client"),
            ("fileWorkspaceFolder", "/project/client"),
            ("relativeFile", "client/şablon.test.ts"),
            ("fileBasenameNoExtension", "şablon.test"),
            ("fileExtname", ".ts"),
            ("relativeFileDirname", "client"),
            ("lineNumber", "4"),
            ("columnNumber", "9"),
        ];
        for (key, wanted) in fields {
            assert_eq!(cx.value(key).unwrap(), wanted, "{key}");
        }
        cx.file = Some("/project/.gitignore");
        assert_eq!(cx.value("fileExtname").unwrap(), "");
        assert_eq!(cx.value("fileBasenameNoExtension").unwrap(), ".gitignore");
        assert_eq!(cx.value("relativeFileDirname").unwrap(), ".");
        cx.file = None;
        assert_eq!(expand("${file}", &cx), Err(Error::MissingFile));
        assert_eq!(expand("${workspaceFolder}", &cx).unwrap(), "/project");
    });
}

#[test]
fn task_variables_keep_process_literals_shell_variables_and_linux_backslashes_intact() {
    with_context(|mut cx| {
        assert_eq!(expand("${HOME} $PATH $(command) şablon; & 'x'", &cx).unwrap(), "${HOME} $PATH $(command) şablon; & 'x'");
        cx.file = Some("/project/a\\b.rs");
        assert_eq!(cx.value("fileBasename").unwrap(), "a\\b.rs");
        cx.platform = Platform::Windows;
        cx.root = "C:\\Work Space";
        cx.file = Some("c:\\work space\\src\\test.rs");
        assert_eq!(cx.value("relativeFile").unwrap(), "src/test.rs");
        assert_eq!(cx.value("fileDirname").unwrap(), "c:/work space/src");
        assert_eq!(cx.value("/").unwrap(), "\\");
    });
}

#[test]
fn task_variables_reject_nested_missing_invalid_and_amplified_values() {
    let huge = "x".repeat(MAX_OUTPUT);
    with_context(|cx| {
        assert_eq!(expand("${input:${env:NAME}}", &cx), Err(Error::Nested));
        assert!(matches!(expand(&"${cwd}".repeat(257), &cx), Err(Error::Limit)));
        let mut cx = Context { cwd: &huge, ..cx };
        assert_eq!(expand("${cwd}${cwd}", &cx), Err(Error::Limit));
        cx.cwd = "a\0b";
        assert!(matches!(expand("${cwd}", &cx), Err(Error::Invalid(_))));
    });
}
