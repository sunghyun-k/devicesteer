//! Who may drive the device: an agent's session and the human at the viewer share it.
//!
//! An agent opens a **session** for one goal (`session start "<name>"`) and sends its input with
//! the session's id. One agent session per device at a time. The session has an owner:
//!
//! - `agent`: the agent drives.
//! - `delegated_to_user`: the agent handed the device to the human (`session handoff --reason`),
//!   for a passcode, a login, a payment or a judgement call. The human presses Done (or tells the
//!   agent), and the agent takes it back with `session resume`.
//! - `user`: the human took over in the viewer. Agent input is refused until they hand it back.
//! - `finished`: ended by the agent (`session finish`), by the human (Stop, or
//!   `session finish --force`), or by expiry when its agent went quiet.
//!
//! Input sent without a session still works while no session runs, so scripts and older callers
//! keep working, but its result carries a `note` pointing at sessions: without one the viewer
//! can't show who drives, and the human has nothing to stop.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

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
    /// The human drives; agent input is refused until `hand_back`.
    TakeOver,
    /// Give the device back to the agent after a take over.
    #[serde(alias = "release")]
    HandBack,
    /// The human finished what the agent handed off to them.
    Done,
    /// End the agent's session now.
    #[serde(alias = "stop_agent")]
    Stop,
}

/// Who holds an agent session's device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Owner {
    Agent,
    DelegatedToUser,
    User,
    Finished,
}

/// Why a session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Ended {
    /// The agent ran `session finish`.
    Finished,
    /// Someone ran `session finish --force`.
    Forced,
    /// The human pressed Stop in the viewer.
    StoppedByUser,
    /// No command for longer than the idle limit.
    Expired,
}

/// A session whose agent sent nothing for this long, while it drives, has ended: its process is
/// most likely gone, and a session that never ends would lock the device for every other agent.
pub const AGENT_IDLE: Duration = Duration::from_secs(10 * 60);
/// The same while the human has the device (a handoff or a take over): a login or a payment can
/// take a while, and the agent is waiting on purpose.
pub const HUMAN_IDLE: Duration = Duration::from_secs(30 * 60);
/// Ended sessions remembered, so a late command with one's id gets told why it ended.
const RECENT: usize = 8;
const MAX_NAME: usize = 80;
const MAX_REASON: usize = 200;

#[derive(Debug, Clone)]
pub struct AgentSession {
    pub id: String,
    pub name: String,
    pub owner: Owner,
    /// Why the agent handed off (for `delegated_to_user`).
    pub reason: Option<String>,
    pub ended: Option<Ended>,
    started: Instant,
    /// The agent's last command with this session's id.
    last_command: Instant,
    /// When `owner` last changed.
    owner_since: Instant,
    /// The human gave the device back since the agent's last input: the screen may have changed.
    returned: bool,
    /// The idle limit when it ended: after that the owner is `finished`, which has none.
    limit_at_end: Option<Duration>,
}

impl AgentSession {
    fn idle_limit(&self) -> Duration {
        match self.owner {
            Owner::Agent => AGENT_IDLE,
            _ => HUMAN_IDLE,
        }
    }

    fn idle(&self) -> Duration {
        self.last_command.max(self.owner_since).elapsed()
    }

    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "owner": self.owner,
            "reason": self.reason,
            "ended": self.ended,
            "seconds": self.started.elapsed().as_secs(),
            "idle_seconds": self.idle().as_secs(),
            "expires_in_seconds": match self.ended {
                None => Some(self.idle_limit().saturating_sub(self.idle()).as_secs()),
                Some(_) => None,
            },
        })
    }

    fn set_owner(&mut self, owner: Owner) {
        if self.owner != owner {
            self.owner = owner;
            self.owner_since = Instant::now();
        }
    }

    /// What an agent sending input with this ended session's id should do next.
    fn ended_message(&self) -> String {
        let (id, name) = (&self.id, &self.name);
        match self.ended {
            Some(Ended::StoppedByUser) => format!(
                "the user stopped session {id} '{name}' in the viewer. Stop now: don't retry, and don't start a new \
                 session to get around it. Tell the user where you stopped; start a new session only if they ask \
                 you to continue"
            ),
            Some(Ended::Forced) => format!(
                "session {id} '{name}' was ended with `session finish --force`. Stop and tell the user; start a new \
                 session only if they ask you to continue"
            ),
            Some(Ended::Expired) => format!(
                "session {id} '{name}' expired after {} min without a command. The user may have used the device \
                 since: read the screen again (`ui`), then start a new session with `iphone-use session start \
                 \"<goal>\"`",
                self.limit_at_end.unwrap_or(AGENT_IDLE).as_secs() / 60
            ),
            _ => format!(
                "session {id} '{name}' has finished. Start a new session for new work: `iphone-use session start \
                 \"<goal>\"`"
            ),
        }
    }
}

/// Sent with input that came without a session.
pub const NO_SESSION_NOTE: &str = "sent without a session: the viewer can't show who is driving or let the user \
    stop you. Start one with `iphone-use session start \"<goal>\"` and pass its id with --session (or \
    IPHONE_USE_SESSION)";
/// Sent with the first input after the human gave the device back.
pub const RETURNED_NOTE: &str = "the user used the device and handed it back before this input; read the screen \
    (`ui`) again if you haven't";

#[derive(Debug, Default)]
pub struct ControlState {
    /// A human's finger is down in the viewer (a live touch).
    pub human_touching: bool,
    session: Option<AgentSession>,
    recent: VecDeque<AgentSession>,
    /// The human pressed Stop: input without a session is refused until a new session starts,
    /// or an agent that never used sessions would carry on as if nothing happened.
    stopped: bool,
}

/// A session id: short enough to type, random enough not to collide on one device.
fn new_id() -> String {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let mix = (n as u64) ^ ((std::process::id() as u64) << 32) ^ (n >> 64) as u64;
    // SplitMix64, so ids from nanoseconds apart look unrelated.
    let mut z = mix.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    format!("s{:06x}", z & 0xff_ffff)
}

fn clean(text: &str, what: &str, max: usize) -> Result<String, String> {
    let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.is_empty() {
        return Err(format!("the {what} is empty; give a few words"));
    }
    if t.chars().count() > max {
        return Err(format!("the {what} is longer than {max} characters; shorten it"));
    }
    Ok(t)
}

impl ControlState {
    /// The running session, if any.
    pub fn session(&self) -> Option<&AgentSession> {
        self.session.as_ref()
    }

    fn end(&mut self, why: Ended) -> Option<AgentSession> {
        let mut s = self.session.take()?;
        s.ended = Some(why);
        s.limit_at_end = Some(s.idle_limit());
        s.set_owner(Owner::Finished);
        if self.recent.len() == RECENT {
            self.recent.pop_front();
        }
        self.recent.push_back(s.clone());
        Some(s)
    }

    /// Ends a session that went quiet for too long. True when it did (send a `control` event).
    pub fn expire(&mut self) -> bool {
        if self.session.as_ref().is_some_and(|s| s.idle() > s.idle_limit()) {
            let s = self.end(Ended::Expired);
            tracing::info!("session {:?} expired", s.map(|s| s.id));
            return true;
        }
        false
    }

    fn live_message(s: &AgentSession) -> String {
        format!(
            "session {} '{}' is using this device (owner: {}). If it is yours, pass its id with --session (or \
             IPHONE_USE_SESSION). Otherwise wait until it finishes, or ask the user: if its agent is gone, they can \
             end it with `iphone-use session finish --force`",
            s.id,
            s.name,
            serde_json::to_value(s.owner).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
        )
    }

    /// `session start`: a new session for one goal, refused while another one runs.
    pub fn start(&mut self, name: &str) -> Result<Value, String> {
        self.expire();
        if let Some(s) = &self.session {
            return Err(Self::live_message(s));
        }
        let name = clean(name, "session name", MAX_NAME)?;
        let now = Instant::now();
        let s = AgentSession {
            id: new_id(),
            name,
            owner: Owner::Agent,
            reason: None,
            ended: None,
            started: now,
            last_command: now,
            owner_since: now,
            returned: false,
            limit_at_end: None,
        };
        self.stopped = false;
        let v = s.to_json();
        self.session = Some(s);
        Ok(v)
    }

    /// The running session if `id` is it, else why not.
    fn mine(&mut self, id: &str) -> Result<&mut AgentSession, String> {
        self.expire();
        match &self.session {
            Some(s) if s.id == id => Ok(self.session.as_mut().expect("matched above")),
            live => {
                if let Some(old) = self.recent.iter().rev().find(|s| s.id == id) {
                    return Err(old.ended_message());
                }
                let mut m = format!("no session {id} on this device");
                if let Some(s) = live {
                    m.push_str(&format!("; {}", Self::live_message(s)));
                } else {
                    m.push_str("; start one with `iphone-use session start \"<goal>\"`");
                }
                Err(m)
            }
        }
    }

    /// `session finish`: the agent ends its session; `force` ends whichever session runs (the
    /// human's way to clear one whose agent is gone).
    pub fn finish(&mut self, id: Option<&str>, force: bool) -> Result<Value, String> {
        if force {
            self.expire();
            return match self.end(Ended::Forced) {
                Some(s) => Ok(s.to_json()),
                None => Ok(json!({ "ended": null, "note": "no session was running" })),
            };
        }
        let id = id.ok_or("say which session to finish with --session (or IPHONE_USE_SESSION); to end another agent's session, the user can run `session finish --force`")?;
        if let Some(old) = self.recent.iter().rev().find(|s| s.id == id)
            && self.session.as_ref().is_none_or(|s| s.id != id)
        {
            // Finishing twice is not an error: the agent's goal is done either way.
            return Ok(old.to_json());
        }
        self.mine(id)?;
        Ok(self.end(Ended::Finished).expect("checked by mine").to_json())
    }

    /// `session handoff`: the agent hands the device to the human, with what they should do.
    pub fn handoff(&mut self, id: &str, reason: &str) -> Result<Value, String> {
        let reason = clean(reason, "handoff reason", MAX_REASON)?;
        let s = self.mine(id)?;
        if s.owner == Owner::User {
            return Err(format!(
                "the user already took over session {id} in the viewer; wait for them to hand it back"
            ));
        }
        s.set_owner(Owner::DelegatedToUser);
        s.reason = Some(reason);
        s.last_command = Instant::now();
        Ok(s.to_json())
    }

    /// `session resume`: the agent takes the device back after a handoff (once the human is done)
    /// or after the human handed it back. Refused while the human holds it after a take over.
    pub fn resume(&mut self, id: &str) -> Result<Value, String> {
        let s = self.mine(id)?;
        if s.owner == Owner::User {
            return Err(format!(
                "the user took over session {id} in the viewer and still has it. Don't send input; wait for them \
                 to press Hand back (`session status` then shows owner \"agent\") or to tell you to continue"
            ));
        }
        s.set_owner(Owner::Agent);
        s.reason = None;
        s.returned = false;
        s.last_command = Instant::now();
        Ok(s.to_json())
    }

    /// `session status`: the session `id` (running or recently ended), or the running one.
    pub fn status(&mut self, id: Option<&str>) -> Result<Value, String> {
        self.expire();
        match id {
            Some(id) => {
                if let Some(s) = self.session.as_mut().filter(|s| s.id == id) {
                    s.last_command = Instant::now();
                    return Ok(json!({ "session": s.to_json() }));
                }
                match self.recent.iter().rev().find(|s| s.id == id) {
                    Some(s) => Ok(json!({ "session": s.to_json(), "next": s.ended_message() })),
                    None => self.mine(id).map(|_| Value::Null),
                }
            }
            None => Ok(json!({ "session": self.session.as_ref().map(AgentSession::to_json) })),
        }
    }

    /// Checks an agent's command against the session state. `input`: the command acts on the
    /// device (reads are allowed in any state). Ok carries a note for the result, if any.
    pub fn check_agent(&mut self, id: Option<&str>, input: bool) -> Result<Option<&'static str>, String> {
        let note = match id {
            None => {
                self.expire();
                if !input {
                    return Ok(None);
                }
                if let Some(s) = &self.session {
                    return Err(Self::live_message(s));
                }
                if self.stopped {
                    return Err("the user pressed Stop in the viewer. Stop now: don't retry, and don't route around \
                                it. Tell the user where you stopped; continue only if they ask, in a new session \
                                (`iphone-use session start \"<goal>\"`)"
                        .into());
                }
                Some(NO_SESSION_NOTE)
            }
            Some(id) => {
                let s = self.mine(id)?;
                s.last_command = Instant::now();
                if !input {
                    return Ok(None);
                }
                match s.owner {
                    Owner::User => {
                        return Err(format!(
                            "the user took over session {} in the viewer; agent input is refused until they hand it \
                             back. Stop here: don't retry or work around it. Tell the user you are waiting, and run \
                             `iphone-use session resume` once they press Hand back or tell you to continue",
                            s.id
                        ));
                    }
                    Owner::DelegatedToUser => {
                        return Err(format!(
                            "you handed session {} to the user (\"{}\"); agent input is refused until you take it \
                             back. Stop and tell the user what they need to do; once they press Done in the viewer or \
                             tell you they are done, run `iphone-use session resume`",
                            s.id,
                            s.reason.as_deref().unwrap_or("")
                        ));
                    }
                    Owner::Finished => return Err(s.ended_message()),
                    Owner::Agent => {}
                }
                std::mem::take(&mut s.returned).then_some(RETURNED_NOTE)
            }
        };
        if self.human_touching {
            return Err("the user is touching the screen in the viewer and kept touching while the agent waited; \
                        read the screen again and retry"
                .into());
        }
        Ok(note)
    }

    /// Applies the human's action. Ok carries whose finger to lift, if anyone's.
    pub fn apply(&mut self, action: ControlAction) -> Result<Option<Source>, String> {
        self.expire();
        let Some(s) = self.session.as_mut() else {
            return Err("no agent session is running on this device".into());
        };
        Ok(match action {
            ControlAction::TakeOver => {
                if s.owner == Owner::Agent {
                    s.set_owner(Owner::User);
                }
                Some(Source::Agent)
            }
            // Either way of giving the device back means the same to the agent.
            ControlAction::HandBack | ControlAction::Done => {
                if matches!(s.owner, Owner::User | Owner::DelegatedToUser) {
                    s.set_owner(Owner::Agent);
                    s.reason = None;
                    s.returned = true;
                }
                Some(Source::Human)
            }
            ControlAction::Stop => {
                self.end(Ended::StoppedByUser);
                self.stopped = true;
                Some(Source::Agent)
            }
        })
    }

    /// The `control` event for the current state.
    pub fn event(&self) -> Value {
        json!({
            "event": "control",
            "session": self.session.as_ref().map(AgentSession::to_json),
            "human_touching": self.human_touching,
            "agent_stopped": self.stopped,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started(c: &mut ControlState) -> String {
        c.start("check the weather").unwrap()["id"].as_str().unwrap().to_string()
    }

    #[test]
    fn one_session_per_device() {
        let mut c = ControlState::default();
        let id = started(&mut c);
        let err = c.start("another goal").unwrap_err();
        assert!(err.contains(&id) && err.contains("is using this device"), "{err}");
        // Input without the id is refused while it runs; reads are not.
        assert!(c.check_agent(None, true).unwrap_err().contains("is using this device"));
        assert_eq!(c.check_agent(None, false), Ok(None));
        assert_eq!(c.check_agent(Some(&id), true), Ok(None));
        c.finish(Some(&id), false).unwrap();
        // Finishing twice is fine; its id now says it ended.
        assert_eq!(c.finish(Some(&id), false).unwrap()["ended"], "finished");
        assert!(c.check_agent(Some(&id), true).unwrap_err().contains("has finished"));
        started(&mut c);
    }

    #[test]
    fn input_without_a_session_carries_a_note() {
        let mut c = ControlState::default();
        assert_eq!(c.check_agent(None, true), Ok(Some(NO_SESSION_NOTE)));
        assert!(c.check_agent(Some("s000000"), true).unwrap_err().contains("no session"));
    }

    #[test]
    fn take_over_refuses_the_agent_until_hand_back() {
        let mut c = ControlState::default();
        let id = started(&mut c);
        assert_eq!(c.apply(ControlAction::TakeOver), Ok(Some(Source::Agent)));
        assert_eq!(c.session().unwrap().owner, Owner::User);
        assert!(c.check_agent(Some(&id), true).unwrap_err().contains("took over"));
        assert!(c.resume(&id).unwrap_err().contains("still has it"));
        // Reading the screen is still allowed.
        assert_eq!(c.check_agent(Some(&id), false), Ok(None));
        c.apply(ControlAction::HandBack).unwrap();
        assert_eq!(c.session().unwrap().owner, Owner::Agent);
        // The first input after the hand back says the screen may have changed.
        assert_eq!(c.check_agent(Some(&id), true), Ok(Some(RETURNED_NOTE)));
        assert_eq!(c.check_agent(Some(&id), true), Ok(None));
        c.resume(&id).unwrap();
    }

    #[test]
    fn handoff_waits_for_done_or_resume() {
        let mut c = ControlState::default();
        let id = started(&mut c);
        let s = c.handoff(&id, "enter the passcode").unwrap();
        assert_eq!(s["owner"], "delegated_to_user");
        assert_eq!(s["reason"], "enter the passcode");
        assert!(c.check_agent(Some(&id), true).unwrap_err().contains("handed session"));
        c.apply(ControlAction::Done).unwrap();
        assert_eq!(c.session().unwrap().owner, Owner::Agent);
        assert!(c.session().unwrap().reason.is_none());
        // Or the agent resumes when the user says so in chat.
        c.handoff(&id, "sign in").unwrap();
        assert_eq!(c.resume(&id).unwrap()["owner"], "agent");
        assert_eq!(c.check_agent(Some(&id), true), Ok(None));
    }

    #[test]
    fn stop_ends_the_session_and_refuses_sessionless_input() {
        let mut c = ControlState::default();
        let id = started(&mut c);
        assert_eq!(c.apply(ControlAction::Stop), Ok(Some(Source::Agent)));
        assert!(c.session().is_none());
        assert!(c.check_agent(Some(&id), true).unwrap_err().contains("stopped session"));
        assert!(c.check_agent(None, true).unwrap_err().contains("pressed Stop"));
        assert!(c.apply(ControlAction::TakeOver).is_err());
        // A new session (the user asked to continue) clears it.
        started(&mut c);
        assert!(!c.event()["agent_stopped"].as_bool().unwrap());
    }

    #[test]
    fn force_finish_clears_another_agents_session() {
        let mut c = ControlState::default();
        let id = started(&mut c);
        assert!(c.finish(None, false).is_err());
        assert_eq!(c.finish(None, true).unwrap()["ended"], "forced");
        assert!(c.check_agent(Some(&id), true).unwrap_err().contains("--force"));
        assert!(c.finish(None, true).is_ok());
    }

    #[test]
    fn quiet_sessions_expire() {
        let mut c = ControlState::default();
        let id = started(&mut c);
        let s = c.session.as_mut().unwrap();
        s.last_command -= AGENT_IDLE + Duration::from_secs(1);
        s.owner_since -= AGENT_IDLE + Duration::from_secs(1);
        assert!(c.expire());
        assert!(c.session().is_none());
        assert!(c.check_agent(Some(&id), true).unwrap_err().contains("expired after 10 min"));
        assert_eq!(c.status(Some(&id)).unwrap()["session"]["ended"], "expired");
    }

    #[test]
    fn the_human_gets_longer_before_expiry() {
        let mut c = ControlState::default();
        let id = started(&mut c);
        c.handoff(&id, "log in").unwrap();
        let s = c.session.as_mut().unwrap();
        s.last_command -= AGENT_IDLE + Duration::from_secs(1);
        s.owner_since -= AGENT_IDLE + Duration::from_secs(1);
        assert!(!c.expire());
    }

    #[test]
    fn a_human_touch_still_refuses_the_agent() {
        let mut c = ControlState::default();
        let id = started(&mut c);
        c.human_touching = true;
        assert!(c.check_agent(Some(&id), true).unwrap_err().contains("touching"));
    }

    #[test]
    fn names_are_trimmed_and_bounded() {
        let mut c = ControlState::default();
        assert!(c.start("   ").is_err());
        assert!(c.start(&"x".repeat(MAX_NAME + 1)).is_err());
        assert_eq!(c.start("  find\n the  photo ").unwrap()["name"], "find the photo");
        assert!(new_id().starts_with('s') && new_id().len() == 7);
    }
}
