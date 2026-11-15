// Song lists with a selection (TrackList): one place that knows them, so Android's Back can leave
// selection mode, and starting a selection in one list clears it in the others.

interface Selecting {
  id: symbol;
  /** Leaves selection mode / clears a multi-selection. False when there was nothing to leave. */
  leave: () => boolean;
  /** Clears whatever is selected (another list started a selection). */
  clear: () => void;
}

const lists: Selecting[] = [];

export function registerSelection(list: Selecting): () => void {
  lists.push(list);
  return () => {
    const i = lists.indexOf(list);
    if (i >= 0) lists.splice(i, 1);
  };
}

/** Android Back: a list in selection mode (or with several songs selected) leaves it. */
export function leaveSelection(): boolean {
  for (let i = lists.length - 1; i >= 0; i--) {
    try {
      if (lists[i].leave()) return true;
    } catch {
      // a list that is going away: nothing to leave
    }
  }
  return false;
}

/** One list started selecting: the others let go of theirs. */
export function selectingIn(id: symbol) {
  for (const l of lists) if (l.id !== id) l.clear();
}
