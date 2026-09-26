//! Real background jobs — the actual, shipped `/background` (planned
//! since v0.6, stubbed with `not_yet()` through v0.8) plus the new
//! non-blocking `/agents run`.
//!
//! Each job is a genuine `tokio::spawn`ed task built on
//! [`super::subagent::run_silent`], sharing status/output back through
//! this registry (`Arc<Mutex<Vec<JobEntry>>>`) exactly the way
//! `tools::tasks::TaskList` shares state with Ctrl+T's panel: the TUI
//! polls [`BackgroundJobs::snapshot`] on every `sync_header` and renders
//! whatever it sees, never touching the live jobs directly.
//!
//! **What's real**: the model calls happen on independent tokio tasks
//! that keep running while the interactive session does something else
//! — type a new prompt, run a command, start another background job.
//! Cooperative cancellation (checked between agent-loop iterations, see
//! `run_silent`) actually stops a job that hasn't sent its current
//! request yet.
//!
//! **What's not** (see the README's Known Limitations): a job already
//! mid-HTTP-request finishes that one request before noticing a cancel
//! flag — there's no way to abort an in-flight `reqwest` call
//! mid-stream without dropping the whole future, which would leave the
//! provider's connection in an unknown state; and every job shares the
//! one workspace on disk with the parent session and every other job —
//! there's no git-worktree isolation (still the one clear item carried
//! forward on the roadmap), so two jobs editing overlapping files race
//! each other exactly like two humans editing the same repo at once
//! would. `execute_tool_guarded`'s panic containment still applies
//! per-tool-call inside a background job, same as the foreground loop.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::config::Config;

use super::subagent::SubagentPersona;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Working,
    Done,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn label(&self) -> &'static str {
        match self {
            JobStatus::Working => "working",
            JobStatus::Done => "completed",
            JobStatus::Failed => "failed",
            JobStatus::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone)]
pub struct JobSnapshot {
    pub id: u32,
    /// The persona name for `/agents run --background`, or "background"
    /// for a plain `/background <task>`.
    pub label: String,
    pub task: String,
    pub status: JobStatus,
    pub output: Option<String>,
    pub started_at: Instant,
    pub finished_at: Option<Instant>,
}

struct JobEntry {
    snapshot: JobSnapshot,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct BackgroundJobs {
    jobs: Mutex<Vec<JobEntry>>,
    next_id: AtomicU32,
}

pub type SharedBackgroundJobs = Arc<BackgroundJobs>;

impl BackgroundJobs {
    /// A snapshot of every job, most recently started first — what the
    /// panel actually renders. Cheap: a handful of small clones at most,
    /// same tradeoff `tools::tasks::TaskList`'s own snapshot makes.
    pub fn snapshot(&self) -> Vec<JobSnapshot> {
        let mut items: Vec<JobSnapshot> = self.jobs.lock().map(|j| j.iter().map(|e| e.snapshot.clone()).collect()).unwrap_or_default();
        items.sort_by(|a, b| b.id.cmp(&a.id));
        items
    }

    pub fn count_working(&self) -> usize {
        self.jobs.lock().map(|j| j.iter().filter(|e| e.snapshot.status == JobStatus::Working).count()).unwrap_or(0)
    }

    /// Start a job right now and register it before returning — a
    /// `snapshot()` immediately after this call always sees it, so there
    /// is no race where the panel briefly shows nothing right after
    /// `/background` runs.
    pub fn spawn(self: &Arc<Self>, config: Config, label: String, task: String, persona: Option<SubagentPersona>) -> u32 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let cancel = Arc::new(AtomicBool::new(false));
        let entry = JobEntry {
            snapshot: JobSnapshot {
                id,
                label,
                task: task.clone(),
                status: JobStatus::Working,
                output: None,
                started_at: Instant::now(),
                finished_at: None,
            },
            cancel: cancel.clone(),
        };
        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.push(entry);
        }

        let registry = self.clone();
        tokio::spawn(async move {
            let result = super::subagent::run_silent(&config, persona.as_ref(), &task, cancel.clone()).await;
            registry.finish(id, result, cancel.load(Ordering::Relaxed));
        });
        id
    }

    fn finish(&self, id: u32, result: anyhow::Result<String>, was_cancelled: bool) {
        if let Ok(mut jobs) = self.jobs.lock() {
            if let Some(entry) = jobs.iter_mut().find(|e| e.snapshot.id == id) {
                match result {
                    Ok(output) if was_cancelled => {
                        entry.snapshot.status = JobStatus::Cancelled;
                        entry.snapshot.output = Some(output);
                    }
                    Ok(output) => {
                        entry.snapshot.status = JobStatus::Done;
                        entry.snapshot.output = Some(output);
                    }
                    Err(e) => {
                        entry.snapshot.status = JobStatus::Failed;
                        entry.snapshot.output = Some(e.to_string());
                    }
                }
                entry.snapshot.finished_at = Some(Instant::now());
            }
        }
    }

    /// Request cancellation. The job notices at its next agent-loop
    /// iteration boundary — not instantly if a request is already
    /// in-flight (see this module's docs). Returns `false` if the job
    /// isn't currently running (already finished, or doesn't exist).
    pub fn cancel(&self, id: u32) -> bool {
        let jobs = self.jobs.lock().unwrap();
        match jobs.iter().find(|e| e.snapshot.id == id) {
            Some(e) if e.snapshot.status == JobStatus::Working => {
                e.cancel.store(true, Ordering::Relaxed);
                true
            }
            _ => false,
        }
    }

    /// Remove a *finished* job from the list. Refuses to remove one
    /// still running (cancel it first) so the panel can't lose track of
    /// a job that's still actually executing in the background.
    pub fn remove(&self, id: u32) -> bool {
        let mut jobs = self.jobs.lock().unwrap();
        let before = jobs.len();
        jobs.retain(|e| !(e.snapshot.id == id && e.snapshot.status != JobStatus::Working));
        jobs.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        let dir = std::env::temp_dir().join(format!("rexo_test_bg_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("rexo.toml"), "[model]\nprovider = \"local\"\nmodel = \"test-model\"\nbase_url = \"http://127.0.0.1:1\"\n").unwrap();
        Config::load(&dir).unwrap()
    }

    #[tokio::test]
    async fn spawn_registers_the_job_before_returning() {
        let registry: SharedBackgroundJobs = Arc::new(BackgroundJobs::default());
        let id = registry.spawn(test_config(), "background".to_string(), "say hi".to_string(), None);
        let snap = registry.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].id, id);
    }

    #[tokio::test]
    async fn a_job_against_an_unreachable_provider_ends_up_failed() {
        // `test_config` points at 127.0.0.1:1 (nothing listening) so the
        // job's HTTP request fails fast — this is the real end-to-end
        // path (spawn → run_silent → provider.chat → network error →
        // finish()), not a mocked shortcut.
        let registry: SharedBackgroundJobs = Arc::new(BackgroundJobs::default());
        let id = registry.spawn(test_config(), "background".to_string(), "say hi".to_string(), None);

        let mut attempts = 0;
        loop {
            let snap = registry.snapshot();
            let job = snap.iter().find(|j| j.id == id).unwrap();
            if job.status != JobStatus::Working || attempts > 50 {
                assert_eq!(job.status, JobStatus::Failed, "expected the unreachable provider to fail the job");
                assert!(job.output.is_some());
                break;
            }
            attempts += 1;
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn cancel_refuses_a_job_that_is_not_running() {
        let registry: SharedBackgroundJobs = Arc::new(BackgroundJobs::default());
        assert!(!registry.cancel(999));
    }

    #[tokio::test]
    async fn remove_refuses_a_still_running_job() {
        let registry: SharedBackgroundJobs = Arc::new(BackgroundJobs::default());
        let id = registry.spawn(test_config(), "background".to_string(), "say hi".to_string(), None);
        // It may already have failed by the time we get here (fast local
        // network error) — only assert the invariant that matters: a
        // job still `Working` can't be removed out from under itself.
        let still_working = registry.snapshot().iter().any(|j| j.id == id && j.status == JobStatus::Working);
        if still_working {
            assert!(!registry.remove(id));
        }
    }
}
