// Runs before every test file (vite.config.js > test.setupFiles).
import "@testing-library/jest-dom/vitest";
import { randomFillSync } from "node:crypto";
import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeAll } from "vitest";

beforeAll(() => {
  // Tauri's mockIPC numbers its callbacks with crypto.getRandomValues; some jsdom versions have none.
  if (!globalThis.crypto?.getRandomValues) {
    Object.defineProperty(globalThis, "crypto", {
      configurable: true,
      value: { getRandomValues: <T extends ArrayBufferView>(buffer: T) => randomFillSync(buffer as never) },
    });
  }

  // jsdom has no layout engine and lacks a few browser APIs the components use. Nothing here
  // pretends to measure anything: tests that need sizes stub getBoundingClientRect themselves.
  window.matchMedia ??= ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia;
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.scrollIntoView ??= function scrollIntoView() {};
  Element.prototype.scrollTo ??= function scrollTo() {};
});

afterEach(() => {
  // Undo mockIPC's hooks on window.__TAURI_INTERNALS__ so one test's backend never answers another's.
  clearMocks();
  localStorage.clear();
  delete (window as unknown as { CadenceAndroid?: unknown }).CadenceAndroid;
});
