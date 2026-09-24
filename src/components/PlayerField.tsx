import { useEffect, useRef, useState, type ReactNode } from "react";
import Field, { hintId } from "./Field";
import { api } from "../api";
import type { PlayerOption } from "../types";

/** The select's value for "type your own"; no detected id can take it. */
export const CUSTOM = "__custom";

/** What the list holds when detection fails: the one entry that needs nothing
 *  installed. The backend always puts it first, so this is the same list with
 *  everything detected missing from it. */
const DEFAULT_ONLY: PlayerOption[] = [{ id: "default", label: "System default", command: "" }];

interface Props {
  value: string;
  /** Typing in the custom field: held, not saved, until blur. */
  onEdit: (command: string) => void;
  onBlur: () => void;
  /** A choice from the list, which has no "done editing" moment: saved now. */
  onPick: (command: string) => void;
}

/**
 * The player, as a list of what is installed rather than a command to recall.
 * A value no detected entry carries — an argument, a path, a player detection
 * does not know — shows as Custom with the text field holding it, so nothing
 * that worked before this list existed is lost or rewritten.
 */
export default function PlayerField({ value, onEdit, onBlur, onPick }: Props) {
  const [players, setPlayers] = useState<PlayerOption[] | null>(null);
  // Custom chosen over a detected player: without this the select would snap
  // straight back to that player, since the command it still holds matches.
  const [customOpen, setCustomOpen] = useState(false);
  const inputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    let alive = true;
    api.detectPlayers()
      .then((list) => { if (alive) setPlayers(list.length ? list : DEFAULT_ONLY); })
      .catch(() => { if (alive) setPlayers(DEFAULT_ONLY); });
    return () => { alive = false; };
  }, []);

  const match = players?.find((p) => p.command === value);
  const custom = players !== null && (customOpen || !match);
  const selected = players === null ? "" : custom ? CUSTOM : match!.id;

  // Once the text field is up it stays up until another entry is picked from
  // the list. Left to `!match` alone it would vanish the moment the typing
  // happened to spell a detected command — `mpv --fullscreen` backspaced to
  // `mpv`, or cleared to `""`, the System default — and an unmounted input
  // never fires its blur, so the edit was never saved while the select showed
  // a choice settings.json did not hold.
  useEffect(() => {
    if (custom && !customOpen) setCustomOpen(true);
  }, [custom, customOpen]);

  function choose(id: string) {
    if (id === CUSTOM) {
      setCustomOpen(true);
      requestAnimationFrame(() => inputRef.current?.focus());
      return;
    }
    const picked = players?.find((p) => p.id === id);
    if (!picked) return;
    setCustomOpen(false);
    onPick(picked.command);
  }

  let hint: ReactNode = null;
  if (custom) {
    hint = "Runs to open a downloaded file. Arguments are allowed, e.g. mpv --fullscreen.";
  } else if (match && match.command === "") {
    hint = "Opens each download in whatever app your system uses for that file type.";
  } else if (match) {
    hint = <>Opens each download with <code>{match.command}</code>.</>;
  }

  return (
    <Field label="Player" htmlFor="player-select" hint={hint}>
      <select
        id="player-select"
        className="select settings-select"
        value={selected}
        disabled={players === null}
        aria-describedby={hint ? hintId("player-select") : undefined}
        onChange={(e) => choose(e.currentTarget.value)}
      >
        {players === null ? (
          <option value="">Looking for players…</option>
        ) : (
          <>
            {players.map((p) => <option key={p.id} value={p.id}>{p.label}</option>)}
            <option value={CUSTOM}>Custom command…</option>
          </>
        )}
      </select>
      {custom && (
        <input
          ref={inputRef}
          className="text-input mono player-custom"
          aria-label="Player command"
          placeholder="mpv --fullscreen"
          value={value}
          onChange={(e) => onEdit(e.currentTarget.value)}
          onFocus={() => setCustomOpen(true)}
          onBlur={onBlur}
          aria-describedby={hintId("player-select")}
          spellCheck={false}
        />
      )}
    </Field>
  );
}
