//! Test support for crates that handle code extensions.

/// The smallest component that passes `check_component` with no permissions: it exports `codeyard:api/guest` and
/// imports nothing else. Kept current by `component_tests::the_fixture_is_current`.
pub const COMPONENT: &[u8] = include_bytes!("../testdata/guest.wasm");
