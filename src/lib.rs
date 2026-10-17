//! ASD-STE100 lint core, shared by the hook binary and the
//! `rushi-ste` standalone checker.
//!
//! The lib target owns the rule engine, the sentence segmenter, the
//! dictionaries, the config loader, and the path classifier. The two
//! binaries stay thin: the hook binary drives the harness windows,
//! and `rushi-ste` is a plain stdin / file CLI.

pub mod classify;
pub mod config;
pub mod dictionaries;
pub mod engine;
pub mod feedback;
pub mod rules;
pub mod sentences;
pub mod types;
