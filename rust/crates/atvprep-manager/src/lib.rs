//! The manager's durable state: SQLite connection policy, schema, repositories and the job state
//! machine (port of `atvprep_manager/db/` and `jobs_state_machine.py`).

pub mod approval;
pub mod clock;
pub mod db;
pub mod deployment_gate;
pub mod error;
pub mod evaluate;
pub mod events;
pub mod inspections;
pub mod jobs;
pub mod leases;
pub mod library;
pub mod media;
pub mod models;
pub mod procid;
pub mod repo;
pub mod scans;
pub mod scheduler;
pub mod schedules;
pub mod schema;
pub mod settings;
pub mod sidecars;
pub mod snapshots;
pub mod state_machine;
pub mod worker;

pub use error::{Error, Result};
