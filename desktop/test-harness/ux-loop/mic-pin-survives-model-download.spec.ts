import { expect, test, type Page } from "@playwright/test";

// Regression tests for two related native failures:
//
// 1. Explicit mic pin resets to "Follow system default" when model
//    download/warmup triggers a settings reload (onSettingsChanged →
//    reloadSettingsAndRender). Fix: pendingDeviceSelection overlays the
//    unsaved pick in settingsRenderContext() until Save or overlay close.
//
// 2. The app's own "margins-tap" virtual loopback device appeared in the
//    mic picker and could be committed as the pinned microphone when the
//    device list re-enumerated during tap creation mid-capture. Fix: filter
//    tap devices at both the Rust enumeration boundary and the frontend
//    render.

async function openAudioSettings(page: Page) {
  await page.goto("/?scenario=settings-audio");
  await page.waitForFunction(() => Boolean(window.__marginsMock?.getState?.()));
  await expect(page.locator("#input-device-name")).toBeVisible();
}

test("pinned mic survives a settings-changed reload triggered during model download", async ({ page }) => {
  await openAudioSettings(page);

  const picker = page.locator("#input-device-name");

  // Confirm picker starts at follow-default
  await expect(picker).toHaveValue("");

  // Pin to Yeti Stereo Microphone (device uid from mock state)
  const yetiUid = "mock-yeti";
  await picker.selectOption(yetiUid);
  await expect(picker).toHaveValue(yetiUid);

  // Simulate what happens when model download completes: backend writes
  // parakeet_model_dir to settings.json → onSettingsChanged fires →
  // reloadSettingsAndRender() reloads settings from "disk" (mock state) and
  // calls render(). Before the fix this wiped the unsaved pin.
  await page.evaluate(() => {
    window.__marginsMock?.simulateSettingsChanged?.();
  });

  // Wait one frame for any async re-render to settle
  await page.waitForTimeout(100);

  // Pin must still be selected after the reload
  await expect(picker).toHaveValue(yetiUid);
});

test("picker resets to follow-default when overlay closes without saving", async ({ page }) => {
  await openAudioSettings(page);

  const picker = page.locator("#input-device-name");
  await picker.selectOption("mock-yeti");

  // Close settings without saving
  await page.locator(".settings-close").click();
  await expect(page.locator("#settings-dialog")).not.toBeVisible();

  // Re-open settings
  await page.locator('button[aria-label="Setup"]').click();
  await expect(page.locator("#settings-dialog")).toBeVisible();

  // Picker must reset to follow-default (persisted value), not the old unsaved pin
  await expect(page.locator("#input-device-name")).toHaveValue("");
});

test("picker UID stays stable when device list changes order and membership", async ({ page }) => {
  await openAudioSettings(page);

  const picker = page.locator("#input-device-name");
  const yetiUid = "mock-yeti";

  // Pin to Yeti
  await picker.selectOption(yetiUid);
  await expect(picker).toHaveValue(yetiUid);

  // Simulate what happens when the tap is created: device list re-enumerates
  // with a new entry at the top (different order) and the tap filtered out.
  // The Yeti UID must still be selected by UID, not by index.
  await page.evaluate(() => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const newDevices: any[] = [
      { uid: "mock-new-usb", name: "USB Headset", is_default: false, sample_rate: 48000 },
      { uid: "mock-default", name: "System Default Microphone", is_default: true, sample_rate: 48000 },
      // Yeti is now at position 2 instead of position 1 — UID must resolve it
      { uid: "mock-yeti", name: "Yeti Stereo Microphone", is_default: false, sample_rate: 48000 },
      { uid: "mock-blackhole", name: "BlackHole 2ch", is_default: false, sample_rate: 48000 },
      // Note: no "margins-tap" — it must be filtered before reaching here
    ];
    window.__marginsMock?.simulateDevicesChanged?.(newDevices);
    // Also fire settings-changed so reloadSettingsAndRender runs (as it would
    // when the model finishes downloading mid-capture).
    window.__marginsMock?.simulateSettingsChanged?.();
  });

  await page.waitForTimeout(100);

  // Yeti must still be selected despite list reorder
  await expect(picker).toHaveValue(yetiUid);
});

test("tap device (margins-tap) is absent from the mic picker", async ({ page }) => {
  await openAudioSettings(page);

  // Inject a device list that contains the tap device — simulates the CoreAudio
  // enumeration that includes it before Rust-side filtering. The frontend guard
  // must catch it even if the Rust filter somehow passes it through.
  await page.evaluate(() => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const devicesWithTap: any[] = [
      { uid: "mock-default", name: "System Default Microphone", is_default: true, sample_rate: 48000 },
      { uid: "mock-yeti", name: "Yeti Stereo Microphone", is_default: false, sample_rate: 48000 },
      { uid: "tap-uid", name: "margins-tap", is_default: false, sample_rate: 48000 },
    ];
    window.__marginsMock?.simulateDevicesChanged?.(devicesWithTap);
  });

  await page.waitForTimeout(100);

  // "margins-tap" must not appear as an option in the mic picker
  const tapOption = page.locator("#input-device-name option[value='tap-uid']");
  await expect(tapOption).toHaveCount(0);

  // Real mics must still be present
  await expect(page.locator("#input-device-name option[value='mock-yeti']")).toHaveCount(1);
});
