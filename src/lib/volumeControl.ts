/** The volume slider's logic (PlayerBar), apart from the component so it can be tested directly.
 *
 * Every change is sent with the output-device generation (`Status.deviceGen`) of the volume it was
 * made from, so the engine drops one meant for a device playback has since left. A drag keeps the
 * generation it started under; when a status of another generation arrives during it (playback
 * moved to another device, or Direct output was turned on or off), the drag is over: the slider
 * shows that device's volume, and nothing more is sent until the pointer is let go. Unmute never
 * brings back a level from another device. */

/** What Unmute returns to when the device's own level isn't known (the slider at 30%: the level
 * Direct output starts a new device at, `NEW_DIRECT_VOLUME`). */
export const SAFE_SLIDER = 0.3;

export class VolumeControl {
  /** The generation the shown volume came from. */
  private gen: number | undefined;
  /** Whether the latest status has been seen at all. */
  private seen = false;
  private dragging = false;
  /** The drag was ended by another generation: its moves aren't sent. */
  private stale = false;
  /** The generation at pointerdown. */
  private dragGen: number | undefined;
  /** The level Unmute returns to (slider position), on this device. */
  lastNonZero = 0.55;

  /** A status arrived (`volume`: the engine's, the slider's squared). Returns the slider position
   * to show, or null to leave the slider where the listener is dragging it. */
  status(volume: number, gen: number | undefined): number | null {
    const v = Math.sqrt(Math.max(0, Number.isFinite(volume) ? volume : 0));
    if (!this.seen || gen !== this.gen) {
      // Another device (or mode): nothing of the one before carries over.
      this.seen = true;
      this.gen = gen;
      if (this.dragging) this.stale = true;
      this.dragging = false;
      this.lastNonZero = v > 0 ? v : Math.min(this.lastNonZero, SAFE_SLIDER);
      return v;
    }
    if (this.dragging) return null;
    if (v > 0) this.lastNonZero = v;
    return v;
  }

  /** The pointer went down on the slider. */
  down(): void {
    this.dragging = true;
    this.stale = false;
    this.dragGen = this.gen;
  }

  /** The pointer was let go (or the slider lost focus). */
  up(): void {
    this.dragging = false;
    this.stale = false;
  }

  /** The listener moved the slider to `v` (dragged, or with the keys). Returns the generation to
   * send it with, or null when it isn't sent (a drag that playback moved away from under). */
  set(v: number): { gen: number | undefined } | null {
    if (this.stale) return null;
    if (v > 0) this.lastNonZero = v;
    return { gen: this.dragging ? this.dragGen : this.gen };
  }

  /** Mute (0) or Unmute (`lastNonZero`) was pressed: for the device shown. */
  press(v: number): number | undefined {
    if (v > 0) this.lastNonZero = v;
    return this.gen;
  }
}
