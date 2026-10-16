import { useState, type CSSProperties } from "react";
import { fileSrc } from "../../api";
import { initials, nameHue } from "../../format";
import type { Channel } from "../../types";

/**
 * A channel's picture, cached by the poll from its listing, or -- until the
 * first poll after an update has fetched it, or when the file will not load --
 * its initials on a disc coloured from its name. The disc is drawn in the same
 * box the picture fills, so a picture arriving never moves the row. Its size
 * is the row's business (CSS), which is how the opened row grows it.
 */
export default function ChannelPicture({ channel }: { channel: Pick<Channel, "title" | "thumb_path"> }) {
  // Keyed on the path that failed, not a bare flag: a new picture (a later
  // poll, a channel that changed its own) deserves a fresh try.
  const [failed, setFailed] = useState<string | null>(null);
  const path = channel.thumb_path;
  if (path && failed !== path) {
    return (
      <img
        className="add-pic"
        src={fileSrc(path)}
        alt=""
        draggable={false}
        onError={() => setFailed(path)}
      />
    );
  }
  return (
    <span
      className="add-pic is-initials"
      aria-hidden="true"
      style={{ "--hue": nameHue(channel.title) } as CSSProperties}
    >
      {initials(channel.title)}
    </span>
  );
}
