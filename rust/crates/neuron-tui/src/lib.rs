//! Neuron's full-screen terminal UI.
//!
//! Milestone M0 provides the contract between the UI and the agent engine ([`protocol`]),
//! a scripted engine for UI tests ([`fake_engine`]) and the guard that owns the terminal
//! and always gives it back ([`terminal`]). The engine side lives in the `neuron` binary
//! (`rusty-claude-cli`), which turns a turn's output into [`protocol::UiEvent`]s and takes
//! [`protocol::EngineCommand`]s.

pub mod fake_engine;
pub mod protocol;
pub mod terminal;
