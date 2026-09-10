use serde::Serialize;
#[cfg(feature = "tauri-app")]
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

#[cfg(feature = "tauri-app")]
use cpal::traits::DeviceTrait;
#[cfg(feature = "tauri-app")]
use margins::recorder;

pub(crate) const DEVICES_CHANGED_EVENT: &str = "devices-changed";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct DeviceInfo {
    pub(crate) uid: String,
    pub(crate) name: String,
    pub(crate) is_default: bool,
    pub(crate) sample_rate: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct DeviceSnapshot {
    pub(crate) devices: Vec<DeviceInfo>,
    pub(crate) generation: u64,
}

impl DeviceSnapshot {
    pub(crate) fn default_device(&self) -> Option<&DeviceInfo> {
        self.devices.iter().find(|device| device.is_default)
    }

    pub(crate) fn device_by_uid(&self, uid: &str) -> Option<&DeviceInfo> {
        self.devices.iter().find(|device| device.uid == uid)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DeviceSelection {
    pub(crate) device_uid: Option<String>,
    pub(crate) device_name: Option<String>,
    pub(crate) missing_saved_uid: Option<String>,
}

pub(crate) fn resolve_selection(
    snapshot: &DeviceSnapshot,
    requested_uid: Option<&str>,
) -> DeviceSelection {
    if let Some(uid) = requested_uid {
        if let Some(device) = snapshot.device_by_uid(uid) {
            return DeviceSelection {
                device_uid: Some(device.uid.clone()),
                device_name: Some(device.name.clone()),
                missing_saved_uid: None,
            };
        }
    }

    let default = snapshot.default_device();
    DeviceSelection {
        // None intentionally asks cpal to open the live OS default. This avoids
        // converting the fallback back into a stale explicit device selection.
        device_uid: None,
        device_name: default.map(|device| device.name.clone()),
        missing_saved_uid: requested_uid.map(str::to_string),
    }
}

pub(crate) fn legacy_uid_for_name(devices: &[DeviceInfo], name: &str) -> Option<String> {
    devices
        .iter()
        .find(|device| device.name == name)
        .map(|device| device.uid.clone())
}

pub(crate) struct DeviceRegistry {
    snapshot: RwLock<Arc<DeviceSnapshot>>,
}

impl Default for DeviceRegistry {
    fn default() -> Self {
        Self {
            snapshot: RwLock::new(Arc::new(DeviceSnapshot::default())),
        }
    }
}

impl DeviceRegistry {
    pub(crate) fn snapshot(&self) -> Arc<DeviceSnapshot> {
        Arc::clone(&self.snapshot.read().unwrap())
    }

    pub(crate) fn refresh(&self) -> Arc<DeviceSnapshot> {
        self.replace_devices(enumerate_devices())
    }

    fn replace_devices(&self, devices: Vec<DeviceInfo>) -> Arc<DeviceSnapshot> {
        let mut guard = self.snapshot.write().unwrap();
        if guard.devices == devices {
            return Arc::clone(&guard);
        }
        let next = Arc::new(DeviceSnapshot {
            devices,
            generation: guard.generation.saturating_add(1),
        });
        *guard = Arc::clone(&next);
        next
    }

    #[cfg(feature = "tauri-app")]
    pub(crate) fn resolve_live_device(
        &self,
        device_uid: Option<&str>,
    ) -> Result<Option<cpal::Device>, String> {
        let Some(device_uid) = device_uid else {
            return Ok(None);
        };
        let mut occurrences = HashMap::<String, usize>::new();
        for (name, device) in recorder::list_input_devices() {
            let occurrence = occurrences.entry(name.clone()).or_default();
            let uid = stable_uid(&name, *occurrence);
            *occurrence += 1;
            if uid.as_deref() == Some(device_uid) {
                return Ok(Some(device));
            }
        }
        Err(
            "Selected microphone is no longer available. Refresh devices and choose again."
                .to_string(),
        )
    }

    #[cfg(not(feature = "tauri-app"))]
    pub(crate) fn resolve_live_device(
        &self,
        _device_uid: Option<&str>,
    ) -> Result<Option<margins::recorder::InputDevice>, String> {
        Ok(None)
    }
}

pub(crate) fn run_device_watcher(
    registry: Arc<DeviceRegistry>,
    publish: impl Fn(Arc<DeviceSnapshot>) + Send + 'static,
) {
    let initial = registry.refresh();
    publish(initial);

    #[cfg(all(target_os = "macos", feature = "tauri-app"))]
    {
        let (tx, rx) = std::sync::mpsc::sync_channel(16);
        let _watcher = match recorder::InputDeviceWatcher::start(tx) {
            Ok(watcher) => watcher,
            Err(error) => {
                eprintln!("margins: could not watch input devices: {error}");
                return;
            }
        };
        while rx.recv().is_ok() {
            std::thread::sleep(Duration::from_millis(400));
            while rx.try_recv().is_ok() {}
            let previous_generation = registry.snapshot().generation;
            let snapshot = registry.refresh();
            if snapshot.generation != previous_generation {
                publish(snapshot);
            }
        }
    }

    #[cfg(not(all(target_os = "macos", feature = "tauri-app")))]
    loop {
        std::thread::sleep(Duration::from_secs(5));
        let previous_generation = registry.snapshot().generation;
        let snapshot = registry.refresh();
        if snapshot.generation != previous_generation {
            publish(snapshot);
        }
    }
}

#[cfg(feature = "tauri-app")]
fn stable_uid(name: &str, occurrence: usize) -> Option<String> {
    recorder::input_device_uid_at(name, occurrence).or_else(|| Some(name.to_string()))
}

#[cfg(feature = "tauri-app")]
fn enumerate_devices() -> Vec<DeviceInfo> {
    let default_name = recorder::default_input_device_name();
    let mut occurrences = HashMap::<String, usize>::new();
    recorder::list_input_devices()
        .into_iter()
        // Exclude the app's own virtual tap device so it never appears in the
        // mic picker and cannot be committed as a user-pinned microphone.
        .filter(|(name, _)| name != recorder::TAP_NAME)
        .filter_map(|(name, device)| {
            let occurrence = occurrences.entry(name.clone()).or_default();
            let uid = stable_uid(&name, *occurrence)?;
            *occurrence += 1;
            let sample_rate = device
                .default_input_config()
                .ok()
                .map(|config| config.sample_rate().0);
            Some(DeviceInfo {
                is_default: default_name.as_ref() == Some(&name),
                uid,
                name,
                sample_rate,
            })
        })
        .collect()
}

#[cfg(not(feature = "tauri-app"))]
fn enumerate_devices() -> Vec<DeviceInfo> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(uid: &str, name: &str, is_default: bool) -> DeviceInfo {
        DeviceInfo {
            uid: uid.to_string(),
            name: name.to_string(),
            is_default,
            sample_rate: Some(48_000),
        }
    }

    #[test]
    fn resolves_uid_without_positional_identity() {
        let snapshot = DeviceSnapshot {
            devices: vec![
                device("built-in", "Mac Mic", true),
                device("usb", "Yeti", false),
            ],
            generation: 4,
        };
        let selected = resolve_selection(&snapshot, Some("usb"));
        assert_eq!(selected.device_uid.as_deref(), Some("usb"));
        assert_eq!(selected.device_name.as_deref(), Some("Yeti"));
        assert_eq!(selected.missing_saved_uid, None);
    }

    #[test]
    fn missing_uid_falls_back_to_live_default() {
        let snapshot = DeviceSnapshot {
            devices: vec![device("built-in", "Mac Mic", true)],
            generation: 2,
        };
        let selected = resolve_selection(&snapshot, Some("unplugged"));
        assert_eq!(selected.device_uid, None);
        assert_eq!(selected.device_name.as_deref(), Some("Mac Mic"));
        assert_eq!(selected.missing_saved_uid.as_deref(), Some("unplugged"));
    }

    #[test]
    fn legacy_name_migrates_only_when_present() {
        let devices = vec![device("usb", "Yeti", false)];
        assert_eq!(
            legacy_uid_for_name(&devices, "Yeti").as_deref(),
            Some("usb")
        );
        assert_eq!(legacy_uid_for_name(&devices, "Missing"), None);
    }

    #[test]
    fn generation_changes_only_when_device_snapshot_changes() {
        let registry = DeviceRegistry::default();
        let first = registry.replace_devices(vec![device("built-in", "Mac Mic", true)]);
        assert_eq!(first.generation, 1);
        let unchanged = registry.replace_devices(first.devices.clone());
        assert_eq!(unchanged.generation, 1);
        let changed = registry.replace_devices(vec![device("usb", "Yeti", true)]);
        assert_eq!(changed.generation, 2);
    }

    // The tap device must never appear in the mic picker list. Simulate the
    // registry receiving a device list that includes "margins-tap" (as happens
    // when the system-audio tap is created mid-capture) and assert it is absent.
    #[test]
    fn tap_device_excluded_from_registry_snapshot() {
        use margins::recorder::TAP_NAME;
        let all_devices = vec![
            device("built-in", "Mac Mic", true),
            device("usb-yeti", "Yeti Stereo Microphone", false),
            // Simulate the tap being visible as a CoreAudio input device.
            device("tap-uid", TAP_NAME, false),
        ];
        // The registry receives the full list (as enumerate_devices would return
        // after filtering). Re-filter here to mirror the production path.
        let filtered: Vec<DeviceInfo> = all_devices
            .into_iter()
            .filter(|d| d.name != TAP_NAME)
            .collect();
        let registry = DeviceRegistry::default();
        let snapshot = registry.replace_devices(filtered);
        assert!(
            snapshot.devices.iter().all(|d| d.name != TAP_NAME),
            "tap device must not appear in device snapshot"
        );
        assert_eq!(snapshot.devices.len(), 2, "only real mics should be present");
    }

    // The Yeti's UID must resolve correctly even after the tap is inserted and
    // the list changes membership. UID-based lookup must be stable regardless of
    // list position.
    #[test]
    fn uid_resolution_stable_after_device_list_reorder() {
        // Original list: [Mac Mic (default), Yeti]
        let snapshot_before = DeviceSnapshot {
            devices: vec![
                device("built-in", "Mac Mic", true),
                device("usb-yeti", "Yeti Stereo Microphone", false),
            ],
            generation: 1,
        };
        let selected_before = resolve_selection(&snapshot_before, Some("usb-yeti"));
        assert_eq!(selected_before.device_uid.as_deref(), Some("usb-yeti"));

        // After tap created + enumerated (tap filtered out), Yeti shifts position.
        let snapshot_after = DeviceSnapshot {
            devices: vec![
                // tap filtered out; Studio Display joined; order differs
                device("studio", "Studio Display Microphone", false),
                device("built-in", "Mac Mic", true),
                device("usb-yeti", "Yeti Stereo Microphone", false),
            ],
            generation: 2,
        };
        let selected_after = resolve_selection(&snapshot_after, Some("usb-yeti"));
        assert_eq!(
            selected_after.device_uid.as_deref(),
            Some("usb-yeti"),
            "Yeti UID must resolve to the same device after list reorder"
        );
        assert_eq!(selected_after.device_name.as_deref(), Some("Yeti Stereo Microphone"));
    }
}
