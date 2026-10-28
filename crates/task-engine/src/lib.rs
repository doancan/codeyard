//! Task planning and state transitions, independent of windows and process transport.
//!
//! Parsing and planning never execute a command or request an input. The runtime owns
//! process handles; the scheduler returns explicit start/stop reservations.

pub mod matchers;
pub mod model;
pub mod schedule;
pub mod variables;
