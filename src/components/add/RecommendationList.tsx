import { useEffect, useId, useMemo, useRef, useState } from "react";
import ChannelPicture from "./ChannelPicture";
import { sortRecommendations } from "./recs";
import { useFold } from "./useFold";
import { IconCaret, IconClose } from "../Icons";
import { useToast } from "../Toast";
import { api, fileSrc } from "../../api";
import { usePending } from "../../pending";
import { formatCompact, formatRelative, videoUrl } from "../../format";
import type { RecSort, RecUpload, Recommendation } from "../../types";

interface Props {
  folded: boolean;
  onFold: (folded: boolean) => void;
  sort: RecSort;
  onSort: (sort: RecSort) => void;
  /** Refetches whenever it changes: the shell's channel list, which moves when
   *  a poll finishes or a subscription is made. */
  refreshKey: unknown;
  /** A subscription was made: the shell reloads. */
  onChanged: () => void;
}

const SORTS: { value: RecSort; label: string }[] = [
  { value: "videos", label: "Most videos" },
  { value: "popular", label: "Most popular" },
];

function channelUrl(r: Recommendation): string {
  return r.handle
    ? `https://www.youtube.com/${encodeURIComponent(r.handle).replace("%40", "@")}`
    : `https://www.youtube.com/channel/${encodeURIComponent(r.id)}`;
}

/**
 * The Add dialog's Recommendations: channels found on new subscription videos
 * -- credited on a collab, or beside them in "up next" -- that `recs` checked
 * and found alive. Each shows its last four uploads, which are what the
 * decision is made from. Subscribe goes the Add box's own way; × is for good.
 */
export default function RecommendationList({ folded, onFold, sort, onSort, refreshKey, onChanged }: Props) {
  const toast = useToast();
  const bodyId = `${useId()}-body`;
  const { body, toggle } = useFold(folded, onFold);
  const { pending, slow, run } = usePending();
  const [recs, setRecs] = useState<Recommendation[] | null>(null);
  const fetchId = useRef(0);

  useEffect(() => {
    const id = ++fetchId.current;
    api.listRecommendations().then(
      (list) => { if (id === fetchId.current) setRecs(list); },
      // Nothing worth a toast: the section reads as empty.
      () => { if (id === fetchId.current) setRecs((r) => r ?? []); },
    );
  }, [refreshKey]);

  const shown = useMemo(() => (recs ? sortRecommendations(recs, sort) : []), [recs, sort]);

  function drop(id: string) {
    setRecs((list) => list?.filter((r) => r.id !== id) ?? null);
  }

  function subscribe(r: Recommendation) {
    void run(`subscribe:${r.id}`, async () => {
      await api.addChannel(r.id);
      drop(r.id);
      toast.success(`Subscribed to ${r.title}.`);
      onChanged();
    });
  }

  function dismiss(r: Recommendation) {
    void run(`dismiss:${r.id}`, async () => {
      await api.dismissRecommendation(r.id);
      drop(r.id);
    });
  }

  function open(key: string, url: string) {
    // Keyed so a double press opens one browser tab, not two.
    void run(`open:${key}`, () => api.openExternal(url));
  }

  return (
    <section className={`add-subs add-recs${folded ? " is-folded" : ""}`} aria-label="Recommendations">
      <div className="add-subs-head">
        <button
          type="button"
          className="add-fold"
          aria-expanded={!folded}
          aria-controls={bodyId}
          onClick={toggle}
        >
          <span className="add-fold-caret" aria-hidden="true"><IconCaret /></span>
          Recommendations{" "}
          <span className="section-count">{recs?.length ?? 0}</span>
        </button>
        <div className="add-subs-tools" inert={folded} aria-hidden={folded || undefined}>
          <div className="rec-sort" role="group" aria-label="Order">
            {SORTS.map((s) => (
              <button
                key={s.value}
                type="button"
                className={`rec-sort-btn${sort === s.value ? " is-on" : ""}`}
                aria-pressed={sort === s.value}
                onClick={() => onSort(s.value)}
              >
                {s.label}
              </button>
            ))}
          </div>
        </div>
      </div>

      <div className="add-subs-body" id={bodyId} ref={body} inert={folded}>
        {recs !== null && recs.length === 0 ? (
          <p className="add-subs-empty">Suggestions appear here as new videos arrive.</p>
        ) : (
          <ul className="add-subs-list rec-list">
            {shown.map((r) => {
              const subscribing = pending.has(`subscribe:${r.id}`);
              return (
                <li key={r.id} className="rec">
                  <div className="rec-head">
                    <ChannelPicture channel={{ title: r.title, thumb_path: r.thumbPath }} />
                    <div className="rec-who">
                      <h4 className="rec-name">{r.title}</h4>
                      <p className="rec-sub">
                        <button
                          type="button"
                          className="rec-handle"
                          title="Open the channel in the browser"
                          onClick={() => open(r.id, channelUrl(r))}
                        >
                          {r.handle ?? "Channel page"}
                        </button>
                        {r.subscribers != null && (
                          <span>
                            {" · "}{formatCompact(r.subscribers)} subscriber{r.subscribers === 1 ? "" : "s"}
                          </span>
                        )}
                      </p>
                    </div>
                    <button
                      type="button"
                      className={`btn btn-sm btn-primary rec-subscribe${slow.has(`subscribe:${r.id}`) ? " is-spinning" : ""}`}
                      aria-label={`Subscribe to ${r.title}`}
                      disabled={subscribing || pending.has(`dismiss:${r.id}`)}
                      onClick={() => subscribe(r)}
                    >
                      {subscribing ? "Subscribing…" : "Subscribe"}
                    </button>
                    <button
                      type="button"
                      className="icon-btn rec-dismiss"
                      aria-label={`Not interested in ${r.title}`}
                      title="Not interested"
                      disabled={subscribing || pending.has(`dismiss:${r.id}`)}
                      onClick={() => dismiss(r)}
                    >
                      <span aria-hidden="true"><IconClose /></span>
                    </button>
                  </div>
                  <div className="rec-strip">
                    {r.uploads.map((u) => (
                      <Upload key={u.id} upload={u} onOpen={() => open(u.id, videoUrl(u))} />
                    ))}
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </section>
  );
}

function Upload({ upload: u, onOpen }: { upload: RecUpload; onOpen: () => void }) {
  const [failed, setFailed] = useState(false);
  return (
    <button
      type="button"
      className="rec-up"
      aria-label={`Open ${u.title} on YouTube`}
      title={u.title}
      onClick={onOpen}
    >
      {u.thumbPath && !failed ? (
        <img className="rec-up-img" src={fileSrc(u.thumbPath)} alt="" draggable={false}
             onError={() => setFailed(true)} />
      ) : (
        <span className="rec-up-img is-empty" aria-hidden="true" />
      )}
      <span className="rec-up-title" aria-hidden="true">{u.title}</span>
      <span className="rec-up-age" aria-hidden="true">{formatRelative(u.publishedAt)}</span>
    </button>
  );
}
