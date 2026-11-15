import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import ContextMenu from "#lib/components/ContextMenu.svelte";
import { app } from "#lib/store.svelte.js";
import { mockBackend, resetApp, settle } from "./helpers";

// jsdom has no layout: every box is 0 x 0. ContextMenu places itself from its own measured size and
// the window's, so the menu is given a size here (200 x 150) and the window a small one (400 x 300).
// What's under test is the placement rule, not the measuring.
const WINDOW = { w: 400, h: 300 };
let menuSize = { w: 200, h: 150 };
let realRect: typeof HTMLElement.prototype.getBoundingClientRect;
let realSize: { w: number; h: number };

beforeEach(() => {
  mockBackend();
  resetApp();
  menuSize = { w: 200, h: 150 };
  realSize = { w: window.innerWidth, h: window.innerHeight };
  Object.assign(window, { innerWidth: WINDOW.w, innerHeight: WINDOW.h });
  realRect = HTMLElement.prototype.getBoundingClientRect;
  HTMLElement.prototype.getBoundingClientRect = function (this: HTMLElement) {
    if (!this.classList.contains("menu")) return realRect.call(this);
    return { x: 0, y: 0, left: 0, top: 0, right: menuSize.w, bottom: menuSize.h, width: menuSize.w, height: menuSize.h, toJSON() {} } as DOMRect;
  };
});
afterEach(() => {
  HTMLElement.prototype.getBoundingClientRect = realRect;
  Object.assign(window, { innerWidth: realSize.w, innerHeight: realSize.h });
});

const items = (n = 4) => Array.from({ length: n }, (_, i) => ({ label: `Item ${i + 1}`, action: vi.fn() }));

async function openMenu(x: number, y: number, list = items()) {
  render(ContextMenu);
  app.menu = { x, y, items: list };
  await settle();
  const menu = screen.getByRole("menu");
  return { menu, left: parseFloat(menu.style.left), top: parseFloat(menu.style.top), list };
}
const inside = (left: number, top: number) => {
  expect(left).toBeGreaterThanOrEqual(0);
  expect(top).toBeGreaterThanOrEqual(0);
  expect(left + menuSize.w).toBeLessThanOrEqual(WINDOW.w);
  expect(top + menuSize.h).toBeLessThanOrEqual(WINDOW.h);
};

describe("ContextMenu placement", () => {
  test("opened in the bottom-right corner it opens up and to the left, inside the window", async () => {
    const { left, top } = await openMenu(395, 295);
    inside(left, top);
    expect(left).toBeLessThan(395); // it moved left of the pointer
    expect(top).toBeLessThan(295); // and opened upwards from it
  });

  test("opened at the very top-left it stays inside the window", async () => {
    const { left, top } = await openMenu(2, 1);
    inside(left, top);
  });

  test("opened near the top with room below it stays where it was opened", async () => {
    const { left, top } = await openMenu(50, 40);
    expect([left, top]).toEqual([50, 40]);
  });

  test("a menu taller than the window starts at the top edge (and scrolls)", async () => {
    menuSize = { w: 200, h: 500 };
    const { top } = await openMenu(100, 200);
    expect(top).toBeGreaterThanOrEqual(0);
    expect(top).toBeLessThanOrEqual(8);
  });
});

describe("ContextMenu keyboard", () => {
  test("the first item has the keyboard; ArrowDown / ArrowUp move it (and wrap)", async () => {
    const user = userEvent.setup();
    await openMenu(50, 50);
    const [one, two, , four] = screen.getAllByRole("menuitem");
    expect(one).toHaveFocus();

    await user.keyboard("{ArrowDown}");
    expect(two).toHaveFocus();
    await user.keyboard("{ArrowUp}");
    expect(one).toHaveFocus();
    await user.keyboard("{ArrowUp}");
    expect(four).toHaveFocus(); // wraps to the last
    await user.keyboard("{ArrowDown}");
    expect(one).toHaveFocus(); // and back to the first
  });

  test("Escape closes the menu and gives the keyboard back to what opened it", async () => {
    const user = userEvent.setup();
    const opener = document.createElement("button");
    opener.textContent = "More";
    document.body.append(opener);
    opener.focus();

    await openMenu(50, 50);
    expect(screen.getAllByRole("menuitem")[0]).toHaveFocus();

    await user.keyboard("{Escape}");
    await settle();
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(app.menu).toBeNull();
    expect(opener).toHaveFocus();
    opener.remove();
  });

  test("choosing an item runs it and closes the menu", async () => {
    const user = userEvent.setup();
    const { list } = await openMenu(50, 50);
    await user.click(screen.getByRole("menuitem", { name: "Item 2" }));
    expect(list[1].action).toHaveBeenCalledTimes(1);
    expect(app.menu).toBeNull();
  });
});
