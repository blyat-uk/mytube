// Types `exports.default` from "cloudflare:workers" as this Worker's own default export.
declare namespace Cloudflare {
  interface GlobalProps {
    mainModule: typeof import("../src/index");
    durableNamespaces: "SyncGroup";
  }
}
