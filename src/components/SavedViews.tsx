import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { IconCheck, IconDelete, IconEdit, IconMore, IconPlus, IconStar, IconView } from "./Icons";
import { describeFilters, newViewId, viewIsShowing } from "../filters";
import type { SavedView, ViewFilters } from "../types";

interface Props {
  views: SavedView[];
  /** The filters on screen, and the search box. */
  current: ViewFilters;
  search: string;
  /** The channel pick of a set of filters, in words ("All but Nexpo"). */
  channelsLabel: (f: ViewFilters) => string;
  onApply: (view: SavedView) => void;
  /** The whole list, after a save, an update, a rename, a star or a delete. */
  onViews: (views: SavedView[]) => void;
  /** A series is open: the filters do not apply, so neither does a view. */
  off: boolean;
}

/** Esc inside these is theirs: it closes the menu or the form, not the sidebar
 *  (`useDismiss` leaves it alone). */
const OWN_ESCAPE = { "data-own-escape": "" };

/**
 * The saved views, at the top of All filters: a named set of filters picked in
 * one press. A view is showing -- lit, its bookmark filled -- while the filters
 * on screen are exactly its own; change one and it goes dark, and its menu
 * offers to take the new ones. A star puts it in the top bar too, ahead of the
 * starred filters.
 */
export default function SavedViews(p: Props) {
  const [menu, setMenu] = useState<string | null>(null);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [name, setName] = useState("");
  const [withSearch, setWithSearch] = useState(false);
  const search = p.search.trim();
  const showing = p.views.some((v) => viewIsShowing(v, p.current, p.search));

  // A press outside the open menu closes it.
  const menuRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (!menu) return;
    const onDown = (e: MouseEvent) => {
      if (!menuRef.current?.contains(e.target as Node)) setMenu(null);
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [menu]);

  const change = (id: string, edit: (v: SavedView) => SavedView) =>
    p.onViews(p.views.map((v) => (v.id === id ? edit(v) : v)));

  const save = () => {
    const trimmed = name.trim();
    if (!trimmed) return;
    p.onViews([...p.views, {
      id: newViewId(), name: trimmed.slice(0, 60), starred: false, filters: p.current,
      search: withSearch && search ? search : null,
    }]);
    setSaving(false);
    setName("");
    setWithSearch(false);
  };

  const escape = (e: KeyboardEvent, close: () => void) => {
    if (e.key !== "Escape") return;
    // Closing this, not the sidebar or the series behind it.
    e.stopPropagation();
    close();
  };

  return (
    <div className="views">
      {p.views.length === 0 && !saving && (
        <p className="views-empty">Save the filters you keep coming back to, and pick them again in one press.</p>
      )}
      {p.views.map((v) => {
        const on = viewIsShowing(v, p.current, p.search);
        const words = describeFilters(v.filters, p.channelsLabel(v.filters), v.search).join(" · ");
        return (
          <div key={v.id} className={`view-row${on ? " is-on" : ""}`}>
            {renaming === v.id ? (
              <div className="view-rename" {...OWN_ESCAPE}>
                <input
                  className="text-input"
                  aria-label={`Rename ${v.name}`}
                  defaultValue={v.name}
                  maxLength={60}
                  autoFocus
                  onFocus={(e) => e.currentTarget.select()}
                  onKeyDown={(e) => {
                    escape(e, () => setRenaming(null));
                    if (e.key === "Enter") e.currentTarget.blur();
                  }}
                  onBlur={(e) => {
                    const next = e.currentTarget.value.trim();
                    if (next) change(v.id, (x) => ({ ...x, name: next }));
                    setRenaming(null);
                  }}
                />
              </div>
            ) : (
              <button
                type="button"
                className="view-main"
                aria-label={v.name}
                aria-pressed={on}
                aria-describedby={`view-${v.id}`}
                disabled={p.off}
                title={p.off ? "Not applied while a series is open" : undefined}
                onClick={() => p.onApply(v)}
              >
                <span className="view-glyph" aria-hidden="true"><IconView on={on} /></span>
                <span className="side-text">
                  <span className="view-name">
                    {v.name}
                    {on && <span className="view-showing">Showing</span>}
                  </span>
                  <span className="side-desc" id={`view-${v.id}`}>{words}</span>
                </span>
              </button>
            )}
            <div className="view-more-wrap" ref={menu === v.id ? menuRef : undefined} {...OWN_ESCAPE}
              onKeyDown={(e) => escape(e, () => setMenu(null))}>
              <button
                type="button"
                className="view-more"
                aria-label={`More for ${v.name}`}
                aria-haspopup="menu"
                aria-expanded={menu === v.id}
                title="Update, rename or delete"
                onClick={() => setMenu(menu === v.id ? null : v.id)}
              >
                <IconMore />
              </button>
              {menu === v.id && (
                <div className="view-menu" role="menu" aria-label={v.name}>
                  <button
                    type="button"
                    role="menuitem"
                    disabled={on}
                    onClick={() => {
                      change(v.id, (x) => ({ ...x, filters: p.current }));
                      setMenu(null);
                    }}
                  >
                    <IconCheck />{on ? "Already these filters" : "Update to these filters"}
                  </button>
                  <button
                    type="button"
                    role="menuitem"
                    onClick={() => {
                      setMenu(null);
                      setRenaming(v.id);
                    }}
                  >
                    <IconEdit />Rename
                  </button>
                  <div className="pop-sep" />
                  <button
                    type="button"
                    role="menuitem"
                    className="is-danger"
                    onClick={() => {
                      setMenu(null);
                      p.onViews(p.views.filter((x) => x.id !== v.id));
                    }}
                  >
                    <IconDelete />Delete view
                  </button>
                </div>
              )}
            </div>
            <button
              type="button"
              className={`side-star${v.starred ? " is-on" : ""}`}
              aria-pressed={v.starred}
              aria-label={`Star ${v.name}`}
              title={v.starred ? "In the top bar. Click to take it out" : "Star to keep it in the top bar"}
              onClick={() => change(v.id, (x) => ({ ...x, starred: !x.starred }))}
            >
              <IconStar on={v.starred} />
            </button>
          </div>
        );
      })}

      {saving ? (
        <form
          className="view-form"
          {...OWN_ESCAPE}
          onKeyDown={(e) => escape(e, () => setSaving(false))}
          onSubmit={(e) => {
            e.preventDefault();
            save();
          }}
        >
          <input
            className="text-input"
            aria-label="Name this view"
            placeholder="Name this view, e.g. Long shows"
            value={name}
            maxLength={60}
            autoFocus
            onChange={(e) => setName(e.currentTarget.value)}
          />
          <div className="view-form-head">It will set</div>
          <div className="view-tags">
            {describeFilters(p.current, p.channelsLabel(p.current), withSearch && search ? search : null)
              .map((t) => <span key={t} className="view-tag">{t}</span>)}
          </div>
          {search && (
            <label className="view-search">
              <input type="checkbox" checked={withSearch} onChange={(e) => setWithSearch(e.currentTarget.checked)} />
              Include the search “{search}”
            </label>
          )}
          <div className="view-form-btns">
            <button type="button" className="btn" onClick={() => setSaving(false)}>Cancel</button>
            <button type="submit" className="btn btn-view" disabled={!name.trim()}>Save view</button>
          </div>
        </form>
      ) : (
        <button
          type="button"
          className="view-save"
          disabled={showing || p.off}
          title={showing ? "These filters are already a view" : undefined}
          onClick={() => setSaving(true)}
        >
          <IconPlus />Save these filters as a view
        </button>
      )}
    </div>
  );
}
