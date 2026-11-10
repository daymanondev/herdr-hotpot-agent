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
// Space & exec plumbing shared by both mods
// ---------------------------------------------------------------------------

/** The Space a mod acts on: the active one, else the first, else none.
 * Pure over the `spaces()` listing so it stays testable outside herdr. */
export function activeSpace(allSpaces) {
  const all = Array.isArray(allSpaces) ? allSpaces : [];
  return all.find((s) => s.active) ?? all[0] ?? null;
}

/** The workspace folder a mod runs `hotpot` against: the Space's folder, or
 * the caller's fallback (`home()`) when the Space carries none. */
export function spaceFolder(space, fallback) {
  return space?.folder || fallback;
}

/** The standard exec options for talking to the `hotpot` engine from a Space:
 * the workspace as cwd, the Space's machine as the target host, and a bounded
 * timeout. One source of truth for the shape both mods exec with. */
export function hotpotExecOptions(space, folder, timeoutMs = 5000) {
  return { cwd: folder, on: space?.machine ?? null, timeout: timeoutMs };
}

/** The directory containing `path` (both `/` and `\` separators), or `null`
 * when there is no parent to watch — the Session Inspector's `watchFolder`
 * target for the transcript a `hotpot inspect` served. */
export function parentDir(path) {
  if (typeof path !== "string" || path === "") return null;
  const cut = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  if (cut <= 0) return null;
  return path.slice(0, cut);
}

// ---------------------------------------------------------------------------
// Attention-First view model (ticket 13)
// ---------------------------------------------------------------------------

// Rank of each Attention Letter kind in the Needs-Attention section:
// NEEDS-HUMAN: / INCIDENT: letters lead, then stalled permissions, then unread
// handbacks; repeated failures and the Lead rotation mark close the list.
// Unknown kinds sort last without dropping them.
export const ATTENTION_KIND_RANK = {
  "needs-human": 0,
  incident: 1,
  "stalled-permission": 2,
  "unread-handback": 3,
  "repeated-failure": 4,
  "lead-context-rotation": 5,
};

// The Needs-Attention section of a `hotpot status` snapshot: its quiet flag
// (`true` when nothing renders — the panel collapses to "All quiet") and the
// letters ranked for display. Every item keeps its stable `att_<id>` key;
// only exception items are flagged for warning colors, so a downranked letter
// (attention_feedback) renders neutral per *Colour Only the Exception*.
export function attentionView(snapshot) {
  const letters = Array.isArray(snapshot?.attention) ? snapshot.attention : [];
  const items = letters
    .map((letter, index) => ({
      ...letter,
      rank: ATTENTION_KIND_RANK[letter.kind] ?? Object.keys(ATTENTION_KIND_RANK).length,
      downranked: Boolean(letter.downranked),
      exception: !letter.downranked,
      order: index,
    }))
    .sort((a, b) => a.rank - b.rank || a.downranked - b.downranked || a.order - b.order);
  return { quiet: items.length === 0, items };
}

// ---------------------------------------------------------------------------
// Segmented context-bar view model (ticket 13)
// ---------------------------------------------------------------------------

// The Lead context rotation mark: rotation ticks apply from 150k tokens
// (`src/runtime/attention.rs: ROTATION_MARK_TOKENS` — the 30% report mark).
export const ROTATION_MARK_TOKENS = 150000;

function segmentValue(context, field) {
  const value = context?.[field];
  return typeof value === "number" && Number.isFinite(value) && value > 0 ? value : 0;
}

// The segmented context bar of one seat: its `Cache Read` / `Fresh Input` /
// `Output` segments (value plus the fraction of the bar they fill), the total
// derived as the segments' sum, and the rotation-tick marker set only at or
// above the rotation mark. A seat with no usage events yields an empty,
// unticked bar.
export function contextBarModel(context, rotationMark = ROTATION_MARK_TOKENS) {
  const segments = [
    { kind: "cacheRead", label: "Cache Read", value: segmentValue(context, "cacheRead") },
    { kind: "freshInput", label: "Fresh Input", value: segmentValue(context, "freshInput") },
    { kind: "output", label: "Output", value: segmentValue(context, "output") },
  ].map((segment) => ({ ...segment, fraction: 0 }));
  const total = segments.reduce((sum, segment) => sum + segment.value, 0);
  for (const segment of segments) {
    segment.fraction = total > 0 ? segment.value / total : 0;
  }
  return {
    segments,
    total,
    rotationTick: total >= rotationMark,
  };
}

// ---------------------------------------------------------------------------
// Inline pill pickers & named preset profiles (ticket 13)
// ---------------------------------------------------------------------------

// The verified agent families the pills offer (`src/runtime/mod.rs:
// AGENT_FAMILIES`; ZCode rides through `pi`, never as its own family).
export const AGENT_OPTIONS = ["codex", "claude", "pi"];

// Model options each agent family offers in its pill. Fixture constants of the
// panel: concrete catalogue names, one list per family.
export const MODEL_OPTIONS = {
  codex: ["gpt-5.1-codex", "gpt-5.1", "gpt-5.1-mini"],
  claude: ["claude-opus-4-6", "claude-sonnet-4-5", "claude-haiku-4-5"],
  pi: ["zai-glm-chatter", "glm-4.7-air", "glm-4.7-flash"],
};

// The permitted per-brief thinking efforts (spec user story 19):
// `ultra`/`ultracode` never appear (`src/runtime/mod.rs: THINKING_EFFORTS`).
export const THINKING_OPTIONS = ["low", "medium", "high"];

// The roles whose defaults live in `<HOTPOT_HOME>/profiles.json`.
export const PROFILE_ROLES = ["supervisor", "lead", "peer"];

// Named room preset profiles: one coherent agent/model/thinking map per role
// seat (spec "Implementation Decisions → UI/UX Ergonomics in GPUI Mods").
export const PRESET_PROFILES = {
  "Deep Architecture": {
    supervisor: { agent: "claude", model: "claude-opus-4-6", thinking: "high" },
    lead: { agent: "codex", model: "gpt-5.1-codex", thinking: "high" },
    peer: { agent: "codex", model: "gpt-5.1-codex", thinking: "medium" },
  },
  "Fast Iteration": {
    supervisor: { agent: "claude", model: "claude-sonnet-4-5", thinking: "low" },
    lead: { agent: "codex", model: "gpt-5.1", thinking: "medium" },
    peer: { agent: "claude", model: "claude-sonnet-4-5", thinking: "low" },
  },
  Budget: {
    supervisor: { agent: "pi", model: "glm-4.7-air", thinking: "low" },
    lead: { agent: "pi", model: "glm-4.7-air", thinking: "low" },
    peer: { agent: "pi", model: "glm-4.7-flash", thinking: "low" },
  },
};

export const PRESET_NAMES = Object.keys(PRESET_PROFILES);

function ensurePillValue(value, allowed, label) {
  if (!allowed.includes(value)) {
    throw new Error(`invalid ${label} \`${value}\`: permitted values are ${allowed.join("|")}`);
  }
  return value;
}

// The argv the panel hands to `exec("hotpot", ...)`: the minimal
// `hotpot profile set <agent> <role> <model> --thinking <effort>` write path
// that atomically updates the role profile defaults in Hotpot Home. Refused
// efforts (`ultra`/`ultracode`) and unknown roles or families throw before any
// process is spawned.
export function profileSetArgs(agent, role, model, thinking) {
  ensurePillValue(agent, AGENT_OPTIONS, "agent");
  ensurePillValue(role, PROFILE_ROLES, "role");
  ensurePillValue(thinking, THINKING_OPTIONS, "thinking");
  if (typeof model !== "string" || model.trim() === "") {
    throw new Error(`invalid model \`${model}\`: a non-empty model name is required`);
  }
  return ["profile", "set", agent, role, model, "--thinking", thinking];
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

// ---------------------------------------------------------------------------
// Seat Slot view model (ticket nui/01)
// ---------------------------------------------------------------------------

// The 1:1 seat roles whose slots the panel always renders (GLOSSARY.md:
// Seat Slot) — launched with live seat data, or unlaunched with the role's
// resolved profile defaults from `hotpot status`'s `profiles`.
const SLOT_ROLES = ["supervisor", "lead"];

const SLOT_TITLES = { supervisor: "Supervisor", lead: "Lead" };

function pick(...values) {
  for (const value of values) {
    if (value !== undefined && value !== null && value !== "") return value;
  }
  return null;
}

/** One always-present Seat Slot: `launched` with the live seat data when a
 * seat of this role is running, else `status: "unlaunched"` carrying the
 * role's `snapshot.profiles` defaults. Live seat values win per field; the
 * profile defaults (then null) fill every field the seat leaves unset. */
function seatSlot(role, snapshot) {
  const seat = Array.isArray(snapshot?.seats)
    ? snapshot.seats.find((s) => s && s.role === role) ?? null
    : null;
  const profile = snapshot?.profiles?.[role] ?? {};
  const fields = {
    agent: pick(seat?.agent, profile.agent),
    model: pick(seat?.model, profile.model),
    thinking: pick(seat?.thinking, profile.thinking),
    terminal: pick(seat?.terminal),
  };
  return {
    role,
    title: SLOT_TITLES[role],
    launched: Boolean(seat),
    status: seat ? pick(seat.status, "offline") : "unlaunched",
    ...fields,
    seat,
  };
}

/** The child Peer rows attached under Lead: every peer seat joined with its
 * assignment record (matched by assignment id) so each row carries the
 * assignment, disposition, gate verdict, attempt, and terminal alongside the
 * seat's own agent/model/thinking/status. */
function peerRows(snapshot) {
  const seats = Array.isArray(snapshot?.seats) ? snapshot.seats : [];
  const tasks = Array.isArray(snapshot?.tasks) ? snapshot.tasks : [];
  return seats
    .filter((s) => s && s.role === "peer")
    .map((seat) => {
      const assignment = seat.assignment ?? null;
      const task = assignment
        ? (tasks.find((t) => t && t.id === assignment) ?? null)
        : null;
      return {
        id: seat.id ?? null,
        role: "peer",
        assignment,
        disposition: task?.disposition ?? null,
        gate: task?.verdict ?? null,
        attempt: task?.attempt ?? null,
        terminal: pick(seat.terminal, task?.terminal),
        agent: seat.agent ?? null,
        model: seat.model ?? null,
        thinking: seat.thinking ?? null,
        status: seat.status ?? null,
        seat,
      };
    });
}

/** The panel's Seat Slot model over one `hotpot status` snapshot: the
 * always-present Supervisor and Lead slots (launched with live seat data or
 * unlaunched with the role's profile defaults) plus the child Peer rows
 * attached under Lead. Pure and total: a null or partial snapshot still yields
 * the full model shape with unlaunched slots and no peers. */
export function seatSlotsModel(snapshot) {
  const model = {};
  for (const role of SLOT_ROLES) {
    model[role] = seatSlot(role, snapshot);
  }
  model.lead.peers = peerRows(snapshot);
  return model;
}
