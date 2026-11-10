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
    case "running":
    case "live":
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

// ---------------------------------------------------------------------------
// Session Inspector view model (ticket 14)
// ---------------------------------------------------------------------------

/** Inspect item kinds rendered with the notice icon: the backend `notice`
 * kind plus the attention / assignment notice vocabulary and the legacy
 * `mailbox` kind the inspector used before the ticket-12 contract. */
const NOTICE_KINDS = new Set([
  "notice",
  "attention",
  "attention_notice",
  "assignment_notice",
  "mailbox",
]);

/** The icon + label one transcript item kind renders with: 💬 messages, 💭
 * Thinking, 🔧 Tool Calls, and 📩 notices. Unknown kinds still get a label so
 * the inspector never renders an unlabeled block. */
export function transcriptKindMeta(kind) {
  if (kind === "thinking") {
    return { icon: "💭", label: "Thinking" };
  }
  if (kind === "tool_call") {
    return { icon: "🔧", label: "Tool Call" };
  }
  if (NOTICE_KINDS.has(kind)) {
    return { icon: "📩", label: "Notice" };
  }
  if (kind === "message") {
    return { icon: "💬", label: "Message" };
  }
  return { icon: "•", label: kind || "Item" };
}

/** Whether an inspect item kind is a 📩 notice (attention or assignment
 * notices, and the legacy `mailbox` kind). */
export function isNoticeKind(kind) {
  return NOTICE_KINDS.has(kind);
}

/** Seat statuses whose turn is still open: the agent is producing output or
 * waiting on the Human mid-turn (`hc` reports `live`/`idle`, Herdr agents
 * `working`/`blocked`/`done`). Everything else — idle, offline, finished —
 * means no turn is running. */
const SEAT_ACTIVE_STATUSES = new Set(["working", "running", "live", "busy", "blocked"]);

/** Whether the seat's turn is active. Accepts a seat object (its `status`)
 * or a bare status string; unknown or missing statuses are not active. */
export function isSeatActive(seatOrStatus) {
  const status =
    typeof seatOrStatus === "string"
      ? seatOrStatus
      : (seatOrStatus && seatOrStatus.status) || "";
  return SEAT_ACTIVE_STATUSES.has(String(status).trim().toLowerCase());
}

/** Default expansion of one inspect item: 💭 Thinking blocks render expanded
 * while the seat's turn is active (the Human watches the reasoning stream)
 * and collapsed once the seat goes idle or finishes; every other kind always
 * renders in full. */
export function defaultItemExpanded(item, seatActive) {
  if (item && item.kind === "thinking") {
    return Boolean(seatActive);
  }
  return true;
}

/** The seat's live `hc` terminal address to focus for the Raw PTY toggle and
 * the prompt box, or `null` when the seat has no terminal (both stay disabled
 * with a hint). */
export function ptyTarget(seat) {
  const address = typeof seat === "object" && seat !== null ? seat.terminal : null;
  return typeof address === "string" && address.trim() !== "" ? address : null;
}
