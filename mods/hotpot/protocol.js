// Shared protocol & formatting helpers for herdr-hotpot-agent mods.
// Canonical source lives in `shared/protocol.js`; synced into `mods/*/` by `scripts/sync.sh`.

export const HOTPOT_SELECTED_KEY = "hotpot.selectedAgent";
export const HOTPOT_VIEW_MODE_KEY = "hotpot.viewMode";

export const LEAD_CONTEXT_CEILING = 128000;
export const PEER_CONTEXT_CEILING = 160000;

export function formatTokenCount(tokens) {
  if (typeof tokens !== "number" || !Number.isFinite(tokens) || tokens <= 0) {
    return "0k";
  }
  if (tokens >= 1000) {
    return `${(tokens / 1000).toFixed(tokens >= 10000 ? 0 : 1)}k`;
  }
  return `${tokens}`;
}

export function formatContextBadge(tokens, role) {
  const ceiling = role === "lead" ? LEAD_CONTEXT_CEILING : PEER_CONTEXT_CEILING;
  return `${formatTokenCount(tokens)} / ${formatTokenCount(ceiling)}`;
}

export function statusColor(themeObj, status) {
  switch (status) {
    case "working":
      return themeObj.working ?? themeObj.accent;
    case "blocked":
      return themeObj.blocked ?? themeObj.ansi1;
    case "done":
    case "green":
      return themeObj.done ?? themeObj.ansi2;
    case "red":
      return themeObj.ansi1 ?? themeObj.blocked;
    default:
      return themeObj.idle ?? themeObj.dim;
  }
}

export function parseHotpotJson(stdout, fallback) {
  if (typeof stdout !== "string" || stdout.trim() === "") {
    return fallback;
  }
  try {
    return JSON.parse(stdout);
  } catch {
    return fallback;
  }
}
