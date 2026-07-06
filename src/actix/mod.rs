#![cfg(feature = "actixweb")]

pub mod extract;
pub mod ops;

pub use ops::{resource, ResourceScope};
