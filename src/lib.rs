//! bilro: context economy for coding agents.
//!
//! The layers point one way. `surface` is reached from outside and calls
//! `report`, which reads what `compress` and `run` produced, which rest on
//! `store`. `redact` sits under all of them and depends on nothing.

pub mod compress;
pub mod proc;
pub mod redact;
pub mod report;
pub mod run;
pub mod store;
pub mod surface;
