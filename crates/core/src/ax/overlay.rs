//! When to end the empty Siri app that hides the screen from the accessibility service.
//!
//! Folded, a foldable's accessibility service listed only Siri (`/Applications/Siri AI.app`), with
//! nothing in it, over whatever the screen showed, reading after reading; ending that one process
//! brought the tree back at once with no visible change, and it didn't come back (PITFALLS #71).

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use super::{attrs, dump::Dump};

/// Siri's bundle id (`/Applications/Siri AI.app`, named Siri by installation_proxy).
pub(crate) const SIRI: &str = "com.apple.campo";

/// How long a pid that was ended is left alone, should it still be listed.
const COOLDOWN: Duration = Duration::from_secs(60);

/// The pid of Siri when it is the dump's only application and has nothing in it: no other
/// element in the dump and no children. Anything else, another app beside it or a single element,
/// and it is left alone.
pub(crate) fn lone_empty_siri(dump: &Dump, bundle_of: impl Fn(i64) -> Option<String>) -> Option<i64> {
    let [app] = dump.applications[..] else { return None };
    if bundle_of(app.pid).as_deref() != Some(SIRI) {
        return None;
    }
    if dump.elements.keys().any(|k| *k != app) {
        return None;
    }
    let raw = dump.elements.get(&app)?;
    let childless = [attrs::CHILDREN, attrs::VISIBLE_CHILDREN]
        .iter()
        .all(|n| raw.attrs.get(n).and_then(|v| v.as_array()).is_none_or(|a| a.is_empty()));
    childless.then_some(app.pid)
}

/// The pids ended lately, so the same one isn't ended again and again.
#[derive(Debug, Default)]
pub(crate) struct Cooldown {
    ended: HashMap<i64, Instant>,
}

impl Cooldown {
    pub(crate) fn allows(&self, pid: i64, now: Instant) -> bool {
        self.ended.get(&pid).is_none_or(|at| now.saturating_duration_since(*at) >= COOLDOWN)
    }

    pub(crate) fn record(&mut self, pid: i64, now: Instant) {
        self.ended.retain(|_, at| now.saturating_duration_since(*at) < COOLDOWN);
        self.ended.insert(pid, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ax::{
        archive::Ka,
        dump::{DumpKind, ObjKey, RawElement},
    };

    fn dump(apps: &[(i64, &[u64])], extra: &[(i64, u64)]) -> Dump {
        let mut elements = std::collections::HashMap::new();
        let mut applications = Vec::new();
        for &(pid, children) in apps {
            let key = ObjKey { pid, object_id: 1 };
            let mut raw = RawElement { is_application: true, ..Default::default() };
            let kids = children
                .iter()
                .map(|&id| Ka::Obj {
                    class: "AXPTranslationObject".into(),
                    fields: vec![("pid".into(), Ka::Int(pid)), ("objectID".into(), Ka::UInt(id))],
                })
                .collect();
            raw.attrs.insert(attrs::VISIBLE_CHILDREN, Ka::Array(kids));
            raw.attrs.insert(attrs::CHILDREN, Ka::Array(vec![]));
            elements.insert(key, raw);
            applications.push(key);
        }
        for &(pid, object_id) in extra {
            elements.insert(ObjKey { pid, object_id }, RawElement::default());
        }
        Dump { kind: DumpKind::Initial, applications, order: elements.keys().copied().collect(), elements }
    }

    fn bundles(pid: i64) -> Option<String> {
        Some(match pid {
            11 => SIRI.into(),
            853 => "com.apple.springboard".into(),
            _ => "com.example.app".into(),
        })
    }

    #[test]
    fn only_a_lone_empty_siri_is_ended() {
        // As seen folded (iOS 27.3): Siri alone, no children, no other element (PITFALLS #71).
        assert_eq!(lone_empty_siri(&dump(&[(11, &[])], &[]), bundles), Some(11));
        // With children, or an element of its own: left alone.
        assert_eq!(lone_empty_siri(&dump(&[(11, &[2])], &[]), bundles), None);
        assert_eq!(lone_empty_siri(&dump(&[(11, &[])], &[(11, 2)]), bundles), None);
        // Beside another app: left alone.
        assert_eq!(lone_empty_siri(&dump(&[(11, &[]), (853, &[])], &[]), bundles), None);
        // Another app alone and empty (right after a launch, PITFALLS #10): left alone.
        assert_eq!(lone_empty_siri(&dump(&[(500, &[])], &[]), bundles), None);
        assert_eq!(lone_empty_siri(&dump(&[], &[]), bundles), None);
    }

    #[test]
    fn a_pid_is_not_ended_twice_within_the_cooldown() {
        let mut c = Cooldown::default();
        let t = Instant::now();
        assert!(c.allows(11, t));
        c.record(11, t);
        assert!(!c.allows(11, t + Duration::from_secs(5)));
        assert!(c.allows(12, t + Duration::from_secs(5)));
        assert!(c.allows(11, t + COOLDOWN));
    }
}
