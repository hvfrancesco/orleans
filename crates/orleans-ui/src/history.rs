//! Persistent history of streamed cut and framing jobs.
//!
//! The history is deliberately separate from project files: it records what
//! was sent to the machine, regardless of which design or project is open.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const FORMAT: u32 = 1;
pub const MAX_JOBS: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobKind {
    Cut,
    Frame,
}

impl JobKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cut => "cut",
            Self::Frame => "frame",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobOutcome {
    Completed,
    Stopped,
    Failed,
}

impl JobOutcome {
    pub fn label(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobRecord {
    /// Unix seconds when streaming started.
    pub started: u64,
    /// Unix seconds when the job ended.
    pub finished: u64,
    pub label: String,
    pub kind: JobKind,
    pub outcome: JobOutcome,
    pub cuts: usize,
    pub layers: usize,
    pub cut_mm: f64,
    pub travel_mm: f64,
    pub estimated_secs: f64,
    pub actual_secs: f64,
    pub lines_done: usize,
    pub lines_total: usize,
    pub source: Option<String>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobHistory {
    #[serde(default = "default_format")]
    pub format: u32,
    #[serde(default)]
    pub jobs: Vec<JobRecord>,
}

fn default_format() -> u32 {
    FORMAT
}

impl Default for JobHistory {
    fn default() -> Self {
        Self {
            format: FORMAT,
            jobs: Vec::new(),
        }
    }
}

impl JobHistory {
    pub fn load(path: &Path) -> Self {
        let Ok(text) = fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(history) = serde_json::from_str::<Self>(&text) else {
            return Self::default();
        };
        if history.format != FORMAT {
            return Self::default();
        }
        history
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        fs::write(path, json)
    }

    pub fn push(&mut self, record: JobRecord) {
        self.jobs.push(record);
        if self.jobs.len() > MAX_JOBS {
            self.jobs.drain(0..self.jobs.len() - MAX_JOBS);
        }
    }
}

pub fn default_path() -> PathBuf {
    if let Ok(data) = std::env::var("XDG_DATA_HOME") {
        return PathBuf::from(data).join("orleans/job-history.json");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".local/share/orleans/job-history.json");
    }
    PathBuf::from("orleans-job-history.json")
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// Short relative age for the history list.
pub fn format_age(started: u64) -> String {
    let seconds = now_unix().saturating_sub(started);
    if seconds < 60 {
        "just now".to_owned()
    } else if seconds < 3600 {
        format!("{}m ago", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h ago", seconds / 3600)
    } else {
        format!("{}d ago", seconds / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(label: &str) -> JobRecord {
        JobRecord {
            started: 1_700_000_000,
            finished: 1_700_000_060,
            label: label.to_owned(),
            kind: JobKind::Cut,
            outcome: JobOutcome::Completed,
            cuts: 3,
            layers: 1,
            cut_mm: 12.5,
            travel_mm: 2.5,
            estimated_secs: 55.0,
            actual_secs: 60.0,
            lines_done: 100,
            lines_total: 100,
            source: Some("design.svg".to_owned()),
            message: None,
        }
    }

    #[test]
    fn history_keeps_only_recent_jobs() {
        let mut history = JobHistory::default();
        for index in 0..(MAX_JOBS + 5) {
            history.push(record(&format!("job {index}")));
        }
        assert_eq!(history.jobs.len(), MAX_JOBS);
        assert_eq!(history.jobs[0].label, "job 5");
        assert_eq!(
            history.jobs[MAX_JOBS - 1].label,
            format!("job {}", MAX_JOBS + 4)
        );
    }

    #[test]
    fn history_round_trips_through_disk() {
        let path =
            std::env::temp_dir().join(format!("orleans-history-test-{}.json", std::process::id()));
        let mut history = JobHistory::default();
        history.push(record("test"));
        history.save(&path).expect("save");
        let loaded = JobHistory::load(&path);
        assert_eq!(loaded.jobs.len(), 1);
        assert_eq!(loaded.jobs[0].label, "test");
        assert_eq!(loaded.jobs[0].outcome, JobOutcome::Completed);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn missing_or_invalid_history_loads_empty() {
        let path = std::env::temp_dir().join(format!(
            "orleans-history-invalid-{}.json",
            std::process::id()
        ));
        let _ = fs::write(&path, "{not json");
        assert!(JobHistory::load(&path).jobs.is_empty());
        let _ = fs::remove_file(path);
    }
}
