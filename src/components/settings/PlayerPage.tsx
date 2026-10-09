import { useEffect, useId, useRef, useState } from "react";
import { IconComputer, IconEdit, IconPlay, IconSectionPlayer } from "../Icons";
import { useSettingsCtx, type RailLine, type SettingsCtx } from "./context";
import { Block, Cap, Choice, ChoiceList, SettingsPage } from "./parts";

/** The detected player's name, "System default", or the custom command itself. */
export function summary({ settings: s, players }: SettingsCtx): RailLine {
  const match = players?.find((p) => p.command === s.player_command);
  if (match) return { text: match.label };
  return { text: s.player_command || "System default" };
}

/**
 * The player, as a list of what is installed rather than a command to recall.
 * A value no detected entry carries -- an argument, a path, a player detection
 * does not know -- shows as Custom with the text field holding it, so nothing
 * that worked before this list existed is lost or rewritten.
 */
export default function PlayerPage() {
  const { settings, set, save, flush, players } = useSettingsCtx();
  // Custom chosen over a detected player: without this the list would snap
  // straight back to that player, since the command it still holds matches.
  const [customOpen, setCustomOpen] = useState(false);
  const inputRef = useRef<HTMLInputElement | null>(null);
  // Set by choosing Custom; the field it asks to focus mounts in that render.
  const focusField = useRef(false);
  const name = useId();
  const capId = useId();
  const hintId = useId();

  const value = settings.player_command;
  const match = players?.find((p) => p.command === value);
  const custom = players !== null && (customOpen || !match);

  useEffect(() => {
    if (!focusField.current) return;
    focusField.current = false;
    inputRef.current?.focus();
  });

  // Once the text field has been focused it stays up until another entry is
  // picked from the list. Left to `!match` alone it would vanish the moment
  // the typing happened to spell a detected command -- `mpv --fullscreen`
  // backspaced to `mpv`, or cleared to `""`, the System default -- and an
  // unmounted input never fires its blur, so the edit was never saved while
  // the list showed a choice settings.json did not hold. Focus, not mere
  // display, is the trigger: typing always starts with it, and an effect
  // keyed on display re-opened the field whenever a pick's `setCustomOpen
  // (false)` rendered a moment before the picked value arrived.
  function chooseCustom() {
    setCustomOpen(true);
    focusField.current = true;
  }

  function choosePlayer(command: string) {
    setCustomOpen(false);
    save("player_command", command);
  }

  return (
    <SettingsPage
      icon={<IconSectionPlayer />}
      title="Player"
      intro="What opens a downloaded video when you press Play."
    >
      <Block>
        <Cap id={capId}>Play downloads with</Cap>
        {players === null ? (
          <p className="st-pl-looking" role="status">Looking for players…</p>
        ) : (
          <ChoiceList labelledBy={capId}>
            {players.map((p) => (
              <Choice
                key={p.id}
                name={name}
                checked={!custom && match?.id === p.id}
                onSelect={() => choosePlayer(p.command)}
                icon={p.command === "" ? <IconComputer /> : <IconPlay />}
                title={p.label}
                detail={p.command === ""
                  ? "Opens each file in the app your system uses for it"
                  : <code className="st-pl-cmd">{p.command}</code>}
              />
            ))}
            <Choice
              name={name}
              checked={custom}
              onSelect={chooseCustom}
              icon={<IconEdit />}
              title="Custom command…"
              detail={custom ? undefined : "A program of your own, with any arguments it needs"}
            >
              <input
                ref={inputRef}
                className="text-input mono st-pl-field"
                aria-label="Player command"
                aria-describedby={hintId}
                placeholder="mpv --fullscreen"
                value={value}
                onChange={(e) => set("player_command", e.currentTarget.value)}
                onFocus={() => setCustomOpen(true)}
                onBlur={flush}
                spellCheck={false}
                autoComplete="off"
              />
              <p className="st-pl-hint" id={hintId}>
                Runs to open a downloaded file. Arguments are allowed, e.g. mpv --fullscreen.
              </p>
            </Choice>
          </ChoiceList>
        )}
      </Block>
    </SettingsPage>
  );
}
