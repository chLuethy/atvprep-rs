//! Shared atvprep logic: models, policies, planning, compliance and patching.

/// The version of the Python distribution this port replaces (`atvprep/_version.py`).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod analyzers;
pub mod compliance;
pub mod console;
pub mod constants;
pub mod copy_policy;
pub mod duplicate_audio;
pub mod encode;
pub mod escalation;
pub mod fd_fix_contract;
pub mod ffprobe;
pub mod hook;
pub mod models;
pub mod mp4;
pub mod output;
pub mod phase_timing;
pub mod pipeline_postfix;
pub mod planner;
pub mod postfix;
pub mod prepare;
pub mod probe_helpers;
pub mod pyfmt;
pub mod pyjson;
pub mod sidecars;
pub mod stages;
pub mod track_policy;
pub mod tracklinks;
pub mod verify;
