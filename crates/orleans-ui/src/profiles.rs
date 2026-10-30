//! Saved machine connection and setup profiles.
//!
//! Profiles keep the transport, address, baud rate, protocol preference,
//! work area and job defaults together, so switching between a USB Marlin
//! laser and a WiFi GRBL controller does not require retyping or re-checking
//! the machine setup.

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

fn default_dynamic_power() -> bool {
    true
}

fn default_speed() -> f64 {
    1000.0
}

fn default_power() -> f64 {
    20.0
}

fn default_framing_feed() -> f64 {
    3000.0
}

/// Per-machine setup that is not stored in controller EEPROM.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MachineSetup {
    #[serde(default)]
    pub bed: Option<[f64; 2]>,
    #[serde(default = "default_dynamic_power")]
    pub dynamic_power: bool,
    #[serde(default = "default_speed")]
    pub default_speed: f64,
    #[serde(default = "default_power")]
    pub default_power: f64,
    #[serde(default = "default_framing_feed")]
    pub framing_feed: f64,
    #[serde(default)]
    pub framing_margin: f64,
    #[serde(default)]
    pub homing: Option<bool>,
    #[serde(default)]
    pub soft_limits: Option<bool>,
    #[serde(default)]
    pub notes: String,
}

impl Default for MachineSetup {
    fn default() -> Self {
        Self {
            bed: None,
            dynamic_power: true,
            default_speed: default_speed(),
            default_power: default_power(),
            framing_feed: default_framing_feed(),
            framing_margin: 0.0,
            homing: None,
            soft_limits: None,
            notes: String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MachineProfile {
    pub name: String,
    pub connection: Connection,
    #[serde(default = "default_max_s")]
    pub max_s: u32,
    #[serde(default)]
    pub setup: MachineSetup,
}

impl MachineProfile {
    pub fn new(
        name: impl Into<String>,
        connection: Connection,
        max_s: u32,
        setup: MachineSetup,
    ) -> Self {
        Self {
            name: name.into(),
            connection,
            max_s,
            setup,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MachineProfiles {
    #[serde(default = "default_format")]
    format: u32,
    #[serde(default)]
    pub profiles: Vec<MachineProfile>,
    #[serde(default)]
    pub last: Option<String>,
}

impl Default for MachineProfiles {
    fn default() -> Self {
        Self {
            format: FORMAT,
            profiles: Vec::new(),
            last: None,
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
    pub fn upsert(&mut self, mut profile: MachineProfile) {
        let name = profile.name.trim().to_owned();
        if name.is_empty() {
            return;
        }
        profile.name = name.clone();
        if let Some(existing) = self
            .profiles
            .iter_mut()
            .find(|existing| existing.name.eq_ignore_ascii_case(&name))
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
        if index >= self.profiles.len() {
            return None;
        }
        let removed = self.profiles.remove(index);
        if self
            .last
            .as_deref()
            .is_some_and(|last| last.eq_ignore_ascii_case(&removed.name))
        {
            self.last = None;
        }
        Some(removed)
    }

    pub fn find(&self, name: &str) -> Option<&MachineProfile> {
        self.profiles
            .iter()
            .find(|profile| profile.name.eq_ignore_ascii_case(name))
    }

    pub fn active(&self) -> Option<&MachineProfile> {
        self.last.as_deref().and_then(|name| self.find(name))
    }

    pub fn set_last(&mut self, name: &str) {
        if self.find(name).is_some() {
            self.last = Some(name.to_owned());
        }
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

    fn setup(bed: Option<[f64; 2]>) -> MachineSetup {
        MachineSetup {
            bed,
            default_speed: 1200.0,
            framing_margin: 3.0,
            notes: "test machine".to_owned(),
            ..MachineSetup::default()
        }
    }

    #[test]
    fn round_trips_profiles() {
        let path = std::env::temp_dir().join(format!(
            "orleans-profiles-{}-{}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let mut profiles = MachineProfiles::default();
        profiles.upsert(MachineProfile::new(
            "WiFi GRBL",
            sample_tcp(),
            1000,
            setup(Some([400.0, 300.0])),
        ));
        profiles.upsert(MachineProfile::new(
            "USB Marlin",
            sample_serial(),
            255,
            setup(Some([220.0, 220.0])),
        ));
        profiles.set_last("USB Marlin");
        profiles.save(&path).expect("save");

        let loaded = MachineProfiles::load(&path);
        let _ = fs::remove_file(path);
        assert_eq!(loaded.profiles.len(), 2);
        assert_eq!(loaded.profiles[0].connection.kind, LinkKind::Tcp);
        assert_eq!(loaded.profiles[1].connection.driver, DriverChoice::Marlin);
        assert_eq!(loaded.profiles[1].max_s, 255);
        assert_eq!(loaded.profiles[1].setup.bed, Some([220.0, 220.0]));
        assert_eq!(
            loaded.active().map(|profile| profile.name.as_str()),
            Some("USB Marlin")
        );
    }

    #[test]
    fn upsert_replaces_the_same_name_case_insensitively() {
        let mut profiles = MachineProfiles::default();
        profiles.upsert(MachineProfile::new(
            "Laser",
            sample_tcp(),
            1000,
            MachineSetup::default(),
        ));
        profiles.upsert(MachineProfile::new(
            "laser",
            sample_serial(),
            255,
            setup(None),
        ));
        assert_eq!(profiles.profiles.len(), 1);
        assert_eq!(profiles.profiles[0].name, "laser");
        assert_eq!(profiles.profiles[0].connection.kind, LinkKind::Serial);
    }

    #[test]
    fn remove_is_bounds_checked_and_clears_last() {
        let mut profiles = MachineProfiles::default();
        profiles.upsert(MachineProfile::new(
            "Laser",
            sample_tcp(),
            1000,
            MachineSetup::default(),
        ));
        profiles.set_last("Laser");
        assert!(profiles.remove(2).is_none());
        assert_eq!(profiles.remove(0).expect("profile").name, "Laser");
        assert!(profiles.profiles.is_empty());
        assert!(profiles.last.is_none());
    }
}
