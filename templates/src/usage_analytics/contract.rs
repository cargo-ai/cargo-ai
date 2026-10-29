//! Shared event contract, with historical custody fixtures available to tests.
#[path = "events.rs"]
mod events;
pub use events::*;
#[cfg(test)]
#[path = "custody.rs"]
mod custody;
#[cfg(test)]
pub use custody::*;
