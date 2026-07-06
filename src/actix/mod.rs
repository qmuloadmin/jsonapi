#![cfg(feature = "actixweb")]

pub mod accept;
pub mod extract;
pub mod ops;

pub use accept::NegotiateContentType;
pub use ops::{resource, ResourceScope};
