#![warn(clippy::pedantic)]

#[cfg(feature = "redb-backend")]
pub(crate) mod codec;
pub mod config;
pub mod crypto;
pub mod deserialize;
pub mod error;
pub mod format;
pub mod handle;
pub mod key;
pub mod merge;
pub mod migrate;
pub mod paths;
#[cfg(feature = "redb-backend")]
pub mod redb;
pub mod source;
pub mod value;
