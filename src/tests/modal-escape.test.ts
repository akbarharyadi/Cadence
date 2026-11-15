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
