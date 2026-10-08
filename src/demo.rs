use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::{
    CacheError, CheckpointStore, Outcome, Policy, SessionCache, SessionSpec, SessionView, Stats,
};

#[derive(Clone, Serialize)]
pub struct Task {
    pub spec: SessionSpec,
    pub prompt: String,
    pub response: String,
}

#[derive(Serialize)]
pub struct Turn {
    pub index: usize,
    pub task: Task,
    pub outcomes: [Outcome; 2],
}

#[derive(Serialize)]
pub struct PolicyView {
    pub policy: Policy,
    pub used_units: u32,
    pub stats: Stats,
    pub sessions: Vec<SessionView>,
}

#[derive(Serialize)]
pub struct Snapshot<'snapshot> {
    pub mode: &'static str,
    pub step: usize,
    pub total: usize,
    pub budget: u32,
    pub policies: [PolicyView; 2],
    pub current: Option<&'snapshot Turn>,
    pub history: &'snapshot [Turn],
    pub checkpoint_files: usize,
    pub checkpoint_bytes: u64,
}

pub struct Demo {
    caches: [SessionCache; 2],
    tasks: Vec<Task>,
    history: Vec<Turn>,
    budget: u32,
    directory: tempfile::TempDir,
}

impl Demo {
    pub fn new(data_path: &Path, budget: u32) -> Result<Self, CacheError> {
        if !(6..=12).contains(&budget) {
            return Err(CacheError::InvalidEstimate);
        }
        fs::create_dir_all(data_path)?;
        let directory = tempfile::Builder::new()
            .prefix("route-")
            .tempdir_in(data_path)?;
        let caches = [
            SessionCache::new(
                budget,
                Policy::Lru,
                CheckpointStore::new(&directory.path().join("lru"), "cache-cab-simulator-v1")?,
            )?,
            SessionCache::new(
                budget,
                Policy::RecomputeCost,
                CheckpointStore::new(
                    &directory.path().join("cost-aware"),
                    "cache-cab-simulator-v1",
                )?,
            )?,
        ];
        let main = SessionSpec::new("refactor", "Refactor admission", 5, 6_000)?;
        let helper = SessionSpec::new("explain", "Explain a symbol", 2, 400)?;
        let tests = SessionSpec::new("tests", "Write regression tests", 3, 900)?;
        let review = SessionSpec::new("review", "Review error paths", 3, 1_500)?;
        let tasks = [
            (main.clone(), "Refactor cache admission without losing session state.", "Split planning from admission. Preserve the existing state until checkpoint writes succeed."),
            (helper.clone(), "What does the SessionId type guarantee?", "SessionId restricts identifiers to a bounded ASCII alphabet. Checkpoint names use a fingerprint hash."),
            (tests.clone(), "Write a regression test for a full cache.", "Admit a helper after the main session, fill the budget, then resume the main session."),
            (main.clone(), "Continue the admission refactor.", "Resume the original conversation and retain its earlier invariants and decisions."),
            (helper.clone(), "Explain checkpoint integrity validation.", "Compare the checksum, session identity, format version, and runtime fingerprint before restoring."),
            (main.clone(), "Keep the refactor focused on admission.", "Use a planned victim list. A cache miss changes latency, not the required computation."),
            (review, "Review the error paths for invalid checkpoints.", "Reject corrupt state before evicting a resident session. Never reuse another runtime's snapshot."),
            (helper.clone(), "Which error means the request exceeds the budget?", "OverBudget carries the requested session, its estimated size, and the configured capacity."),
            (tests.clone(), "Extend the regression tests to cover restoration.", "After eviction, restore the checkpoint and verify that the previous turns are preserved."),
            (main.clone(), "Return to the main refactor and finish the plan.", "The expensive conversation resumes with its prior context intact."),
            (tests, "Verify both policies obey the same budget.", "Check resident units after every request; compare the policies using the identical route."),
            (main, "Summarize the change for a reviewed diff.", "Keep the scheduling policy separate from storage integrity and from the inference backend."),
        ].into_iter().map(|(spec, prompt, response)| Task { spec, prompt: prompt.to_owned(), response: response.to_owned() }).collect();
        Ok(Self {
            caches,
            tasks,
            history: Vec::new(),
            budget,
            directory,
        })
    }

    pub fn advance(&mut self) -> Result<(), CacheError> {
        let Some(task) = self.tasks.get(self.history.len()).cloned() else {
            return Ok(());
        };
        let outcomes = [
            self.caches[0].request(&task.spec, &task.prompt)?,
            self.caches[1].request(&task.spec, &task.prompt)?,
        ];
        self.history.push(Turn {
            index: self.history.len() + 1,
            task,
            outcomes,
        });
        Ok(())
    }

    pub fn snapshot(&self) -> Result<Snapshot<'_>, CacheError> {
        let mut checkpoint_files = 0;
        let mut checkpoint_bytes = 0;
        for name in ["lru", "cost-aware"] {
            for entry in fs::read_dir(self.directory.path().join(name))? {
                let entry = entry?;
                let metadata = entry.metadata()?;
                if metadata.is_file()
                    && entry
                        .path()
                        .extension()
                        .is_some_and(|extension| extension == "json")
                {
                    checkpoint_files += 1;
                    checkpoint_bytes += metadata.len();
                }
            }
        }
        let policies = std::array::from_fn(|index| PolicyView {
            policy: if index == 0 {
                Policy::Lru
            } else {
                Policy::RecomputeCost
            },
            used_units: self.caches[index].used_units(),
            stats: self.caches[index].stats().clone(),
            sessions: self.caches[index].sessions(),
        });
        Ok(Snapshot {
            mode: "deterministic_simulation",
            step: self.history.len(),
            total: self.tasks.len(),
            budget: self.budget,
            policies,
            current: self.history.last(),
            history: &self.history,
            checkpoint_files,
            checkpoint_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Access;

    #[test]
    fn identical_routes_produce_real_checkpoints_and_the_expected_policy_difference() {
        let directory = tempfile::tempdir().unwrap();
        let mut demo = Demo::new(directory.path(), 8).unwrap();
        for _ in 0..4 {
            demo.advance().unwrap();
        }
        let snapshot = demo.snapshot().unwrap();
        let turn = snapshot.current.unwrap();
        assert_eq!(turn.outcomes[0].access, Access::DiskRestore);
        assert_eq!(turn.outcomes[1].access, Access::RamHit);
        assert!(snapshot.checkpoint_files > 0);
        assert!(snapshot.checkpoint_bytes > 0);
        assert_eq!(
            snapshot.policies[0].stats.requests,
            snapshot.policies[1].stats.requests
        );
        while demo.history.len() < demo.tasks.len() {
            demo.advance().unwrap();
        }
        demo.advance().unwrap();
        let completed = demo.snapshot().unwrap();
        assert_eq!(completed.step, completed.total);
        assert!(completed.policies[1].stats.modeled_ms < completed.policies[0].stats.modeled_ms);
    }

    #[test]
    fn every_supported_budget_remains_bounded_and_all_requests_finish() {
        let directory = tempfile::tempdir().unwrap();
        for budget in 6..=12 {
            let mut demo = Demo::new(directory.path(), budget).unwrap();
            for _ in 0..12 {
                demo.advance().unwrap();
                for policy in demo.snapshot().unwrap().policies {
                    assert!(policy.used_units <= budget);
                }
            }
        }
    }
}
