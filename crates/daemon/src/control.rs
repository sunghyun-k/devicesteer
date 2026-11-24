//! Who may drive the device: the agent (CLI, MCP) and the human at the viewer share it, and the
//! human can take over or stop the agent at any time.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Where a request comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    #[default]
    Agent,
    Human,
}

/// What the human asked for in the viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlAction {
    /// The human drives; agent input is refused until `release`.
    TakeOver,
    Release,
    /// Refuse agent input until `resume_agent`, without taking over.
    StopAgent,
    ResumeAgent,
}

/// How long after its last input the agent still counts as driving (for the viewer's indicator).
pub const AGENT_ACTIVE_FOR: Duration = Duration::from_millis(2500);

#[derive(Debug, Default)]
pub struct ControlState {
    pub human_owns: bool,
    pub agent_stopped: bool,
    /// A human's finger is down in the viewer (a live touch).
    pub human_touching: bool,
    last_agent_input: Option<Instant>,
    /// What the last `control` event said `agent_active` was.
    reported_active: bool,
}

impl ControlState {
    pub fn agent_active(&self) -> bool {
        self.last_agent_input.is_some_and(|t| t.elapsed() < AGENT_ACTIVE_FOR)
    }

    /// Why an agent's input is refused now, if it is. The text tells the agent what to do.
    pub fn refuse_agent(&self) -> Option<&'static str> {
        if self.human_owns {
            Some("a human took over the device in the viewer; agent input is refused until they hand control back. Wait and retry later")
        } else if self.agent_stopped {
            Some("a human stopped the agent in the viewer; agent input is refused until they resume it. Stop and report back")
        } else if self.human_touching {
            Some("a human is touching the screen in the viewer and kept touching while the agent waited; read the screen again and retry")
        } else {
            None
        }
    }

    pub fn agent_input(&mut self) {
        self.last_agent_input = Some(Instant::now());
    }

    pub fn apply(&mut self, action: ControlAction) {
        match action {
            ControlAction::TakeOver => self.human_owns = true,
            ControlAction::Release => self.human_owns = false,
            ControlAction::StopAgent => self.agent_stopped = true,
            ControlAction::ResumeAgent => self.agent_stopped = false,
        }
    }

    /// The `control` event for the current state, and remembers what it said.
    pub fn event(&mut self) -> Value {
        self.reported_active = self.agent_active();
        json!({
            "event": "control",
            "owner": if self.human_owns { "human" } else { "shared" },
            "agent_stopped": self.agent_stopped,
            "agent_active": self.reported_active,
        })
    }

    /// Whether `agent_active` changed since the last event (the timer checks this).
    pub fn active_changed(&self) -> bool {
        self.agent_active() != self.reported_active
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_human_take_over_or_stop_refuses_the_agent() {
        let mut c = ControlState::default();
        assert!(c.refuse_agent().is_none());
        c.apply(ControlAction::TakeOver);
        assert!(c.refuse_agent().unwrap().contains("took over"));
        c.apply(ControlAction::Release);
        c.apply(ControlAction::StopAgent);
        assert!(c.refuse_agent().unwrap().contains("stopped"));
        c.apply(ControlAction::ResumeAgent);
        assert!(c.refuse_agent().is_none());
        c.human_touching = true;
        assert!(c.refuse_agent().unwrap().contains("touching"));
    }

    #[test]
    fn agent_activity_shows_until_it_goes_quiet() {
        let mut c = ControlState::default();
        assert!(!c.agent_active());
        c.agent_input();
        assert!(c.agent_active());
        assert!(c.active_changed());
        assert_eq!(c.event()["agent_active"], true);
        assert!(!c.active_changed());
    }
}
