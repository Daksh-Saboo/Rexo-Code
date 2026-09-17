//! Task planner (future work).
//!
//! For V1, the agent loop in [`crate::agent`] handles tasks directly:
//! it lets the model decide, turn by turn, which tool to call next. That
//! works well for most edits and investigations, but larger tasks
//! ("add OAuth login") benefit from an explicit upfront plan the user can
//! review before any files change.
//!
//! This module is the intended home for that: given a task description and
//! project context, produce a short ordered [`Plan`] of steps, optionally
//! shown to the user for approval before the agent loop executes it one
//! step at a time. Not wired into `main.rs` yet — see the project roadmap
//! in the README.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanStep {
    pub description: String,
}

#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub steps: Vec<PlanStep>,
}

// Not wired into the agent loop yet (see module docs / README roadmap) —
// intentionally unused scaffolding rather than dead leftover code.
#[allow(dead_code)]
impl Plan {
    pub fn new(steps: Vec<PlanStep>) -> Self {
        Self { steps }
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    pub fn render(&self) -> String {
        self.steps
            .iter()
            .enumerate()
            .map(|(i, step)| format!("{}. {}", i + 1, step.description))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_numbered_steps() {
        let plan = Plan::new(vec![
            PlanStep { description: "Inspect project".to_string() },
            PlanStep { description: "Locate auth module".to_string() },
        ]);
        assert_eq!(plan.render(), "1. Inspect project\n2. Locate auth module");
    }
}
