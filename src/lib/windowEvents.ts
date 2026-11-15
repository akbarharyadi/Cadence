import { listen, type EventCallback, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

/**
 * Listens to an event meant for this window only. The plain `listen` hears events sent to any
 * window (`emit_to("main", …)` still reaches a global listener in the mini player), so the
 * per-window ones (`player-status` while a window is hidden, `window-visible`) would cross over:
 * the mini player stopped its animation for the main window's minimizing. A listener bound to
 * this webview window gets what is sent to it and what is sent to everyone, nothing else.
 */
export function listenHere<T>(event: string, handler: EventCallback<T>): Promise<UnlistenFn> {
  let here: ReturnType<typeof getCurrentWebviewWindow>;
  try {
    here = getCurrentWebviewWindow();
  } catch {
    // No window info (a plain browser, a test without one): everything is for this page.
    return listen<T>(event, handler);
  }
  return here.listen<T>(event, handler);
}
