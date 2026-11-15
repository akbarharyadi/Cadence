import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test, vi } from "vitest";
import ModalHarness from "./ModalHarness.svelte";

function setup() {
  const onOuterClose = vi.fn();
  const onInnerClose = vi.fn();
  render(ModalHarness, { onOuterClose, onInnerClose });
  return { onOuterClose, onInnerClose, user: userEvent.setup() };
}

describe("Escape in dialogs", () => {
  test("with a dropdown open inside a dialog: the first Escape closes only the dropdown, the second the dialog", async () => {
    const { onOuterClose, user } = setup();

    await user.click(screen.getByRole("button", { name: "Sort by" }));
    expect(screen.getByRole("listbox")).toBeInTheDocument();

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "Outer dialog" })).toBeInTheDocument();
    expect(onOuterClose).not.toHaveBeenCalled();

    await user.keyboard("{Escape}");
    expect(onOuterClose).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  test("with two dialogs stacked: Escape closes only the top one", async () => {
    const { onOuterClose, onInnerClose, user } = setup();

    await user.click(screen.getByRole("button", { name: "Open second dialog" }));
    expect(screen.getAllByRole("dialog")).toHaveLength(2);

    await user.keyboard("{Escape}");
    expect(onInnerClose).toHaveBeenCalledTimes(1);
    expect(onOuterClose).not.toHaveBeenCalled();
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
    expect(screen.getByRole("dialog", { name: "Outer dialog" })).toBeInTheDocument();

    await user.keyboard("{Escape}");
    expect(onOuterClose).toHaveBeenCalledTimes(1);
  });
});

describe("Focus around dialogs", () => {
  test("closing a dialog gives the keyboard back to what had it when it opened", async () => {
    const before = document.createElement("button");
    before.textContent = "Opener";
    document.body.appendChild(before);
    before.focus();
    const { user } = setup();
    // The dialog has the keyboard while open.
    expect(before).not.toHaveFocus();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(before).toHaveFocus();
    before.remove();
  });

  test("a dialog opened from another: closing it goes back to the button in the first", async () => {
    const { user } = setup();
    const opener = screen.getByRole("button", { name: "Open second dialog" });
    await user.click(opener);
    expect(screen.getAllByRole("dialog")).toHaveLength(2);
    await user.keyboard("{Escape}");
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
    expect(opener).toHaveFocus();
  });

  test("focus moved elsewhere on purpose while closing is left there", async () => {
    const before = document.createElement("button");
    const elsewhere = document.createElement("input");
    document.body.append(before, elsewhere);
    before.focus();
    const { user, onOuterClose } = setup();
    onOuterClose.mockImplementation(() => elsewhere.focus());
    await user.keyboard("{Escape}");
    expect(elsewhere).toHaveFocus();
    before.remove();
    elsewhere.remove();
  });

  test("a dropdown inside a dialog opens as its own layer, so the dialog's scrolling can't cut it off", async () => {
    const { user } = setup();
    await user.click(screen.getByRole("button", { name: "Sort by" }));
    expect(screen.getByRole("listbox").style.position).toBe("fixed");
  });
});
