use super::*;

#[test]
fn managed_service_configuration_keeps_html_css_diagnostics_enabled_and_preserves_user_options() {
    let defaults = Service::Html.configuration(&Value::Null);
    assert_eq!(defaults["css"], serde_json::json!({}));
    assert_eq!(defaults["html"], serde_json::json!({}));
    let user = serde_json::json!({"html":{"validate":{"styles":false}},"css":{"lint":{"unknownProperties":"ignore"}},"extra":{"value":7}});
    let configured = Service::Html.configuration(&user);
    assert_eq!(configured["html"], user["html"]);
    assert_eq!(configured["css"], user["css"]);
    assert_eq!(configured["extra"], user["extra"]);
    assert_eq!(Service::Css.configuration(&serde_json::json!({"css":null}))["css"], serde_json::json!({}));
    assert_eq!(Service::TypeScript.configuration(&Value::Null), Value::Null);
}

#[test]
fn managed_service_catalog_pins_every_package_and_its_integrity() {
    let catalog = Catalog::bundled().unwrap();
    assert_eq!(catalog.versions["typescript"], "6.0.3");
    assert_eq!(catalog.versions["typescript-language-server"], "6.0.2");
    assert!(catalog.packages > 3 && catalog.packages < MAX_PACKAGES);
    assert_eq!(catalog.fingerprint.len(), 64);
}

#[test]
fn managed_service_catalog_rejects_untrusted_sources_links_and_missing_integrity() {
    let original: Value = serde_json::from_str(PACKAGE_LOCK).unwrap();
    for (field, value, expected) in [
        ("resolved", Value::String("https://evil.invalid/package.tgz".into()), CatalogError::UnsafePackage),
        ("resolved", Value::String("https://registry.npmjs.org.evil.invalid/package.tgz".into()), CatalogError::UnsafePackage),
        ("resolved", Value::String("file:/workspace/package.tgz".into()), CatalogError::UnsafePackage),
        ("resolved", Value::String("https://registry.npmjs.org/a\n/evil.tgz".into()), CatalogError::UnsafePackage),
        ("resolved", Value::String("https://registry.npmjs.org/a/../../evil.tgz".into()), CatalogError::UnsafePackage),
        ("resolved", Value::String("https://registry.npmjs.org/a//evil.tgz".into()), CatalogError::UnsafePackage),
        ("integrity", Value::String(format!("sha512-{}==", "é".repeat(43))), CatalogError::IntegrityMissing),
        ("integrity", Value::Null, CatalogError::IntegrityMissing),
        ("link", Value::Bool(true), CatalogError::UnsafePackage),
    ] {
        let mut lock = original.clone();
        lock["packages"]["node_modules/typescript-language-server"][field] = value;
        assert_eq!(Catalog::parse(PACKAGE_JSON, &lock.to_string()), Err(expected));
    }
}

#[test]
fn managed_service_catalog_accepts_only_exact_semver_versions() {
    for value in ["0.0.0", "1.2.3", "1.2.3-beta.1", "1.2.3+build.001", "1.2.3-rc.1+sha.0123"] {
        assert!(exact_version(value), "{value}");
    }
    for value in ["1.2", "^1.2.3", "01.2.3", "1.2.3-", "1.2.3+", "1.2.3-a..b", "1.2.3-01", "1.2.3+a+b", "1.2.3-ş"] {
        assert!(!exact_version(value), "{value}");
    }
}

#[test]
fn managed_service_catalog_rejects_unpinned_or_changed_root_dependencies() {
    assert_eq!(Catalog::parse(&PACKAGE_JSON.replace("6.0.3", "^6.0.3"), PACKAGE_LOCK), Err(CatalogError::InvalidManifest));
    assert_eq!(Catalog::parse(&PACKAGE_JSON.replace("6.0.3", "6.0.2"), PACKAGE_LOCK), Err(CatalogError::DependencyMismatch));
    assert_eq!(Catalog::parse(PACKAGE_JSON, &" ".repeat(MAX_LOCK + 1)), Err(CatalogError::InvalidLock));
}

#[test]
fn managed_service_node_runtime_checks_the_actual_minimum_version() {
    for value in ["v22.22.2", "22.23.3", "v24.0.0\n", "v26.3.1"] {
        assert!(supported_node(value), "{value}");
    }
    for value in ["v22.22.1", "v22.21.99", "v20.99.99", "v26.0.0-rc.1", "v99.0.0+custom", "version 24", "24.1", "v999999999999999.0.0"] {
        assert!(!supported_node(value), "{value}");
    }
}
