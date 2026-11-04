//! Token budgets for sub-agents and multi-agent runs.
//!
//! A [`TokenBudget`] watches what one conversation runtime's model calls were billed
//! and, optionally, adds it to a [`RunMeter`] that several runtimes share (all the
//! agents of one Colony run). After each response it says whether to go on:
//!
//! - below [`WRAP_UP_PERCENT`] of every limit: go on;
//! - from there, once: tell the model to finish with what it has
//!   ([`WRAP_UP_NOTE`], added to the system prompt for the rest of the turn);
//! - at a limit, or when someone stopped the run: run no more tools and end the
//!   turn with what the model wrote so far.
//!
//! Tokens are the provider's billed counts: input, output and cache tokens.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use crate::usage::TokenUsage;

/// The share of a limit (in percent) at which the model is told to wrap up.
pub const WRAP_UP_PERCENT: u64 = 80;

/// Added to the system prompt once a budget is mostly spent.
pub const WRAP_UP_NOTE: &str = "Token budget note: most of the token budget for this task is used. Do not start new work. In your next reply, finish with what you have: summarise what you did and found, and list what is left undone. Call no more tools unless one last call is needed to finish.";

/// The result a tool call gets when the budget ran out before it could run.
pub const BUDGET_STOP_RESULT: &str =
    "Not run: the token budget for this task is used up, so the agent stopped here.";

/// Tokens used by every agent of one run, against the run's limit. Shared between
/// threads.
#[derive(Debug)]
pub struct RunMeter {
    limit: AtomicU64,
    used: AtomicU64,
    stopped: AtomicBool,
}

impl RunMeter {
    #[must_use]
    pub fn new(limit: u64) -> Self {
        Self {
            limit: AtomicU64::new(limit),
            used: AtomicU64::new(0),
            stopped: AtomicBool::new(false),
        }
    }

    /// Adds `tokens` and returns the new total.
    pub fn add(&self, tokens: u64) -> u64 {
        self.used.fetch_add(tokens, Ordering::SeqCst) + tokens
    }

    #[must_use]
    pub fn used(&self) -> u64 {
        self.used.load(Ordering::SeqCst)
    }

    #[must_use]
    pub fn limit(&self) -> u64 {
        self.limit.load(Ordering::SeqCst)
    }

    /// Raises (or lowers) the limit, for a user who chose to go on past it.
    pub fn set_limit(&self, limit: u64) {
        self.limit.store(limit, Ordering::SeqCst);
    }

    #[must_use]
    pub fn remaining(&self) -> u64 {
        self.limit().saturating_sub(self.used())
    }

    /// Whether the run reached [`WRAP_UP_PERCENT`] of its limit.
    #[must_use]
    pub fn near_limit(&self) -> bool {
        reached_percent(self.used(), self.limit(), WRAP_UP_PERCENT)
    }

    /// Whether the run reached its limit or was stopped.
    #[must_use]
    pub fn exhausted(&self) -> bool {
        self.is_stopped() || self.used() >= self.limit()
    }

    /// Stops every agent of the run after its current response.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }

    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
}

/// What a runtime does after a response, by its budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetVerdict {
    Proceed,
    /// Mostly spent: tell the model to finish (only the first time).
    WrapUp,
    /// Spent: run no more tools and end the turn.
    Stop,
}

/// One runtime's token budget, and the run it counts towards.
#[derive(Debug, Clone, Default)]
pub struct TokenBudget {
    limit: Option<u64>,
    run: Option<Arc<RunMeter>>,
    used: u64,
    warned: bool,
}

impl TokenBudget {
    /// A budget of `limit` tokens for this runtime (`None`: no limit of its own).
    #[must_use]
    pub fn new(limit: Option<u64>) -> Self {
        Self {
            limit,
            ..Self::default()
        }
    }

    /// Also counts towards (and stops at) `meter`'s limit.
    #[must_use]
    pub fn with_run_meter(mut self, meter: Arc<RunMeter>) -> Self {
        self.run = Some(meter);
        self
    }

    /// Tokens this runtime used so far.
    #[must_use]
    pub fn used(&self) -> u64 {
        self.used
    }

    #[must_use]
    pub fn limit(&self) -> Option<u64> {
        self.limit
    }

    /// Records a response's billed usage and says what to do next.
    pub fn record(&mut self, usage: TokenUsage) -> BudgetVerdict {
        let tokens = u64::from(usage.total_tokens());
        self.used += tokens;
        let run_total = self.run.as_ref().map(|meter| meter.add(tokens));
        self.verdict(run_total)
    }

    /// What to do now, without new usage (a run stopped by the user ends the agent
    /// too).
    #[must_use]
    pub fn check(&self) -> BudgetVerdict {
        if self.run.as_ref().is_some_and(|meter| meter.exhausted()) {
            BudgetVerdict::Stop
        } else {
            BudgetVerdict::Proceed
        }
    }

    fn verdict(&mut self, run_total: Option<u64>) -> BudgetVerdict {
        let own_spent = self.limit.is_some_and(|limit| self.used >= limit);
        let run_spent = self.run.as_ref().is_some_and(|meter| {
            meter.is_stopped() || run_total.unwrap_or_else(|| meter.used()) >= meter.limit()
        });
        if own_spent || run_spent {
            return BudgetVerdict::Stop;
        }
        let own_near = self
            .limit
            .is_some_and(|limit| reached_percent(self.used, limit, WRAP_UP_PERCENT));
        let run_near = self.run.as_ref().is_some_and(|meter| {
            reached_percent(
                run_total.unwrap_or_else(|| meter.used()),
                meter.limit(),
                WRAP_UP_PERCENT,
            )
        });
        if (own_near || run_near) && !self.warned {
            self.warned = true;
            return BudgetVerdict::WrapUp;
        }
        BudgetVerdict::Proceed
    }
}

fn reached_percent(used: u64, limit: u64, percent: u64) -> bool {
    used.saturating_mul(100) >= limit.saturating_mul(percent)
}

#[cfg(test)]
mod tests {
    use super::{BudgetVerdict, RunMeter, TokenBudget};
    use crate::usage::TokenUsage;
    use std::sync::Arc;

    fn usage(tokens: u32) -> TokenUsage {
        TokenUsage {
            input_tokens: tokens,
            ..TokenUsage::default()
        }
    }

    #[test]
    fn warns_once_near_the_limit_and_stops_at_it() {
        let mut budget = TokenBudget::new(Some(1_000));
        assert_eq!(budget.record(usage(500)), BudgetVerdict::Proceed);
        assert_eq!(budget.record(usage(300)), BudgetVerdict::WrapUp);
        assert_eq!(budget.record(usage(100)), BudgetVerdict::Proceed);
        assert_eq!(budget.record(usage(100)), BudgetVerdict::Stop);
        assert_eq!(budget.used(), 1_000);
    }

    #[test]
    fn agents_of_one_run_share_its_meter() {
        let meter = Arc::new(RunMeter::new(1_000));
        let mut first = TokenBudget::new(None).with_run_meter(Arc::clone(&meter));
        let mut second = TokenBudget::new(Some(10_000)).with_run_meter(Arc::clone(&meter));
        assert_eq!(first.record(usage(400)), BudgetVerdict::Proceed);
        assert_eq!(second.record(usage(400)), BudgetVerdict::WrapUp);
        assert!(meter.near_limit());
        assert_eq!(first.record(usage(250)), BudgetVerdict::Stop);
        assert_eq!(meter.used(), 1_050);
        assert!(meter.exhausted());
        assert_eq!(second.check(), BudgetVerdict::Stop);
    }

    #[test]
    fn a_stopped_run_stops_its_agents_and_a_raised_limit_lets_them_go_on() {
        let meter = Arc::new(RunMeter::new(1_000));
        let mut budget = TokenBudget::new(None).with_run_meter(Arc::clone(&meter));
        meter.stop();
        assert_eq!(budget.record(usage(1)), BudgetVerdict::Stop);

        let meter = Arc::new(RunMeter::new(100));
        let mut budget = TokenBudget::new(None).with_run_meter(Arc::clone(&meter));
        assert_eq!(budget.record(usage(100)), BudgetVerdict::Stop);
        meter.set_limit(1_000);
        assert_eq!(meter.remaining(), 900);
        assert_eq!(budget.record(usage(10)), BudgetVerdict::Proceed);
    }
}
