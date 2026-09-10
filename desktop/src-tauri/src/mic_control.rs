//! Pure microphone-selection policy.
//!
//! This module deliberately knows nothing about cpal, CoreAudio, capture
//! streams, channels, or threads. The controller serializes effects; this
//! reducer only decides which effect is next from an immutable registry view.

use crate::device_registry::{DeviceInfo, DeviceSnapshot};
use crate::settings::InputDeviceMode;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SessionMicTarget {
    Follow,
    Pin {
        uid: String,
    },
    /// Sticky for the rest of the recording session (02A A11).
    PinFallback {
        pinned_uid: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActiveMic {
    /// `None` means the backend was asked to follow the live OS default.
    pub(crate) uid: Option<String>,
    pub(crate) resolved_uid: String,
    pub(crate) name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PolicyMicRuntime {
    Inactive,
    Active(ActiveMic),
    Switching {
        from: Option<ActiveMic>,
        to: OpenTarget,
        manual: bool,
        old_usable: bool,
    },
    MicHolding {
        last_good: Option<ActiveMic>,
        reason: HoldingReason,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HoldingReason {
    DeviceLost,
    OpenFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PolicyState {
    pub(crate) target: SessionMicTarget,
    pub(crate) runtime: PolicyMicRuntime,
    pub(crate) latest_registry_generation: u64,
    pub(crate) last_default_uid: Option<String>,
    pub(crate) paused: bool,
    pub(crate) stopped: bool,
}

impl PolicyState {
    pub(crate) fn new(target: SessionMicTarget) -> Self {
        Self {
            target,
            runtime: PolicyMicRuntime::Inactive,
            latest_registry_generation: 0,
            last_default_uid: None,
            paused: false,
            stopped: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct OpenTarget {
    pub(crate) uid: Option<String>,
    pub(crate) resolved_uid: String,
    pub(crate) name: String,
}

impl OpenTarget {
    pub(crate) fn active(&self) -> ActiveMic {
        ActiveMic {
            uid: self.uid.clone(),
            resolved_uid: self.resolved_uid.clone(),
            name: self.name.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SwapCause {
    Start,
    Manual,
    DefaultChanged,
    ActiveFailed,
    HoldingRecovery,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PolicyAction {
    None,
    BeginMicCandidate {
        ladder: Vec<OpenTarget>,
        cause: SwapCause,
        manual: bool,
        drop_active_first: bool,
    },
    CancelCandidate,
    EnterHolding(HoldingReason),
    PersistFollow,
    PersistPin {
        uid: String,
        name: String,
    },
    PauseAndSeal,
    ResumeSegment,
    StopAndSeal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PolicyEvent {
    Start {
        mode: InputDeviceMode,
        pinned_uid: Option<String>,
        snapshot: DeviceSnapshot,
    },
    ManualSelect {
        uid: Option<String>,
        snapshot: DeviceSnapshot,
    },
    RegistrySnapshot(DeviceSnapshot),
    ActiveCaptureFailed(DeviceSnapshot),
    CandidateCommitted(OpenTarget),
    CandidateFailed {
        manual: bool,
        old_usable: bool,
        reason: HoldingReason,
        previous_target: Option<SessionMicTarget>,
    },
    Pause,
    Resume,
    Stop,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PolicyTransition {
    pub(crate) next: PolicyState,
    pub(crate) actions: Vec<PolicyAction>,
    pub(crate) ignored_stale_generation: bool,
}

impl PolicyTransition {
    fn unchanged(state: &PolicyState) -> Self {
        Self {
            next: state.clone(),
            actions: vec![PolicyAction::None],
            ignored_stale_generation: false,
        }
    }
}

pub(crate) fn target_from_settings(
    mode: InputDeviceMode,
    pinned_uid: Option<&str>,
) -> SessionMicTarget {
    match (mode, pinned_uid) {
        (InputDeviceMode::Pinned, Some(uid)) => SessionMicTarget::Pin {
            uid: uid.to_string(),
        },
        _ => SessionMicTarget::Follow,
    }
}

pub(crate) fn reduce(state: &PolicyState, event: PolicyEvent) -> PolicyTransition {
    if state.stopped && !matches!(event, PolicyEvent::Stop) {
        return PolicyTransition::unchanged(state);
    }
    match event {
        PolicyEvent::Start {
            mode,
            pinned_uid,
            snapshot,
        } => reduce_start(mode, pinned_uid, snapshot),
        PolicyEvent::ManualSelect { uid, snapshot } => reduce_manual(state, uid, snapshot),
        PolicyEvent::RegistrySnapshot(snapshot) => reduce_registry(state, snapshot),
        PolicyEvent::ActiveCaptureFailed(snapshot) => reduce_active_failure(state, snapshot),
        PolicyEvent::CandidateCommitted(target) => reduce_commit(state, target),
        PolicyEvent::CandidateFailed {
            manual,
            old_usable,
            reason,
            previous_target,
        } => reduce_candidate_failed(state, manual, old_usable, reason, previous_target),
        PolicyEvent::Pause => {
            if state.paused {
                return PolicyTransition::unchanged(state);
            }
            let mut next = state.clone();
            next.paused = true;
            next.runtime = PolicyMicRuntime::Inactive;
            let mut actions = Vec::new();
            if matches!(state.runtime, PolicyMicRuntime::Switching { .. }) {
                actions.push(PolicyAction::CancelCandidate);
            }
            actions.push(PolicyAction::PauseAndSeal);
            PolicyTransition {
                next,
                actions,
                ignored_stale_generation: false,
            }
        }
        PolicyEvent::Resume => {
            if !state.paused {
                return PolicyTransition::unchanged(state);
            }
            let mut next = state.clone();
            next.paused = false;
            PolicyTransition {
                next,
                actions: vec![PolicyAction::ResumeSegment],
                ignored_stale_generation: false,
            }
        }
        PolicyEvent::Stop => {
            if state.stopped {
                return PolicyTransition::unchanged(state);
            }
            let mut next = state.clone();
            next.stopped = true;
            next.runtime = PolicyMicRuntime::Inactive;
            let mut actions = Vec::new();
            if matches!(state.runtime, PolicyMicRuntime::Switching { .. }) {
                actions.push(PolicyAction::CancelCandidate);
            }
            actions.push(PolicyAction::StopAndSeal);
            PolicyTransition {
                next,
                actions,
                ignored_stale_generation: false,
            }
        }
    }
}

fn reduce_start(
    mode: InputDeviceMode,
    pinned_uid: Option<String>,
    snapshot: DeviceSnapshot,
) -> PolicyTransition {
    let initial_target = target_from_settings(mode, pinned_uid.as_deref());
    let mut next = PolicyState::new(initial_target.clone());
    update_generation(&mut next, &snapshot);
    let ladder = match &initial_target {
        SessionMicTarget::Follow => default_then_remaining(&snapshot),
        SessionMicTarget::Pin { uid } if snapshot.device_by_uid(uid).is_some() => {
            requested_then_default_then_remaining(&snapshot, Some(uid))
        }
        SessionMicTarget::Pin { uid } => {
            next.target = SessionMicTarget::PinFallback {
                pinned_uid: uid.clone(),
            };
            default_then_remaining(&snapshot)
        }
        SessionMicTarget::PinFallback { .. } => default_then_remaining(&snapshot),
    };
    if ladder.is_empty() {
        next.runtime = PolicyMicRuntime::MicHolding {
            last_good: None,
            reason: HoldingReason::OpenFailed,
        };
        return PolicyTransition {
            next,
            actions: vec![PolicyAction::EnterHolding(HoldingReason::OpenFailed)],
            ignored_stale_generation: false,
        };
    }
    next.runtime = PolicyMicRuntime::Switching {
        from: None,
        to: ladder[0].clone(),
        manual: false,
        old_usable: false,
    };
    PolicyTransition {
        next,
        actions: vec![PolicyAction::BeginMicCandidate {
            ladder,
            cause: SwapCause::Start,
            manual: false,
            drop_active_first: false,
        }],
        ignored_stale_generation: false,
    }
}

fn reduce_manual(
    state: &PolicyState,
    uid: Option<String>,
    snapshot: DeviceSnapshot,
) -> PolicyTransition {
    if state.paused {
        return PolicyTransition::unchanged(state);
    }
    let mut next = state.clone();
    update_generation(&mut next, &snapshot);
    next.target = uid
        .as_ref()
        .map(|uid| SessionMicTarget::Pin { uid: uid.clone() })
        .unwrap_or(SessionMicTarget::Follow);
    // Manual intent is all-or-nothing: unlike automatic recovery (T6/T8/T9/
    // T12), a failed requested candidate returns to the old usable mic.
    let ladder = match uid.as_deref() {
        Some(uid) => snapshot
            .device_by_uid(uid)
            .map(|device| {
                vec![OpenTarget {
                    uid: Some(device.uid.clone()),
                    resolved_uid: device.uid.clone(),
                    name: device.name.clone(),
                }]
            })
            .unwrap_or_default(),
        None => snapshot
            .default_device()
            .map(|device| {
                vec![OpenTarget {
                    uid: None,
                    resolved_uid: device.uid.clone(),
                    name: device.name.clone(),
                }]
            })
            .unwrap_or_default(),
    };
    if ladder.is_empty() {
        return PolicyTransition::unchanged(state);
    }
    let from = active_from_runtime(&state.runtime);
    next.runtime = PolicyMicRuntime::Switching {
        from: from.clone(),
        to: ladder[0].clone(),
        manual: true,
        old_usable: from.is_some(),
    };
    let mut actions = Vec::new();
    if matches!(
        state.runtime,
        PolicyMicRuntime::Switching { manual: false, .. }
    ) {
        actions.push(PolicyAction::CancelCandidate);
    }
    actions.push(PolicyAction::BeginMicCandidate {
        ladder,
        cause: SwapCause::Manual,
        manual: true,
        drop_active_first: false,
    });
    PolicyTransition {
        next,
        actions,
        ignored_stale_generation: false,
    }
}

fn reduce_registry(state: &PolicyState, snapshot: DeviceSnapshot) -> PolicyTransition {
    if snapshot.generation <= state.latest_registry_generation {
        let mut transition = PolicyTransition::unchanged(state);
        transition.ignored_stale_generation = true;
        return transition;
    }
    let mut next = state.clone();
    let previous_default = next.last_default_uid.clone();
    update_generation(&mut next, &snapshot);
    if state.paused
        || matches!(
            state.runtime,
            PolicyMicRuntime::Switching { manual: true, .. }
        )
    {
        return PolicyTransition {
            next,
            actions: vec![PolicyAction::None],
            ignored_stale_generation: false,
        };
    }
    if matches!(state.runtime, PolicyMicRuntime::MicHolding { .. }) {
        return holding_recovery(&next, &snapshot);
    }
    let Some(active) = active_from_runtime(&state.runtime) else {
        return PolicyTransition {
            next,
            actions: vec![PolicyAction::None],
            ignored_stale_generation: false,
        };
    };
    if !snapshot
        .devices
        .iter()
        .any(|d| d.uid == active.resolved_uid)
    {
        return reduce_active_failure(&next, snapshot);
    }
    match &state.target {
        SessionMicTarget::Pin { .. } => PolicyTransition {
            next,
            actions: vec![PolicyAction::None],
            ignored_stale_generation: false,
        },
        SessionMicTarget::Follow | SessionMicTarget::PinFallback { .. }
            if previous_default != next.last_default_uid =>
        {
            begin_auto_swap(next, &snapshot, SwapCause::DefaultChanged, false)
        }
        _ => PolicyTransition {
            next,
            actions: vec![PolicyAction::None],
            ignored_stale_generation: false,
        },
    }
}

fn reduce_active_failure(state: &PolicyState, snapshot: DeviceSnapshot) -> PolicyTransition {
    let mut next = state.clone();
    if snapshot.generation > next.latest_registry_generation {
        update_generation(&mut next, &snapshot);
    }
    if let SessionMicTarget::Pin { uid } = &state.target {
        next.target = SessionMicTarget::PinFallback {
            pinned_uid: uid.clone(),
        };
    }
    begin_auto_swap(next, &snapshot, SwapCause::ActiveFailed, true)
}

fn begin_auto_swap(
    mut next: PolicyState,
    snapshot: &DeviceSnapshot,
    cause: SwapCause,
    drop_active_first: bool,
) -> PolicyTransition {
    let ladder = default_then_remaining(snapshot);
    let from = active_from_runtime(&next.runtime);
    if ladder.is_empty() {
        next.runtime = PolicyMicRuntime::MicHolding {
            last_good: from,
            reason: HoldingReason::DeviceLost,
        };
        return PolicyTransition {
            next,
            actions: vec![PolicyAction::EnterHolding(HoldingReason::DeviceLost)],
            ignored_stale_generation: false,
        };
    }
    next.runtime = PolicyMicRuntime::Switching {
        from,
        to: ladder[0].clone(),
        manual: false,
        old_usable: !drop_active_first,
    };
    PolicyTransition {
        next,
        actions: vec![PolicyAction::BeginMicCandidate {
            ladder,
            cause,
            manual: false,
            drop_active_first,
        }],
        ignored_stale_generation: false,
    }
}

fn holding_recovery(state: &PolicyState, snapshot: &DeviceSnapshot) -> PolicyTransition {
    let ladder = holding_recovery_ladder(&state.target, snapshot);
    if ladder.is_empty() {
        return PolicyTransition {
            next: state.clone(),
            actions: vec![PolicyAction::None],
            ignored_stale_generation: false,
        };
    }
    let mut next = state.clone();
    let last_good = match &state.runtime {
        PolicyMicRuntime::MicHolding { last_good, .. } => last_good.clone(),
        _ => None,
    };
    next.runtime = PolicyMicRuntime::Switching {
        from: last_good,
        to: ladder[0].clone(),
        manual: false,
        old_usable: false,
    };
    PolicyTransition {
        next,
        actions: vec![PolicyAction::BeginMicCandidate {
            ladder,
            cause: SwapCause::HoldingRecovery,
            manual: false,
            drop_active_first: false,
        }],
        ignored_stale_generation: false,
    }
}

/// The holding timer deliberately reuses the latest generation. Keeping this
/// helper generation-agnostic lets a 5-second probe retry transient open
/// failures without pretending a topology change occurred.
pub(crate) fn holding_recovery_ladder(
    target: &SessionMicTarget,
    snapshot: &DeviceSnapshot,
) -> Vec<OpenTarget> {
    match target {
        SessionMicTarget::Pin { uid } if snapshot.device_by_uid(uid).is_some() => {
            requested_then_default_then_remaining(snapshot, Some(uid))
        }
        // A11: PinFallback never tries the saved pin in this session.
        _ => default_then_remaining(snapshot),
    }
}

fn reduce_commit(state: &PolicyState, target: OpenTarget) -> PolicyTransition {
    let mut next = state.clone();
    next.runtime = PolicyMicRuntime::Active(target.active());
    let action = match &state.runtime {
        PolicyMicRuntime::Switching { manual: true, .. } => match &state.target {
            SessionMicTarget::Follow => PolicyAction::PersistFollow,
            SessionMicTarget::Pin { uid } => PolicyAction::PersistPin {
                uid: uid.clone(),
                name: target.name,
            },
            SessionMicTarget::PinFallback { .. } => PolicyAction::None,
        },
        _ => PolicyAction::None,
    };
    PolicyTransition {
        next,
        actions: vec![action],
        ignored_stale_generation: false,
    }
}

fn reduce_candidate_failed(
    state: &PolicyState,
    manual: bool,
    old_usable: bool,
    reason: HoldingReason,
    previous_target: Option<SessionMicTarget>,
) -> PolicyTransition {
    let mut next = state.clone();
    let from = match &state.runtime {
        PolicyMicRuntime::Switching { from, .. } => from.clone(),
        _ => None,
    };
    if old_usable {
        next.runtime = from.map_or(PolicyMicRuntime::Inactive, PolicyMicRuntime::Active);
        // A failed manual selection does not change session or persisted policy.
        if manual {
            if let Some(previous_target) = previous_target {
                next.target = previous_target;
            }
        }
        return PolicyTransition {
            next,
            actions: vec![PolicyAction::None],
            ignored_stale_generation: false,
        };
    }
    next.runtime = PolicyMicRuntime::MicHolding {
        last_good: from,
        reason,
    };
    PolicyTransition {
        next,
        actions: vec![PolicyAction::EnterHolding(reason)],
        ignored_stale_generation: false,
    }
}

fn update_generation(state: &mut PolicyState, snapshot: &DeviceSnapshot) {
    state.latest_registry_generation = snapshot.generation;
    state.last_default_uid = snapshot.default_device().map(|d| d.uid.clone());
}

fn active_from_runtime(runtime: &PolicyMicRuntime) -> Option<ActiveMic> {
    match runtime {
        PolicyMicRuntime::Active(active) => Some(active.clone()),
        PolicyMicRuntime::Switching { from, .. } => from.clone(),
        PolicyMicRuntime::MicHolding { last_good, .. } => last_good.clone(),
        PolicyMicRuntime::Inactive => None,
    }
}

pub(crate) fn default_then_remaining(snapshot: &DeviceSnapshot) -> Vec<OpenTarget> {
    requested_then_default_then_remaining(snapshot, None)
}

pub(crate) fn requested_then_default_then_remaining(
    snapshot: &DeviceSnapshot,
    requested_uid: Option<&str>,
) -> Vec<OpenTarget> {
    let mut devices: Vec<(&DeviceInfo, bool)> = Vec::new();
    if let Some(uid) = requested_uid {
        if let Some(device) = snapshot.device_by_uid(uid) {
            devices.push((device, false));
        }
    }
    if let Some(device) = snapshot.default_device() {
        if !devices.iter().any(|(item, _)| item.uid == device.uid) {
            devices.push((device, true));
        }
    }
    for device in &snapshot.devices {
        if !devices.iter().any(|(item, _)| item.uid == device.uid) {
            devices.push((device, false));
        }
    }
    devices
        .into_iter()
        .map(|(device, as_default)| OpenTarget {
            uid: (!as_default).then(|| device.uid.clone()),
            resolved_uid: device.uid.clone(),
            name: device.name.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(uid: &str, default: bool) -> DeviceInfo {
        DeviceInfo {
            uid: uid.into(),
            name: uid.to_uppercase(),
            is_default: default,
            sample_rate: Some(48_000),
        }
    }

    fn snapshot(generation: u64, devices: &[(&str, bool)]) -> DeviceSnapshot {
        DeviceSnapshot {
            generation,
            devices: devices.iter().map(|(u, d)| device(u, *d)).collect(),
        }
    }

    fn active_state(target: SessionMicTarget, uid: &str, generation: u64) -> PolicyState {
        PolicyState {
            target,
            runtime: PolicyMicRuntime::Active(ActiveMic {
                uid: Some(uid.into()),
                resolved_uid: uid.into(),
                name: uid.to_uppercase(),
            }),
            latest_registry_generation: generation,
            last_default_uid: Some(uid.into()),
            paused: false,
            stopped: false,
        }
    }

    fn ladder(t: &PolicyTransition) -> &[OpenTarget] {
        match t.actions.last().unwrap() {
            PolicyAction::BeginMicCandidate { ladder, .. } => ladder,
            other => panic!("expected candidate action, got {other:?}"),
        }
    }

    #[test]
    fn t1_follow_start_opens_live_default() {
        let t = reduce(
            &PolicyState::new(SessionMicTarget::Follow),
            PolicyEvent::Start {
                mode: InputDeviceMode::FollowDefault,
                pinned_uid: None,
                snapshot: snapshot(1, &[("a", true)]),
            },
        );
        assert_eq!(t.next.target, SessionMicTarget::Follow);
        assert_eq!(ladder(&t)[0].uid, None);
    }

    #[test]
    fn t2_pinned_start_opens_present_pin() {
        let t = reduce(
            &PolicyState::new(SessionMicTarget::Follow),
            PolicyEvent::Start {
                mode: InputDeviceMode::Pinned,
                pinned_uid: Some("pin".into()),
                snapshot: snapshot(1, &[("default", true), ("pin", false)]),
            },
        );
        assert_eq!(ladder(&t)[0].uid.as_deref(), Some("pin"));
    }

    #[test]
    fn t3_missing_pin_latches_fallback_without_persisting() {
        let t = reduce(
            &PolicyState::new(SessionMicTarget::Follow),
            PolicyEvent::Start {
                mode: InputDeviceMode::Pinned,
                pinned_uid: Some("pin".into()),
                snapshot: snapshot(1, &[("default", true)]),
            },
        );
        assert!(matches!(
            t.next.target,
            SessionMicTarget::PinFallback { .. }
        ));
        assert!(!t
            .actions
            .iter()
            .any(|a| matches!(a, PolicyAction::PersistPin { .. })));
    }

    #[test]
    fn t4_manual_uid_cancels_auto_and_persists_only_after_commit() {
        let mut state = active_state(SessionMicTarget::Follow, "a", 1);
        state.runtime = PolicyMicRuntime::Switching {
            from: active_from_runtime(&state.runtime),
            to: default_then_remaining(&snapshot(2, &[("b", true)]))[0].clone(),
            manual: false,
            old_usable: true,
        };
        let t = reduce(
            &state,
            PolicyEvent::ManualSelect {
                uid: Some("pin".into()),
                snapshot: snapshot(3, &[("b", true), ("pin", false)]),
            },
        );
        assert_eq!(t.actions[0], PolicyAction::CancelCandidate);
        assert!(!t
            .actions
            .iter()
            .any(|a| matches!(a, PolicyAction::PersistPin { .. })));
        let committed = reduce(
            &t.next,
            PolicyEvent::CandidateCommitted(ladder(&t)[0].clone()),
        );
        assert!(matches!(
            committed.actions[0],
            PolicyAction::PersistPin { .. }
        ));
    }

    #[test]
    fn t5_manual_default_becomes_follow_and_persists_on_commit() {
        let state = active_state(SessionMicTarget::Pin { uid: "a".into() }, "a", 1);
        let t = reduce(
            &state,
            PolicyEvent::ManualSelect {
                uid: None,
                snapshot: snapshot(2, &[("b", true), ("a", false)]),
            },
        );
        let committed = reduce(
            &t.next,
            PolicyEvent::CandidateCommitted(ladder(&t)[0].clone()),
        );
        assert_eq!(committed.next.target, SessionMicTarget::Follow);
        assert_eq!(committed.actions, vec![PolicyAction::PersistFollow]);
    }

    #[test]
    fn t6_follow_and_fallback_track_changed_default() {
        for target in [
            SessionMicTarget::Follow,
            SessionMicTarget::PinFallback {
                pinned_uid: "p".into(),
            },
        ] {
            let state = active_state(target, "a", 1);
            let t = reduce(
                &state,
                PolicyEvent::RegistrySnapshot(snapshot(2, &[("b", true), ("a", false)])),
            );
            assert_eq!(ladder(&t)[0].resolved_uid, "b");
        }
    }

    #[test]
    fn t7_pin_ignores_default_change_while_present() {
        let state = active_state(SessionMicTarget::Pin { uid: "pin".into() }, "pin", 1);
        let t = reduce(
            &state,
            PolicyEvent::RegistrySnapshot(snapshot(2, &[("d", true), ("pin", false)])),
        );
        assert_eq!(t.actions, vec![PolicyAction::None]);
    }

    #[test]
    fn t8_pin_failure_latches_fallback_and_drops_first() {
        let state = active_state(SessionMicTarget::Pin { uid: "pin".into() }, "pin", 1);
        let t = reduce(
            &state,
            PolicyEvent::ActiveCaptureFailed(snapshot(2, &[("d", true)])),
        );
        assert!(matches!(
            t.next.target,
            SessionMicTarget::PinFallback { .. }
        ));
        assert!(matches!(
            t.actions[0],
            PolicyAction::BeginMicCandidate {
                drop_active_first: true,
                ..
            }
        ));
    }

    #[test]
    fn t9_follow_failure_uses_default_then_remaining_once() {
        let state = active_state(SessionMicTarget::Follow, "a", 1);
        let t = reduce(
            &state,
            PolicyEvent::ActiveCaptureFailed(snapshot(2, &[("b", true), ("c", false)])),
        );
        assert_eq!(
            ladder(&t)
                .iter()
                .map(|x| x.resolved_uid.as_str())
                .collect::<Vec<_>>(),
            vec!["b", "c"]
        );
    }

    #[test]
    fn t10_pin_fallback_never_auto_restores_reappeared_pin() {
        let state = active_state(
            SessionMicTarget::PinFallback {
                pinned_uid: "pin".into(),
            },
            "d",
            1,
        );
        let t = reduce(
            &state,
            PolicyEvent::RegistrySnapshot(snapshot(2, &[("d", true), ("pin", false)])),
        );
        assert_eq!(t.actions, vec![PolicyAction::None]);
    }

    #[test]
    fn t11_no_devices_enters_mic_holding() {
        let state = active_state(SessionMicTarget::Follow, "a", 1);
        let t = reduce(&state, PolicyEvent::ActiveCaptureFailed(snapshot(2, &[])));
        assert!(matches!(
            t.next.runtime,
            PolicyMicRuntime::MicHolding { .. }
        ));
    }

    #[test]
    fn t12_holding_honors_pin_only_before_fallback_latch() {
        for (target, expected) in [
            (SessionMicTarget::Pin { uid: "pin".into() }, "pin"),
            (
                SessionMicTarget::PinFallback {
                    pinned_uid: "pin".into(),
                },
                "d",
            ),
        ] {
            let mut state = PolicyState::new(target);
            state.latest_registry_generation = 1;
            state.runtime = PolicyMicRuntime::MicHolding {
                last_good: None,
                reason: HoldingReason::OpenFailed,
            };
            let t = reduce(
                &state,
                PolicyEvent::RegistrySnapshot(snapshot(2, &[("d", true), ("pin", false)])),
            );
            assert_eq!(ladder(&t)[0].resolved_uid, expected);
        }
    }

    #[test]
    fn t13_pause_and_stop_are_idempotent_and_cancel_switching() {
        let state = PolicyState {
            runtime: PolicyMicRuntime::Switching {
                from: None,
                to: default_then_remaining(&snapshot(1, &[("a", true)]))[0].clone(),
                manual: false,
                old_usable: false,
            },
            ..PolicyState::new(SessionMicTarget::Follow)
        };
        let paused = reduce(&state, PolicyEvent::Pause);
        assert_eq!(
            paused.actions,
            vec![PolicyAction::CancelCandidate, PolicyAction::PauseAndSeal]
        );
        assert_eq!(
            reduce(&paused.next, PolicyEvent::Pause).actions,
            vec![PolicyAction::None]
        );
        let stopped = reduce(&paused.next, PolicyEvent::Stop);
        assert_eq!(stopped.actions, vec![PolicyAction::StopAndSeal]);
        assert_eq!(
            reduce(&stopped.next, PolicyEvent::Stop).actions,
            vec![PolicyAction::None]
        );
    }

    #[test]
    fn fallback_latch_survives_pause_resume_holding_and_repeated_signals() {
        let mut state = active_state(
            SessionMicTarget::PinFallback {
                pinned_uid: "pin".into(),
            },
            "d",
            1,
        );
        state.runtime = PolicyMicRuntime::MicHolding {
            last_good: None,
            reason: HoldingReason::DeviceLost,
        };
        let paused = reduce(&state, PolicyEvent::Pause);
        let resumed = reduce(&paused.next, PolicyEvent::Resume);
        assert!(matches!(
            resumed.next.target,
            SessionMicTarget::PinFallback { .. }
        ));
        let once = reduce(
            &state,
            PolicyEvent::RegistrySnapshot(snapshot(2, &[("d", true), ("pin", false)])),
        );
        assert_eq!(ladder(&once)[0].resolved_uid, "d");
        let stale = reduce(
            &once.next,
            PolicyEvent::RegistrySnapshot(snapshot(2, &[("pin", true)])),
        );
        assert!(stale.ignored_stale_generation);
    }

    #[test]
    fn coalesced_registry_generation_uses_latest_snapshot_directly() {
        let state = active_state(SessionMicTarget::Follow, "a", 3);

        let transition = reduce(
            &state,
            PolicyEvent::RegistrySnapshot(snapshot(9, &[("latest", true), ("a", false)])),
        );

        assert_eq!(transition.next.latest_registry_generation, 9);
        assert_eq!(ladder(&transition)[0].resolved_uid, "latest");
    }

    #[test]
    fn automatic_fallback_transitions_never_request_persistence() {
        let state = active_state(SessionMicTarget::Pin { uid: "pin".into() }, "pin", 1);
        let failed = reduce(
            &state,
            PolicyEvent::ActiveCaptureFailed(snapshot(2, &[("d", true)])),
        );
        let committed = reduce(
            &failed.next,
            PolicyEvent::CandidateCommitted(ladder(&failed)[0].clone()),
        );

        assert!(matches!(
            committed.next.target,
            SessionMicTarget::PinFallback { .. }
        ));
        assert_eq!(committed.actions, vec![PolicyAction::None]);
    }

    #[test]
    fn failed_manual_candidate_returns_to_old_active_without_policy_persist() {
        let state = active_state(SessionMicTarget::Follow, "old", 1);
        let switching = reduce(
            &state,
            PolicyEvent::ManualSelect {
                uid: Some("new".into()),
                snapshot: snapshot(2, &[("old", true), ("new", false)]),
            },
        );
        let failed = reduce(
            &switching.next,
            PolicyEvent::CandidateFailed {
                manual: true,
                old_usable: true,
                reason: HoldingReason::OpenFailed,
                previous_target: Some(SessionMicTarget::Follow),
            },
        );
        assert!(matches!(failed.next.runtime, PolicyMicRuntime::Active(_)));
        assert_eq!(failed.next.target, SessionMicTarget::Follow);
        assert_eq!(failed.actions, vec![PolicyAction::None]);
    }
}
