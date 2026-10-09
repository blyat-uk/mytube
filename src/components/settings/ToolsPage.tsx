import { api } from "../../api";
import { formatRelative } from "../../format";
import { usePending } from "../../pending";
import { useToast } from "../Toast";
import { IconInfo, IconSectionTools } from "../Icons";
import { useSettingsCtx, type RailLine, type SettingsCtx } from "./context";
import { toolInFlight } from "./live";
import { Block, OptRow, Segmented, SettingsPage, Switch } from "./parts";
import { TOOL_LABEL, toolProgressPercent, toolProgressText } from "./tools";
import type { ToolKind, ToolProgress, ToolSource, ToolStatus } from "../../types";

/** "yt-dlp, ffmpeg, deno · ready", "Updating yt-dlp…", "ffmpeg failed to install". */
export function summary({ tools: live }: SettingsCtx): RailLine {
  const list = live.tools;
  if (!list) return { text: live.loadError ? "Status unavailable" : "Checking…" };
  const broken = list.filter((t) => t.state === "error");
  if (broken.length) {
    return {
      text: broken.length === 1
        ? `${TOOL_LABEL[broken[0].kind]} failed to install`
        : `${broken.length} tools failed to install`,
      tone: "error",
      flag: { text: "!", tone: "error", label: "Needs attention" },
    };
  }
  const busy = list.find((t) => toolInFlight(t.state));
  if (busy) return { text: `${busy.state === "updating" ? "Updating" : "Installing"} ${TOOL_LABEL[busy.kind]}…` };
  return { text: `${list.map((t) => TOOL_LABEL[t.kind]).join(", ")} · ready` };
}

const SOURCE_WORDS: Record<ToolSource, string> = {
  managed: "Managed by MyTube",
  system: "System",
  override: "Custom path (settings.json)",
  missing: "Not installed",
};

/** Two letters for the tile: the names are short and all lower case, so a
 *  glyph would only be a guess at what each one looks like. */
const MONOGRAM: Record<ToolKind, string> = { ytdlp: "yt", ffmpeg: "ff", deno: "dn" };

/** What each one is for, in the user's terms: nobody needs to know what deno
 *  is to see why a download would stop without it. */
const ROLE: Record<ToolKind, string> = {
  ytdlp: "Finds each video's streams and downloads them",
  ffmpeg: "Joins video and audio into one file, and converts audio",
  deno: "Answers the JavaScript check YouTube sets before it serves a video",
};

type Health = "ready" | "busy" | "error" | "missing";

/** The dot on the tile. Grey is a tool that is absent and not yet attempted:
 *  nothing has gone wrong, the install loop just has not reached it. */
function healthOf(t: ToolStatus): Health {
  if (t.state === "error") return "error";
  if (toolInFlight(t.state)) return "busy";
  return t.source === "missing" ? "missing" : "ready";
}

/** The dot's name, for a screen reader and the tooltip. */
function healthWords(t: ToolStatus, health: Health): string {
  switch (health) {
    case "ready": return "Ready";
    case "busy": return t.state === "updating" ? "Updating" : "Installing";
    case "error": return "Not working";
    case "missing": return "Not installed yet";
  }
}

/* One key for Check for updates and Retry: both are `tools_update_now`, and a
   second one sent while the first runs would only be answered "busy". */
const UPDATE = "tools-update";

/**
 * Where yt-dlp, ffmpeg and deno come from, and the one place a failed install
 * is visible. Provisioning itself is silent — it runs at startup with no dialog
 * — so this page is read, not driven; its two buttons exist for the cases the
 * background loop would otherwise only get round to on the next tick.
 *
 * The status is the shell's (first read, `tools://status`, `tools://progress`),
 * so the rail's line and these cards can never disagree.
 */
export default function ToolsPage() {
  const { settings, save, tools: live } = useSettingsCtx();
  const toast = useToast();
  const { pending, slow, run } = usePending();
  const { tools, progress, loadError } = live;

  const anyError = tools?.some((t) => t.state === "error") ?? false;
  const anyBusy = tools?.some((t) => toolInFlight(t.state)) ?? false;
  const ytdlp = tools?.find((t) => t.kind === "ytdlp");
  const updating = pending.has(UPDATE);

  const updateNow = (announce: boolean) => run(UPDATE, async () => {
    const before = ytdlp?.version ?? null;
    // A yt-dlp that was not already MyTube's own is not updated by this call
    // but installed, and the shell says "yt-dlp is ready." for that — a second
    // "updated to" toast on top would announce one event twice.
    const wasManaged = ytdlp?.source === "managed";
    const list = await api.toolsUpdateNow();
    live.apply(list);
    // Only a managed yt-dlp is ever updated, so only then does "up to date"
    // mean something; a Retry speaks through its cards instead.
    const after = list.find((t) => t.kind === "ytdlp");
    if (announce && wasManaged && after?.source === "managed" && after.state === "ready") {
      if (after.version && after.version !== before) {
        toast.success(`yt-dlp updated to ${after.version}.`);
      } else {
        toast.info("yt-dlp is up to date.");
      }
    }
  });

  const checkButton = (
    <button
      type="button"
      className="btn"
      disabled={updating || anyBusy || tools === null}
      aria-busy={updating}
      onClick={() => void updateNow(true)}
    >
      {slow.has(UPDATE) ? "Checking…" : "Check for updates"}
    </button>
  );

  return (
    <SettingsPage
      icon={<IconSectionTools />}
      title="Tools"
      className="st-tl-page"
      intro="The three programs behind every download. Any that is missing is fetched from its publisher and checked against its checksum before it is used."
    >
      {tools === null ? (
        loadError
          // Said in place rather than toasted: the page would otherwise sit on
          // "Checking…" forever with the reason already gone from the screen.
          ? <p className="st-tl-load-error" role="alert">Could not read the tools' status: {loadError}</p>
          : <div className="st-tl-loading"><span className="spinner" aria-hidden="true" />Checking…</div>
      ) : (
        <ul className="st-tl-list">
          {tools.map((t) => (
            <ToolCard
              key={t.kind}
              tool={t}
              progress={progress[t.kind]}
              retry={anyError ? {
                disabled: updating || anyBusy,
                busy: updating,
                onRetry: () => void updateNow(false),
              } : null}
            />
          ))}
        </ul>
      )}

      <Block
        title="yt-dlp updates"
        className="st-tl-updates"
        aside={<>
          {ytdlp?.lastCheck ? <span>Last checked {formatRelative(ytdlp.lastCheck)}</span> : null}
          {checkButton}
        </>}
      >
        <OptRow
          title="Channel"
          hint="YouTube breaks things often, and the fixes reach nightly first."
          hintId="ytdlp-channel-hint"
        >
          <Segmented
            label="yt-dlp channel"
            describedBy="ytdlp-channel-hint"
            options={[
              { value: "nightly", label: "Nightly", note: "recommended" },
              { value: "stable", label: "Stable" },
            ]}
            // Anything but "stable" is what the backend treats as nightly.
            value={settings.ytdlp_channel === "stable" ? "stable" : "nightly"}
            onChange={(v) => save("ytdlp_channel", v)}
          />
        </OptRow>
        <Switch
          label="Update automatically"
          hint="Once a day, and never while a download is running."
          checked={settings.ytdlp_auto_update}
          onChange={(on) => save("ytdlp_auto_update", on)}
        />
      </Block>

      <p className="st-tl-foot">
        <IconInfo />
        <span>
          A path you set yourself in <code>settings.json</code> (<code>ytdlp_path</code> and so
          on) always wins over all of this.
        </span>
      </p>
    </SettingsPage>
  );
}

function ToolCard({ tool, progress, retry }: {
  tool: ToolStatus;
  progress?: ToolProgress;
  /** Present while any tool is broken; drawn only on a broken one's card. */
  retry: { disabled: boolean; busy: boolean; onRetry: () => void } | null;
}) {
  const health = healthOf(tool);
  const busy = health === "busy";
  const broken = health === "error";
  const label = TOOL_LABEL[tool.kind];
  const pct = toolProgressPercent(progress);
  const errorId = `st-tl-error-${tool.kind}`;
  const words = healthWords(tool, health);

  return (
    <li className={`st-tl-card is-${health}`}>
      <span className="st-tl-mono">
        <span aria-hidden="true">{MONOGRAM[tool.kind]}</span>
        <span className="st-tl-dot" role="img" aria-label={words} title={words} />
      </span>

      <div className="st-tl-body">
        <div className="st-tl-name">
          <span>{label}</span>
          {tool.version && <span className="st-tl-version">{tool.version}</span>}
        </div>

        {busy ? (
          <div className="st-tl-progress">
            <div
              className={`st-tl-bar${pct === null ? " is-indeterminate" : ""}`}
              role="progressbar"
              aria-label={`${tool.state === "updating" ? "Updating" : "Installing"} ${label}`}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={pct === null ? undefined : Math.round(pct)}
            >
              <div className="st-tl-fill" style={{ width: pct === null ? undefined : `${pct}%` }} />
            </div>
            <div className="st-tl-status">{toolProgressText(tool.state, progress)}</div>
          </div>
        ) : (
          <div className="st-tl-role">{ROLE[tool.kind]}</div>
        )}

        {/* Only a broken tool gets the alert. A working one can still carry an
            error — a managed yt-dlp whose update check failed stays usable at
            the version it has — and that is a note, not a reason to Retry. */}
        {broken && tool.error && <p className="st-tl-error" id={errorId} role="alert">{tool.error}</p>}
        {!busy && tool.path && <div className="st-tl-path" title={tool.path}>{tool.path}</div>}
        {!broken && tool.error && <p className="st-tl-note">{tool.error}</p>}
      </div>

      <div className="st-tl-end">
        <span className={`st-tl-source is-${tool.source}`}>{SOURCE_WORDS[tool.source]}</span>
        {broken && retry && (
          <button
            type="button"
            className="btn btn-primary"
            disabled={retry.disabled}
            aria-busy={retry.busy}
            aria-describedby={tool.error ? errorId : undefined}
            onClick={retry.onRetry}
          >
            Retry
          </button>
        )}
      </div>
    </li>
  );
}
