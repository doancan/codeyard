//! Test support for crates that handle code extensions.

/// The smallest component that passes `check_component` with no permissions: it exports `codeyard:api/guest` and
/// imports nothing else. Kept current by `component_tests::the_fixture_is_current`.
pub const COMPONENT: &[u8] = include_bytes!("../testdata/guest.wasm");

/// The same component built against API 0.1.0, which Codeyard still runs during the move to 1.0 but registries refuse.
/// Kept current by the same test.
pub const COMPONENT_0_1: &[u8] = include_bytes!("../testdata/guest-0.1.wasm");
