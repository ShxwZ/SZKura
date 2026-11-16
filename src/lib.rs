//! Domain and persistence modules for SZKura. No GUI dependencies required by callers.
pub mod booth;
pub mod db;
pub mod import;
pub mod managed_storage;
pub mod backup;
pub mod migration;
pub mod updater;

pub mod package_inspect;

pub mod quick_import;

pub mod share_transfer;
