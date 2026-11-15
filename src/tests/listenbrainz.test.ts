import { cleanup, render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { emit } from "@tauri-apps/api/event";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import ListenBrainzTools from "#lib/components/ListenBrainzTools.svelte";
import NewReleasesSection from "#lib/components/NewReleases.svelte";
import type { LbImportStatus, LbSyncPreview, NewRelease, NewReleases } from "#lib/api.js";
import { mockBackend, settle } from "./helpers";

afterEach(() => cleanup());

/** Numbers as the page writes them (in the person's language). */
const num = (v: number) => v.toLocaleString();

const idle: LbImportStatus = { phase: "idle", connected: true, seen: 0, matched: 0, unmatched: 0, ambiguous: 0, summary: null, applied: null, message: "" };
const scanning: LbImportStatus = { ...idle, phase: "scanning", seen: 2000, matched: 1500 };
const ready: LbImportStatus = {
  ...idle,
  phase: "ready",
  seen: 5000,
  matched: 4100,
  unmatched: 900,
  ambiguous: 12,
  summary: { onListenbrainz: 5000, matched: 4100, alreadyHere: 3000, willAdd: 1100, unmatched: 900, ambiguous: 12 },
};

function openTools(extra: Record<string, (a: Record<string, unknown>) => unknown> = {}) {
  const backend = mockBackend({
    get_app_settings: () => ({ closeToTray: true, globalHotkeys: true, lyricsOnline: true, fixTagsOnline: true, listenbrainzUser: "me", listenbrainzLoved: true }),
    lb_loved_waiting: () => 0,
    lb_import_status: () => idle,
    ...extra,
  });
  render(ListenBrainzTools);
  return { backend, user: userEvent.setup() };
}

describe("Settings › ListenBrainz: restoring the history", () => {
  test("it reads first, shows what it would add, and adds only after the person says yes", async () => {
    // The backend remembers where it is: after adding, its status says "done".
    const applied = { added: 1100, live: 700, archived: 400, already: 3000, safety: "C:\\data\\backups\\safety-1.zip" };
    let now: LbImportStatus = idle;
    const { backend, user } = openTools({
      lb_import_status: () => now,
      lb_import_start: () => scanning,
      lb_import_apply: () => {
        now = { ...ready, phase: "done", applied };
        return applied;
      },
    });
    await settle();
    await user.click(screen.getByRole("button", { name: /Restore history from ListenBrainz/ }));
    const dialog = screen.getByRole("dialog", { name: "Restore history from ListenBrainz" });
    expect(within(dialog).getByText(/Nothing changes until you say so/)).toBeInTheDocument();

    await user.click(within(dialog).getByRole("button", { name: "Start reading" }));
    await settle();
    expect(backend.to("lb_import_start")).toHaveLength(1);
    expect(dialog).toHaveTextContent(`${num(2000)} so far, ${num(1500)} matched`);
    // Pausing is offered while it reads.
    expect(within(dialog).getByRole("button", { name: "Pause" })).toBeInTheDocument();

    // The job finishes: the dry-run summary.
    await emit("listenbrainz-import-progress", ready);
    await settle();
    expect(dialog).toHaveTextContent(`${num(5000)} listens on ListenBrainz, ${num(4100)} match songs in your library, ${num(3000)} already in your history, will add ${num(1100)}.`);
    expect(dialog).toHaveTextContent("900 listens didn’t match a song for sure");
    expect(dialog).toHaveTextContent("12 fit more than one song");
    expect(dialog).toHaveTextContent("A copy of your data is saved first");
    // Nothing was written by looking.
    expect(backend.to("lb_import_apply")).toHaveLength(0);

    await user.click(within(dialog).getByRole("button", { name: `Add ${num(1100)} plays` }));
    await settle();
    expect(backend.to("lb_import_apply")).toHaveLength(1);
    expect(dialog).toHaveTextContent(`Added ${num(1100)} plays: 700 to your recent history and 400 to its archive.`);
    expect(dialog).toHaveTextContent("safety-1.zip");
  });

  test("Discard forgets what was read without adding anything, and there's nothing to add when all of it is there", async () => {
    const { backend, user } = openTools({ lb_import_status: () => ({ ...ready, summary: { ...ready.summary!, willAdd: 0, alreadyHere: 4100 } }) });
    await settle();
    await user.click(screen.getByRole("button", { name: /Restore history from ListenBrainz/ }));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByRole("button", { name: "Add 0 plays" })).toBeDisabled();
    await user.click(within(dialog).getByRole("button", { name: "Discard" }));
    await settle();
    expect(backend.to("lb_import_discard")).toHaveLength(1);
    expect(backend.to("lb_import_apply")).toHaveLength(0);
  });

  test("a failed or paused reading carries on from where it was", async () => {
    const { backend, user } = openTools({ lb_import_status: () => ({ ...idle, phase: "failed", seen: 3000, message: "Couldn’t reach ListenBrainz. Check the connection and try again." }), lb_import_start: () => scanning });
    await settle();
    await user.click(screen.getByRole("button", { name: /Restore history from ListenBrainz/ }));
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent("Couldn’t reach ListenBrainz");
    expect(dialog).toHaveTextContent(`${num(3000)} listens were read before that`);
    await user.click(within(dialog).getByRole("button", { name: "Resume" }));
    await settle();
    expect(backend.to("lb_import_start")).toHaveLength(1);
  });

  test("a failing command shows its message instead of breaking the page", async () => {
    const { user } = openTools({
      lb_import_start: () => {
        throw "Connect ListenBrainz first.";
      },
    });
    await settle();
    await user.click(screen.getByRole("button", { name: /Restore history from ListenBrainz/ }));
    await user.click(screen.getByRole("button", { name: "Start reading" }));
    await settle();
    expect(screen.getByRole("dialog")).toHaveTextContent("Connect ListenBrainz first.");
  });
});

describe("Settings › ListenBrainz: loved songs", () => {
  const preview: LbSyncPreview = { favorites: 40, loved: 55, inBoth: 30, toSend: 10, toHeart: 25, notFound: 6, ambiguous: 2 };

  test("Sync favorites shows the numbers and changes nothing until Apply", async () => {
    const { backend, user } = openTools({ lb_loved_preview: () => preview, lb_loved_apply: () => ({ hearted: 25, queued: 10 }) });
    await settle();
    await user.click(screen.getByRole("button", { name: "Sync favorites…" }));
    await settle();
    const dialog = screen.getByRole("dialog", { name: "Sync favorites" });
    expect(backend.to("lb_loved_preview")).toHaveLength(1);
    expect(dialog).toHaveTextContent("40 favorites here, 55 loved songs on ListenBrainz, 30 in both.");
    expect(dialog).toHaveTextContent("Send 10 favorites to ListenBrainz as loved.");
    expect(dialog).toHaveTextContent("Heart 25 songs here that you love there.");
    expect(dialog).toHaveTextContent("6 loved songs aren’t in your library for sure (2 fit more than one song): left out.");
    expect(dialog).toHaveTextContent("Nothing is ever un-hearted here.");
    expect(backend.to("lb_loved_apply")).toHaveLength(0);

    await user.click(within(dialog).getByRole("button", { name: "Apply" }));
    await settle();
    expect(backend.to("lb_loved_apply")).toHaveLength(1);
    expect(dialog).toHaveTextContent("Hearted 25 songs here. 10 favorites will be sent to ListenBrainz in the background.");
  });

  test("Cancel applies nothing, and when both sides agree there is nothing to apply", async () => {
    const { backend, user } = openTools({ lb_loved_preview: () => ({ ...preview, toSend: 0, toHeart: 0 }) });
    await settle();
    await user.click(screen.getByRole("button", { name: "Sync favorites…" }));
    await settle();
    expect(screen.getByRole("button", { name: "Nothing to do" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(backend.to("lb_loved_apply")).toHaveLength(0);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  test("the toggle for sending hearts is on to begin with, and saved when changed", async () => {
    const { backend, user } = openTools({ lb_loved_waiting: () => 3 });
    await settle();
    const toggle = screen.getByRole("switch", { name: /Send favorites to ListenBrainz/ });
    expect(toggle).toHaveAttribute("aria-checked", "true");
    expect(screen.getByText("3 loved songs waiting to be sent.")).toBeInTheDocument();
    await user.click(toggle);
    expect(backend.last("set_listenbrainz_loved")).toEqual({ enabled: false });
  });
});

describe("Discover › New from artists you own", () => {
  // jsdom has no IntersectionObserver: a stand-in the test drives, one row at a time.
  let live: { callback: IntersectionObserverCallback; targets: Set<Element> }[];
  beforeEach(() => {
    live = [];
    vi.stubGlobal(
      "IntersectionObserver",
      class {
        entry = { callback: undefined as unknown as IntersectionObserverCallback, targets: new Set<Element>() };
        constructor(callback: IntersectionObserverCallback) {
          this.entry.callback = callback;
          live.push(this.entry);
        }
        observe(el: Element) {
          this.entry.targets.add(el);
        }
        unobserve(el: Element) {
          this.entry.targets.delete(el);
        }
        disconnect() {
          live = live.filter((o) => o !== this.entry);
        }
      },
    );
  });
  afterEach(() => vi.unstubAllGlobals());

  /** The row scrolls into view. */
  function scrollTo(row: HTMLElement) {
    for (const o of live.filter((o) => o.targets.has(row))) o.callback([{ isIntersecting: true, target: row } as unknown as IntersectionObserverEntry], {} as IntersectionObserver);
  }

  const release = (n: number, over: Partial<NewRelease> = {}): NewRelease => ({
    key: `r${n}`,
    title: `Prism ${n}`,
    artist: "Lumen",
    date: "2026-11-10",
    kind: "Album",
    upcoming: false,
    cover: `https://coverartarchive.org/release-group/g${n}/front-250`,
    ...over,
  });
  const answer = (items: NewRelease[], over: Partial<NewReleases> = {}): NewReleases => ({ items, fetched: 1_794_000_000, enabled: true, error: "", ...over });

  test("lists each release with its artist, date, kind and where to buy it", async () => {
    mockBackend({ new_releases: () => answer([release(1), release(2, { kind: "Single", upcoming: true, date: "2026-12-25", title: "Glass" })]), new_releases_cached: () => answer([]) });
    render(NewReleasesSection);
    await settle();
    const section = screen.getByRole("region", { name: "New from artists you own" });
    expect(within(section).getAllByRole("listitem")).toHaveLength(2);
    expect(within(section).getByText("Prism 1")).toBeInTheDocument();
    expect(within(section).getByText("Single")).toBeInTheDocument();
    expect(within(section).getByText("Not out yet")).toBeInTheDocument();
    expect(within(section).getAllByText("Buy on")).toHaveLength(2);
    expect(within(section).getAllByRole("button", { name: "Bandcamp" })).toHaveLength(2);
  });

  test("a cover is asked for only once its row is on screen", async () => {
    mockBackend({ new_releases: () => answer([release(1), release(2), release(3)]), new_releases_cached: () => answer([]) });
    const { container } = render(NewReleasesSection);
    await settle();
    // Three rows, no image request yet.
    expect(container.querySelectorAll("img")).toHaveLength(0);
    const rows = Array.from(container.querySelectorAll<HTMLElement>("[data-key]"));
    expect(rows).toHaveLength(3);
    scrollTo(rows[1]);
    await settle();
    const imgs = Array.from(container.querySelectorAll("img"));
    expect(imgs).toHaveLength(1);
    expect(imgs[0].getAttribute("src")).toContain("/g2/");
    // The rows still off screen have none.
    expect(rows[0].querySelector("img")).toBeNull();
    expect(rows[2].querySelector("img")).toBeNull();
  });

  test("rows past the first few aren't drawn at all until asked for", async () => {
    const many = Array.from({ length: 12 }, (_, i) => release(i + 1));
    mockBackend({ new_releases: () => answer(many), new_releases_cached: () => answer([]) });
    const { container } = render(NewReleasesSection);
    await settle();
    expect(container.querySelectorAll("[data-key]")).toHaveLength(8);
    expect(container.querySelectorAll("img")).toHaveLength(0);
    await userEvent.setup().click(screen.getByRole("button", { name: "Show all 12" }));
    await settle();
    expect(container.querySelectorAll("[data-key]")).toHaveLength(12);
    expect(container.querySelectorAll("img")).toHaveLength(0);
  });

  test("an empty answer says so, and a failed one gives its reason without breaking the page", async () => {
    mockBackend({ new_releases: () => answer([]), new_releases_cached: () => answer([]) });
    const first = render(NewReleasesSection);
    await settle();
    expect(screen.getByText("Nothing new from the artists in your library in the last month.")).toBeInTheDocument();
    first.unmount();
    mockBackend({ new_releases: () => answer([], { fetched: 0, error: "Couldn’t reach the release list. Check the connection." }), new_releases_cached: () => answer([], { fetched: 0 }) });
    render(NewReleasesSection);
    await settle();
    expect(screen.getByText(/Couldn’t reach the release list/)).toBeInTheDocument();
  });

  test("turned off with nothing kept, the section isn't there; and an unanswered command doesn't throw", async () => {
    mockBackend({ new_releases: () => answer([], { enabled: false, fetched: 0 }), new_releases_cached: () => answer([], { enabled: false, fetched: 0 }) });
    const off = render(NewReleasesSection);
    await settle();
    expect(screen.queryByRole("region", { name: "New from artists you own" })).toBeNull();
    off.unmount();
    // The backend answers nothing at all (null): still nothing drawn, no error.
    mockBackend();
    render(NewReleasesSection);
    await settle();
    expect(screen.queryByRole("region", { name: "New from artists you own" })).toBeNull();
  });
});
