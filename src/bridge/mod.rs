//! Cross-interpreter bridges.
//!
//! Both bridges solve the same class of problem — hand work across
//! an interpreter boundary — via a dedicated OS thread listening on
//! an MPSC channel:
//!
//! - `main_bridge`: sub-interpreter → main interpreter, for routes
//!   that must run on the main interp (C extensions, pydantic-core,
//!   numpy, etc. flagged with `gil=True`).
//! - `db_bridge`: application handler → dedicated DB pool thread,
//!   for database workloads that need a persistent connection pool
//!   without blocking request threads.
//!
//! Grouped here so the "bridge" pattern lives in one place even
//! though the two bridges serve different boundaries.

pub(crate) mod db_bridge;
pub(crate) mod main_bridge;
