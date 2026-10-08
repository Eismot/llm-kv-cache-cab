#![forbid(unsafe_code)]

pub mod coding;
pub mod demo;
pub mod live;
pub mod llama;
pub mod repo_task;

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CacheError {
    #[error("session IDs must contain 1-40 lowercase ASCII letters, digits, or hyphens")]
    InvalidSessionId,
    #[error("budget, estimated size, and recomputation cost must be positive")]
    InvalidEstimate,
    #[error("session {session} requires {required} units, exceeding the {budget}-unit budget")]
    OverBudget {
        session: SessionId,
        required: u32,
        budget: u32,
    },
    #[error("session {0} was registered with different estimates")]
    IncompatibleSession(SessionId),
    #[error("checkpoint is corrupt or belongs to a different model/session")]
    InvalidCheckpoint,
    #[error("checkpoint I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("checkpoint JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("checkpoint persistence: {0}")]
    Persist(#[from] tempfile::PersistError),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct SessionId(String);

impl TryFrom<String> for SessionId {
    type Error = CacheError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty()
            || value.len() > 40
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(CacheError::InvalidSessionId);
        }
        Ok(Self(value))
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SessionSpec {
    pub id: SessionId,
    pub label: String,
    pub estimated_units: u32,
    pub rebuild_ms: u32,
}

impl SessionSpec {
    pub fn new(
        id: &str,
        label: &str,
        estimated_units: u32,
        rebuild_ms: u32,
    ) -> Result<Self, CacheError> {
        if estimated_units == 0 || rebuild_ms == 0 {
            return Err(CacheError::InvalidEstimate);
        }
        Ok(Self {
            id: id.to_owned().try_into()?,
            label: label.to_owned(),
            estimated_units,
            rebuild_ms,
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Policy {
    Lru,
    RecomputeCost,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Residency {
    Ram,
    Disk,
}

#[derive(Clone, Debug, Serialize)]
pub struct SessionView {
    pub spec: SessionSpec,
    pub residency: Residency,
    pub last_used: u64,
    pub checkpoint_bytes: u64,
    pub turns: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Payload {
    version: u8,
    fingerprint: String,
    session: SessionId,
    turns: Vec<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    payload: Payload,
    checksum: String,
}

pub struct CheckpointStore {
    directory: PathBuf,
    fingerprint: String,
}

impl CheckpointStore {
    pub fn new(directory: &Path, fingerprint: &str) -> Result<Self, CacheError> {
        if fingerprint.is_empty() {
            return Err(CacheError::InvalidCheckpoint);
        }
        fs::create_dir_all(directory)?;
        Ok(Self {
            directory: directory.to_owned(),
            fingerprint: fingerprint.to_owned(),
        })
    }

    fn filename(&self, session: &SessionId) -> PathBuf {
        let hash = Sha256::digest(format!("{}:{session}", self.fingerprint));
        self.directory.join(format!("{hash:x}.json"))
    }

    fn save(&self, payload: &Payload) -> Result<u64, CacheError> {
        let encoded = serde_json::to_vec(payload)?;
        let checkpoint = Checkpoint {
            payload: payload.clone(),
            checksum: format!("{:x}", Sha256::digest(encoded)),
        };
        let bytes = serde_json::to_vec(&checkpoint)?;
        let mut temporary = tempfile::NamedTempFile::new_in(&self.directory)?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary.persist(self.filename(&payload.session))?;
        Ok(bytes.len() as u64)
    }

    fn load(&self, session: &SessionId) -> Result<Payload, CacheError> {
        let file = fs::File::open(self.filename(session))?;
        if file.metadata()?.len() > 1_048_576 {
            return Err(CacheError::InvalidCheckpoint);
        }
        let checkpoint: Checkpoint = serde_json::from_reader(file)?;
        let expected = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&checkpoint.payload)?)
        );
        if checkpoint.checksum != expected
            || checkpoint.payload.version != 1
            || checkpoint.payload.fingerprint != self.fingerprint
            || checkpoint.payload.session != *session
        {
            return Err(CacheError::InvalidCheckpoint);
        }
        Ok(checkpoint.payload)
    }
}

struct Session {
    view: SessionView,
    payload: Option<Payload>,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    Cold,
    RamHit,
    DiskRestore,
}

#[derive(Debug, Serialize)]
pub struct Outcome {
    pub session: SessionId,
    pub access: Access,
    pub evicted: Vec<SessionId>,
    pub modeled_ms: u32,
    pub avoided_rebuild_ms: u32,
}

#[derive(Default, Debug, Clone, Serialize)]
pub struct Stats {
    pub requests: u32,
    pub cold_starts: u32,
    pub ram_hits: u32,
    pub disk_restores: u32,
    pub evictions: u32,
    pub modeled_ms: u64,
    pub avoided_rebuild_ms: u64,
}

pub struct SessionCache {
    budget: u32,
    policy: Policy,
    store: CheckpointStore,
    sessions: BTreeMap<SessionId, Session>,
    clock: u64,
    stats: Stats,
}

impl SessionCache {
    pub fn new(budget: u32, policy: Policy, store: CheckpointStore) -> Result<Self, CacheError> {
        if budget == 0 {
            return Err(CacheError::InvalidEstimate);
        }
        Ok(Self {
            budget,
            policy,
            store,
            sessions: BTreeMap::new(),
            clock: 0,
            stats: Stats::default(),
        })
    }

    pub fn used_units(&self) -> u32 {
        self.sessions
            .values()
            .filter(|session| session.view.residency == Residency::Ram)
            .map(|session| session.view.spec.estimated_units)
            .sum()
    }

    pub fn sessions(&self) -> Vec<SessionView> {
        self.sessions
            .values()
            .map(|session| session.view.clone())
            .collect()
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    pub fn request(&mut self, spec: &SessionSpec, turn: &str) -> Result<Outcome, CacheError> {
        if spec.estimated_units == 0 || spec.rebuild_ms == 0 {
            return Err(CacheError::InvalidEstimate);
        }
        if spec.estimated_units > self.budget {
            return Err(CacheError::OverBudget {
                session: spec.id.clone(),
                required: spec.estimated_units,
                budget: self.budget,
            });
        }
        let existing = self.sessions.get(&spec.id);
        if existing.is_some_and(|session| session.view.spec != *spec) {
            return Err(CacheError::IncompatibleSession(spec.id.clone()));
        }
        let (access, mut payload) = match existing {
            Some(session) if session.view.residency == Residency::Ram => (
                Access::RamHit,
                session
                    .payload
                    .clone()
                    .ok_or(CacheError::InvalidCheckpoint)?,
            ),
            Some(_) => (Access::DiskRestore, self.store.load(&spec.id)?),
            None => (
                Access::Cold,
                Payload {
                    version: 1,
                    fingerprint: self.store.fingerprint.clone(),
                    session: spec.id.clone(),
                    turns: Vec::new(),
                },
            ),
        };
        let additional = if access == Access::RamHit {
            0
        } else {
            spec.estimated_units
        };
        let mut victims: Vec<_> = self
            .sessions
            .values()
            .filter(|session| {
                session.view.residency == Residency::Ram && session.view.spec.id != spec.id
            })
            .map(|session| session.view.clone())
            .collect();
        victims.sort_by(|left, right| match self.policy {
            Policy::Lru => left.last_used.cmp(&right.last_used),
            Policy::RecomputeCost => (u64::from(left.spec.rebuild_ms)
                * u64::from(right.spec.estimated_units))
            .cmp(&(u64::from(right.spec.rebuild_ms) * u64::from(left.spec.estimated_units)))
            .then_with(|| left.last_used.cmp(&right.last_used)),
        });
        let mut evicted = Vec::new();
        let mut projected = u64::from(self.used_units()) + u64::from(additional);
        let mut checkpoints = Vec::new();
        for victim in victims {
            if projected <= u64::from(self.budget) {
                break;
            }
            let session = self
                .sessions
                .get(&victim.spec.id)
                .ok_or(CacheError::InvalidCheckpoint)?;
            let bytes = self.store.save(
                session
                    .payload
                    .as_ref()
                    .ok_or(CacheError::InvalidCheckpoint)?,
            )?;
            projected -= u64::from(victim.spec.estimated_units);
            checkpoints.push((victim.spec.id, bytes));
        }
        for (id, bytes) in checkpoints {
            let session = self
                .sessions
                .get_mut(&id)
                .ok_or(CacheError::InvalidCheckpoint)?;
            session.view.checkpoint_bytes = bytes;
            session.view.residency = Residency::Disk;
            session.payload = None;
            evicted.push(id);
        }
        self.clock += 1;
        payload.turns.push(turn.to_owned());
        let view = SessionView {
            spec: spec.clone(),
            residency: Residency::Ram,
            last_used: self.clock,
            checkpoint_bytes: existing_bytes(&self.sessions, &spec.id),
            turns: payload.turns.len(),
        };
        self.sessions.insert(
            spec.id.clone(),
            Session {
                view,
                payload: Some(payload),
            },
        );
        let modeled_ms = match access {
            Access::Cold => spec.rebuild_ms,
            Access::RamHit => 20,
            Access::DiskRestore => 180,
        };
        let avoided = spec.rebuild_ms.saturating_sub(modeled_ms);
        self.stats.requests += 1;
        self.stats.evictions += evicted.len() as u32;
        self.stats.modeled_ms += u64::from(modeled_ms);
        self.stats.avoided_rebuild_ms += u64::from(avoided);
        match access {
            Access::Cold => self.stats.cold_starts += 1,
            Access::RamHit => self.stats.ram_hits += 1,
            Access::DiskRestore => self.stats.disk_restores += 1,
        }
        Ok(Outcome {
            session: spec.id.clone(),
            access,
            evicted,
            modeled_ms,
            avoided_rebuild_ms: avoided,
        })
    }
}

fn existing_bytes(sessions: &BTreeMap<SessionId, Session>, id: &SessionId) -> u64 {
    sessions
        .get(id)
        .map_or(0, |session| session.view.checkpoint_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache(directory: &Path, policy: Policy) -> SessionCache {
        SessionCache::new(
            8,
            policy,
            CheckpointStore::new(directory, "demo-v1").unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn cost_policy_keeps_the_expensive_conversation_where_lru_evicts_it() {
        let main = SessionSpec::new("refactor", "Refactor", 5, 6_000).unwrap();
        let helper = SessionSpec::new("helper", "Helper", 2, 400).unwrap();
        let tests = SessionSpec::new("tests", "Tests", 3, 900).unwrap();
        for policy in [Policy::Lru, Policy::RecomputeCost] {
            let directory = tempfile::tempdir().unwrap();
            let mut cache = cache(directory.path(), policy);
            cache.request(&main, "first").unwrap();
            cache.request(&helper, "helper").unwrap();
            let admission = cache.request(&tests, "tests").unwrap();
            let expected = if policy == Policy::Lru {
                &main.id
            } else {
                &helper.id
            };
            assert_eq!(admission.evicted, [expected.clone()]);
            assert!(cache.used_units() <= 8);
            let resumed = cache.request(&main, "second").unwrap();
            assert_eq!(
                resumed.access,
                if policy == Policy::Lru {
                    Access::DiskRestore
                } else {
                    Access::RamHit
                }
            );
            assert_eq!(
                cache
                    .sessions()
                    .iter()
                    .find(|session| session.spec.id == main.id)
                    .unwrap()
                    .turns,
                2
            );
        }
    }

    #[test]
    fn a_failed_checkpoint_write_does_not_partially_evict_resident_sessions() {
        let directory = tempfile::tempdir().unwrap();
        let mut cache = cache(directory.path(), Policy::Lru);
        let first = SessionSpec::new("first", "First", 3, 400).unwrap();
        let second = SessionSpec::new("second", "Second", 3, 500).unwrap();
        cache.request(&first, "first").unwrap();
        cache.request(&second, "second").unwrap();
        fs::create_dir(cache.store.filename(&second.id)).unwrap();
        let incoming = SessionSpec::new("incoming", "Incoming", 6, 1_000).unwrap();
        assert!(cache.request(&incoming, "incoming").is_err());
        assert_eq!(cache.used_units(), 6);
        assert_eq!(cache.stats().requests, 2);
        assert_eq!(cache.stats().evictions, 0);
        assert!(cache
            .sessions()
            .iter()
            .all(|session| session.residency == Residency::Ram));
    }

    #[test]
    fn over_budget_requests_do_not_evict_or_mutate_existing_sessions() {
        let directory = tempfile::tempdir().unwrap();
        let mut cache = cache(directory.path(), Policy::Lru);
        let session = SessionSpec::new("main", "Main", 5, 1_000).unwrap();
        cache.request(&session, "first").unwrap();
        let large = SessionSpec::new("large", "Large", 9, 5_000).unwrap();
        assert!(matches!(
            cache.request(&large, "no"),
            Err(CacheError::OverBudget { .. })
        ));
        assert_eq!(cache.used_units(), 5);
        assert_eq!(cache.stats().requests, 1);
    }

    #[test]
    fn corrupt_checkpoints_fail_without_restoring_invalid_state() {
        let directory = tempfile::tempdir().unwrap();
        let mut cache = cache(directory.path(), Policy::Lru);
        let main = SessionSpec::new("main", "Main", 5, 1_000).unwrap();
        let other = SessionSpec::new("other", "Other", 5, 1_000).unwrap();
        cache.request(&main, "original").unwrap();
        cache.request(&other, "other").unwrap();
        fs::write(cache.store.filename(&main.id), b"{} ").unwrap();
        assert!(cache.request(&main, "resume").is_err());
        assert_eq!(cache.stats().requests, 2);
        assert_eq!(
            cache
                .sessions()
                .iter()
                .find(|session| session.spec.id == main.id)
                .unwrap()
                .residency,
            Residency::Disk
        );
    }

    #[test]
    fn session_ids_and_estimates_are_validated() {
        for id in ["", "../escape", "A", "hello world", "one/two"] {
            assert!(SessionSpec::new(id, "Invalid", 1, 1).is_err());
        }
        assert!(SessionSpec::new("valid", "Invalid", 0, 1).is_err());
        assert!(SessionSpec::new("valid", "Invalid", 1, 0).is_err());
        assert!(serde_json::from_str::<SessionId>("\"../escape\"").is_err());
    }

    #[test]
    fn changed_configuration_cannot_restore_another_checkpoint() {
        let directory = tempfile::tempdir().unwrap();
        let first = CheckpointStore::new(directory.path(), "model-a").unwrap();
        let second = CheckpointStore::new(directory.path(), "model-b").unwrap();
        let id = SessionId::try_from("main".to_owned()).unwrap();
        let payload = Payload {
            version: 1,
            fingerprint: "model-a".to_owned(),
            session: id.clone(),
            turns: vec!["original".to_owned()],
        };
        first.save(&payload).unwrap();
        fs::copy(first.filename(&id), second.filename(&id)).unwrap();
        assert!(matches!(
            second.load(&id),
            Err(CacheError::InvalidCheckpoint)
        ));
    }
}
