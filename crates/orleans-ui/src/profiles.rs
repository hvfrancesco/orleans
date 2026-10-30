//! Saved machine connection profiles.
//!
//! Profiles keep the transport, address, baud rate, protocol preference and
//! max S together, so switching between a USB Marlin laser and a WiFi GRBL
//! controller does not require retyping the connection each time.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::machine::Connection;

pub const FORMAT: u32 = 1;
pub const MAX_PROFILES: usize = 20;

fn default_format() -> u32 {
    FORMAT
}

fn default_max_s() -> u32 {
    1000
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineProfile {
    pub name: String,
    pub connection: Connection,
    #[serde(default = "default_max_s")]
    pub max_s: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MachineProfiles {
    #[serde(default = "default_format")]
    format: u32,
    #[serde(default)]
    pub profiles: Vec<MachineProfile>,
}

impl Default for MachineProfiles {
    fn default() -> Self {
        Self {
            format: FORMAT,
            profiles: Vec::new(),
        }
    }
}

impl MachineProfiles {
    pub fn load(path: &Path) -> Self {
        let Ok(text) = fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(profiles) = serde_json::from_str::<Self>(&text) else {
            return Self::default();
        };
        if profiles.format != FORMAT {
            return Self::default();
        }
        profiles
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        fs::write(path, json)
    }

    /// Add a profile, replacing an existing one with the same name.
    pub fn upsert(&mut self, name: &str, connection: Connection, max_s: u32) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let profile = MachineProfile {
            name: name.to_owned(),
            connection,
            max_s,
        };
        if let Some(existing) = self
            .profiles
            .iter_mut()
            .find(|profile| profile.name.eq_ignore_ascii_case(name))
        {
            *existing = profile;
            return;
        }
        self.profiles.push(profile);
        if self.profiles.len() > MAX_PROFILES {
            self.profiles.drain(0..self.profiles.len() - MAX_PROFILES);
        }
    }

    pub fn remove(&mut self, index: usize) -> Option<MachineProfile> {
        (index < self.profiles.len()).then(|| self.profiles.remove(index))
    }
}

pub fn default_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("orleans").join("machines.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::{DriverChoice, LinkKind};

    fn sample_tcp() -> Connection {
        Connection::tcp("tcp://192.168.1.50", DriverChoice::Auto)
    }

    fn sample_serial() -> Connection {
        Connection::serial("/dev/ttyUSB0", 115_200, DriverChoice::Marlin)
    }

    #[test]
    fn round_trips_profiles() {
        let path = std::env::temp_dir().join(format!(
            "orleans-profiles-{}-{}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let mut profiles = MachineProfiles::default();
        profiles.upsert("WiFi GRBL", sample_tcp(), 1000);
        profiles.upsert("USB Marlin", sample_serial(), 255);
        profiles.save(&path).expect("save");

        let loaded = MachineProfiles::load(&path);
        let _ = fs::remove_file(path);
        assert_eq!(loaded.profiles.len(), 2);
        assert_eq!(loaded.profiles[0].connection.kind, LinkKind::Tcp);
        assert_eq!(loaded.profiles[1].connection.driver, DriverChoice::Marlin);
        assert_eq!(loaded.profiles[1].max_s, 255);
    }

    #[test]
    fn upsert_replaces_the_same_name_case_insensitively() {
        let mut profiles = MachineProfiles::default();
        profiles.upsert("Laser", sample_tcp(), 1000);
        profiles.upsert("laser", sample_serial(), 255);
        assert_eq!(profiles.profiles.len(), 1);
        assert_eq!(profiles.profiles[0].name, "laser");
        assert_eq!(profiles.profiles[0].connection.kind, LinkKind::Serial);
    }

    #[test]
    fn remove_is_bounds_checked() {
        let mut profiles = MachineProfiles::default();
        profiles.upsert("Laser", sample_tcp(), 1000);
        assert!(profiles.remove(2).is_none());
        assert_eq!(profiles.remove(0).expect("profile").name, "Laser");
        assert!(profiles.profiles.is_empty());
    }
}
