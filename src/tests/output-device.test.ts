import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import OutputDevice from "#lib/components/OutputDevice.svelte";
import SignalFlow from "#lib/components/SignalFlow.svelte";
import { deviceName, shortDeviceName } from "#lib/devices.js";
import type { OutputDevices, Status } from "#lib/api.js";
import { app } from "#lib/store.svelte.js";
import { makeStatus, mockBackend, resetApp, settle } from "./helpers";

/** What `list_output_devices` answers: a DAC, the HDMI monitor that's the default, unplugged headphones. */
function devices(over: Partial<OutputDevices> = {}): OutputDevices {
  return {
    supported: true,
    devices: [
      { id: "wasapi:dac", name: "Speakers (FiiO K7)", isDefault: false, present: true },
      { id: "wasapi:hdmi", name: "LG TV (NVIDIA High Definition Audio)", isDefault: true, present: true },
      { id: "wasapi:hp", name: "Headphones (USB Audio)", isDefault: false, present: false },
    ],
    chosen: null,
    profiles: true,
    ...over,
  };
}

const onDac = (over: Partial<Status> = {}) => makeStatus({ device: { id: "wasapi:dac", name: "Speakers (FiiO K7)", chosen: true, profile: 1 }, ...over });

async function setup(list: OutputDevices, status: Status | null = onDac()) {
  const backend = mockBackend({ list_output_devices: () => list });
  resetApp({ status });
  render(OutputDevice, { windows: true });
  await settle();
  return { backend, user: userEvent.setup() };
}

describe("Output device picker", () => {
  test("lists the devices, marking the system default and the unplugged one", async () => {
    const { user } = await setup(devices());
    const picker = screen.getByRole("button", { name: "Output device" });
    expect(picker.textContent).toContain("System default (follow Windows)");
    await user.click(picker);
    const options = screen.getAllByRole("option").map((o) => o.textContent?.trim());
    expect(options).toEqual([
      "System default (follow Windows)",
      "Speakers (FiiO K7)",
      "LG TV (NVIDIA High Definition Audio) (system default)",
      "Headphones (USB Audio) (not connected)",
    ]);
    expect(screen.getByText("Now: LG TV (NVIDIA High Definition Audio).")).toBeTruthy();
  });

  test("choosing a device plays on it; choosing System default goes back to following it", async () => {
    const { backend, user } = await setup(devices());
    await user.click(screen.getByRole("button", { name: "Output device" }));
    await user.click(screen.getByRole("option", { name: /FiiO K7/ }));
    expect(backend.last("set_output_device")).toEqual({ device: { id: "wasapi:dac", name: "Speakers (FiiO K7)" } });
    expect(screen.getByRole("button", { name: "Output device" }).textContent).toContain("Speakers (FiiO K7)");

    await user.click(screen.getByRole("button", { name: "Output device" }));
    await user.click(screen.getByRole("option", { name: /System default/ }));
    expect(backend.last("set_output_device")).toEqual({ device: null });
  });

  test("a chosen device that isn't connected is shown as chosen and not connected", async () => {
    await setup(devices({ chosen: { id: "wasapi:hp", name: "Headphones (USB Audio)" } }));
    expect(screen.getByRole("button", { name: "Output device" }).textContent).toContain("Headphones (USB Audio) (not connected)");
  });

  test("says whose sound settings the page changes, and the switch turns that off", async () => {
    const { backend, user } = await setup(devices());
    expect(screen.getByText("Editing the sound settings for Speakers (FiiO K7).")).toBeTruthy();
    await user.click(screen.getByRole("switch", { name: "Remember sound settings per device" }));
    expect(backend.last("set_device_profiles")).toEqual({ enabled: false });
    expect(screen.queryByText(/Editing the sound settings for/)).toBeNull();
  });

  test("an unplugged DAC: paused, with Play on the default and Keep waiting", async () => {
    const { backend, user } = await setup(devices(), onDac({ deviceMissing: { name: "Speakers (FiiO K7)", fallback: "Speakers (Realtek(R) Audio)", resume: true } }));
    expect(screen.getByRole("status").textContent).toContain("isn’t connected, so playback is paused");
    expect(screen.getByRole("status").textContent).toContain("within two minutes");
    await user.click(screen.getByRole("button", { name: "Play on Speakers (Realtek(R) Audio)" }));
    expect(backend.to("output_use_default")).toHaveLength(1);
    await user.click(screen.getByRole("button", { name: "Keep waiting" }));
    expect(screen.queryByRole("status")).toBeNull();
  });

  test("the list is read again when the system's devices change, not on every status update", async () => {
    const { backend } = await setup(devices(), onDac({ devices: 1 }));
    expect(backend.to("list_output_devices")).toHaveLength(1);
    app.status = onDac({ devices: 1, position: 3 });
    await settle();
    expect(backend.to("list_output_devices")).toHaveLength(1);
    app.status = onDac({ devices: 2 });
    await settle();
    expect(backend.to("list_output_devices")).toHaveLength(2);
  });

  test("a change while the list is being read is read once more after it (never dropped, never piled up)", async () => {
    let release!: () => void;
    let calls = 0;
    const backend = mockBackend({
      list_output_devices: () => {
        calls++;
        return calls === 1 ? devices() : new Promise<OutputDevices>((r) => (release = () => r(devices({ chosen: { id: "wasapi:dac", name: "Speakers (FiiO K7)" } }))));
      },
    });
    resetApp({ status: onDac({ devices: 1 }) });
    render(OutputDevice, { windows: true });
    await settle();
    app.status = onDac({ devices: 2 });
    await settle();
    expect(backend.to("list_output_devices")).toHaveLength(2);
    // Two more changes while that read is still out: one more read after it, not two.
    app.status = onDac({ devices: 3 });
    await settle();
    app.status = onDac({ devices: 4 });
    await settle();
    expect(backend.to("list_output_devices")).toHaveLength(2);
    release();
    await settle();
    release();
    await settle();
    expect(backend.to("list_output_devices")).toHaveLength(3);
    expect(screen.getByRole("button", { name: "Output device" }).textContent).toContain("Speakers (FiiO K7)");
  });

  test("the first status after the page opened doesn't read the list a second time", async () => {
    const backend = mockBackend({ list_output_devices: () => devices() });
    resetApp({ status: null });
    render(OutputDevice, { windows: true });
    await settle();
    expect(backend.to("list_output_devices")).toHaveLength(1);
    app.status = onDac({ devices: 5 });
    await settle();
    expect(backend.to("list_output_devices")).toHaveLength(1);
    app.status = onDac({ devices: 6 });
    await settle();
    expect(backend.to("list_output_devices")).toHaveLength(2);
  });

  test("odd or empty device names don't break the page", async () => {
    await setup(devices({ devices: [{ id: "x", name: "", isDefault: true, present: true }, { id: "y", name: "\u0007 ヘッドホン (USB DAC) ", isDefault: false, present: true }] }), makeStatus({ device: { id: "x", name: "", chosen: false, profile: 0 } }));
    expect(screen.getByText("Now: Unnamed device.")).toBeTruthy();
    expect(screen.getByText("Editing the sound settings for Unnamed device.")).toBeTruthy();
    expect(deviceName(" \u0000 ")).toBe("Unnamed device");
    expect(shortDeviceName("Speakers (FiiO K7)")).toBe("FiiO K7");
    expect(shortDeviceName("LG TV (NVIDIA High Definition Audio)")).toBe("LG TV (NVIDIA High Definition Audio)");
    expect(shortDeviceName("\u0007 ヘッドホン (USB DAC) ")).toBe("ヘッドホン (USB DAC)");
  });

  test("hidden where Cadence doesn't choose the device (Android)", async () => {
    await setup(devices({ supported: false, devices: [] }));
    expect(screen.queryByRole("button", { name: "Output device" })).toBeNull();
  });
});

describe("Signal path", () => {
  test("names the device in use", async () => {
    mockBackend();
    const status = onDac({ queue: ["t1"], index: 0, duration: 200 });
    status.output = { ...status.output, device: "Speakers (FiiO K7)", mode: "exclusive", bits: 24 };
    resetApp({ status });
    render(SignalFlow);
    await settle();
    const value = screen.getByTitle("Speakers (FiiO K7)");
    expect(value.textContent).toBe("FiiO K7");
  });
});
