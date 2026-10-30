//! Safety checks run before framing or sending a job.

use kurbo::Rect;

use crate::i18n::tr;
use crate::machine::LinkKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Issue {
    NoCutLayers,
    NoCuts,
    ControllerNotReady,
    LaserOn,
    LaserModeOff,
    NoWorkArea,
    OutsideWorkArea,
    NoSerialPort,
    InvalidLayerSettings,
    HomingMismatch,
    SoftLimitsMismatch,
}

impl Issue {
    pub fn severity(self) -> Severity {
        match self {
            Self::NoWorkArea | Self::HomingMismatch | Self::SoftLimitsMismatch => Severity::Warning,
            _ => Severity::Error,
        }
    }

    pub fn text(self) -> String {
        match self {
            Self::NoCutLayers => tr("no layers are set to Cut").to_owned(),
            Self::NoCuts => tr("the job has no cuts").to_owned(),
            Self::ControllerNotReady => {
                tr("the controller is not idle or the laser is on").to_owned()
            }
            Self::LaserOn => tr("the laser is on; stop before sending").to_owned(),
            Self::LaserModeOff => tr("laser mode is off ($32=0)").to_owned(),
            Self::NoWorkArea => {
                tr("machine work area is unknown; bounds were not checked").to_owned()
            }
            Self::OutsideWorkArea => {
                tr("the design extends outside the machine work area").to_owned()
            }
            Self::NoSerialPort => tr("select a serial port before connecting").to_owned(),
            Self::InvalidLayerSettings => {
                tr("a layer has an invalid speed or power setting").to_owned()
            }
            Self::HomingMismatch => {
                tr("the profile expects homing, but the controller reports $22=0").to_owned()
            }
            Self::SoftLimitsMismatch => {
                tr("the profile expects soft limits, but the controller reports $20=0").to_owned()
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Input {
    pub bounds: Option<Rect>,
    pub bed: Option<[f64; 2]>,
    pub connection: LinkKind,
    pub endpoint_empty: bool,
    pub controller_ready: bool,
    pub laser_off: bool,
    pub laser_mode_off: bool,
    pub cut_layers: usize,
    pub cuts: usize,
    pub invalid_layer_settings: bool,
    pub profile_homing: Option<bool>,
    pub profile_soft_limits: Option<bool>,
    pub controller_homing: Option<bool>,
    pub controller_soft_limits: Option<bool>,
}

pub fn check(input: &Input) -> Vec<Issue> {
    let mut issues = Vec::new();
    if input.cut_layers == 0 {
        issues.push(Issue::NoCutLayers);
    }
    if input.cuts == 0 {
        issues.push(Issue::NoCuts);
    }
    if !input.controller_ready {
        issues.push(Issue::ControllerNotReady);
    }
    if !input.laser_off {
        issues.push(Issue::LaserOn);
    }
    if input.laser_mode_off {
        issues.push(Issue::LaserModeOff);
    }
    if input.connection == LinkKind::Serial && input.endpoint_empty {
        issues.push(Issue::NoSerialPort);
    }
    if input.invalid_layer_settings {
        issues.push(Issue::InvalidLayerSettings);
    }
    if let Some(bounds) = input.bounds {
        match input.bed {
            Some([width, height]) if width > 0.0 && height > 0.0 => {
                let outside = bounds.x0 < -0.01
                    || bounds.y0 < -0.01
                    || bounds.x1 > width + 0.01
                    || bounds.y1 > height + 0.01;
                if outside {
                    issues.push(Issue::OutsideWorkArea);
                }
            }
            _ => issues.push(Issue::NoWorkArea),
        }
    }
    if input.profile_homing == Some(true) && input.controller_homing == Some(false) {
        issues.push(Issue::HomingMismatch);
    }
    if input.profile_soft_limits == Some(true) && input.controller_soft_limits == Some(false) {
        issues.push(Issue::SoftLimitsMismatch);
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> Input {
        Input {
            bounds: Some(Rect::new(0.0, 0.0, 100.0, 80.0)),
            bed: Some([220.0, 220.0]),
            connection: LinkKind::Tcp,
            endpoint_empty: false,
            controller_ready: true,
            laser_off: true,
            laser_mode_off: false,
            cut_layers: 2,
            cuts: 8,
            invalid_layer_settings: false,
            profile_homing: Some(true),
            profile_soft_limits: Some(true),
            controller_homing: Some(true),
            controller_soft_limits: Some(true),
        }
    }

    #[test]
    fn a_ready_job_has_no_issues() {
        assert!(check(&ready()).is_empty());
    }

    #[test]
    fn outside_bounds_block_the_job() {
        let mut input = ready();
        input.bounds = Some(Rect::new(-1.0, 0.0, 100.0, 80.0));
        assert_eq!(check(&input), vec![Issue::OutsideWorkArea]);
    }

    #[test]
    fn unknown_work_area_is_a_warning() {
        let mut input = ready();
        input.bed = None;
        let issues = check(&input);
        assert_eq!(issues, vec![Issue::NoWorkArea]);
        assert_eq!(issues[0].severity(), Severity::Warning);
    }

    #[test]
    fn unsafe_controller_state_is_an_error() {
        let mut input = ready();
        input.controller_ready = false;
        input.laser_off = false;
        input.laser_mode_off = true;
        assert_eq!(
            check(&input),
            vec![
                Issue::ControllerNotReady,
                Issue::LaserOn,
                Issue::LaserModeOff
            ]
        );
    }

    #[test]
    fn serial_needs_a_port() {
        let mut input = ready();
        input.connection = LinkKind::Serial;
        input.endpoint_empty = true;
        assert_eq!(check(&input), vec![Issue::NoSerialPort]);
    }

    #[test]
    fn invalid_layer_settings_block_the_job() {
        let mut input = ready();
        input.invalid_layer_settings = true;
        assert_eq!(check(&input), vec![Issue::InvalidLayerSettings]);
    }

    #[test]
    fn profile_expectations_warn_on_mismatch() {
        let mut input = ready();
        input.controller_homing = Some(false);
        input.controller_soft_limits = Some(false);
        assert_eq!(
            check(&input),
            vec![Issue::HomingMismatch, Issue::SoftLimitsMismatch]
        );
    }
}
