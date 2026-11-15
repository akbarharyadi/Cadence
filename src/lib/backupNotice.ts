// A restore that was cut short is dealt with at startup; the owner hears about it once, as a toast,
// so they need not open Settings. The Backup section keeps showing it until a restore goes through.
import type { BackupInfo } from "./api";

const SEEN = "cadence-interrupted-seen";

/** Says (once per interruption) that a restore was cut short; `show` is the toast. */
export function announceInterrupted(info: BackupInfo | null | undefined, show: (message: string) => void) {
  const i = info?.interrupted;
  if (!i?.message) return;
  try {
    if (localStorage.getItem(SEEN) === String(i.at)) return;
    localStorage.setItem(SEEN, String(i.at));
  } catch {
    // No storage: said again next start, which is better than never.
  }
  show(i.message);
}
