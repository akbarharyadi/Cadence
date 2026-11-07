import type { HandleClientError } from "@sveltejs/kit/hooks";
import { install, recordCrash } from "#lib/diag.js";

// Start recording steps for bug reports before anything else runs.
install();

// SvelteKit replaces an unexpected error's message with "Internal Error". Cadence runs locally,
// so there's nothing to hide: keep the real message so the error page (and a screenshot of it)
// says what broke, and keep the error for "Report on GitHub".
export const handleError: HandleClientError = ({ error }) => {
  console.error("[Cadence]", error);
  const crash = recordCrash(error, "page");
  // Kit hands boundaries (Guard.svelte) this object, not the error itself.
  return { message: crash.message };
};
