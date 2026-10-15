#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

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
pub mod source;
#[cfg(feature = "sqlite-backend")]
pub mod sqlite;
pub mod value;
