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

// The feedback actions the panel's inline Ack / Dismiss buttons deliver:
// `useful` (the letter earned the Human's attention) and `noisy` (it did not).
// The CLI maps them onto the runtime's own vocabulary (`src/main.rs:
// cmd_attention`: `useful` → snooze, `noisy` → mute).
export const ATTENTION_FEEDBACK_ACTIONS = ["useful", "noisy"];

/** The argv the panel's inline Ack / Dismiss buttons exec:
 * `hotpot attention feedback <att_id> <useful|noisy>`. Anything but the two
 * actions — including the runtime's own `mute`/`snooze`/`downrank` vocabulary —
 * throws before any process is spawned, so the button wiring stays honest. */
export function attentionFeedbackArgs(key, action) {
  if (!ATTENTION_FEEDBACK_ACTIONS.includes(action)) {
    throw new Error(
      `invalid action \`${action}\`: permitted values are ${ATTENTION_FEEDBACK_ACTIONS.join("|")}`,
    );
  }
  if (typeof key !== "string" || key.trim() === "") {
    throw new Error(`invalid letter key \`${key}\`: a non-empty att_<id> key is required`);
  }
  return ["attention", "feedback", key, action];
}

/** The seat a letter's row-click selects into the Session Inspector: the peer
 * holding the letter's assignment when one is live, else the Lead (the
 * letters' operator-side addressee), else null — a room with no seats selects
 * nothing. Pure over the `hotpot status` snapshot. */
export function attentionSeatTarget(letter, snapshot) {
  const seats = Array.isArray(snapshot?.seats) ? snapshot.seats : [];
  const assignment = letter?.assignment;
  if (assignment) {
    const peer = seats.find((s) => s && s.role === "peer" && s.assignment === assignment);
    if (peer) return peer;
  }
  return seats.find((s) => s && s.role === "lead") ?? null;
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

/** The header preset dropdown's menu model: the named room presets the panel
 * offers, each under the stable id its menu row renders with. The compact
 * dropdown in the panel header is the presets' only surface — no fixed-width
 * middle section truncates the names. */
export function presetMenu() {
  return PRESET_NAMES.map((name) => ({
    id: name.replace(/\s+/g, "-").toLowerCase(),
    name,
  }));
}

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
 * seat's own agent/model/thinking/status. Each row also carries its tree
 * connector — `├` for every non-last child, `└` for the last — mirroring
 * Herdr's native sub-agent tree. */
function peerRows(snapshot) {
  const seats = Array.isArray(snapshot?.seats) ? snapshot.seats : [];
  const tasks = Array.isArray(snapshot?.tasks) ? snapshot.tasks : [];
  const peers = seats.filter((s) => s && s.role === "peer");
  return peers.map((seat, index) => {
    const assignment = seat.assignment ?? null;
    const task = assignment
      ? (tasks.find((t) => t && t.id === assignment) ?? null)
      : null;
    return {
      id: seat.id ?? null,
      role: "peer",
      connector: index === peers.length - 1 ? "└" : "├",
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

/** Apply one selector pick to a draft `agent/model/thinking` selection: the
 * patch wins per field, and a family change that leaves a model the new
 * family does not offer resets the model to the family's first option. A
 * patch naming an unknown family throws before anything execs. The mod
 * persists the returned selection immediately through `profileSetArgs`. */
export function applyPickerPatch(draft, patch) {
  const next = { ...draft, ...patch };
  if (!MODEL_OPTIONS[next.agent]) {
    throw new Error(`invalid agent \`${next.agent}\`: permitted values are ${AGENT_OPTIONS.join("|")}`);
  }
  if (!MODEL_OPTIONS[next.agent].includes(next.model)) {
    next.model = MODEL_OPTIONS[next.agent][0];
  }
  return next;
}

// ---------------------------------------------------------------------------
// Native panel chrome: vendor icons and geometric state markers
// (ticket nui/06)
// ---------------------------------------------------------------------------

// The chrome fallback every seat renders when Herdr's program table has no
// entry (or no icon) for the agent: the terminal glyph, a one-color svg.
export const AGENT_ICON_FALLBACK = {
  kind: "fallback",
  icon: "mods/workspace-chrome/terminal.svg",
  tint: true,
};

/** The icon descriptor one seat or peer row renders for its agent: the
 * vendor icon Herdr's `knownAgents()` registry reports (`svg(icon)` tinted
 * when `tint`, else the full-color `image(icon)`), or the terminal fallback
 * when the registry lacks the agent or an icon for it. Pure over the
 * registry so the tint rule stays node-testable outside herdr. */
export function agentIconModel(agent, registry) {
  const entries = Array.isArray(registry) ? registry : [];
  const program = entries.find(
    (entry) =>
      entry &&
      typeof entry === "object" &&
      entry.key === agent &&
      typeof entry.icon === "string" &&
      entry.icon !== "",
  );
  if (!program) return { ...AGENT_ICON_FALLBACK };
  return { kind: "vendor", icon: program.icon, tint: Boolean(program.tint) };
}

/** The quiet geometric state marker one seat row renders for its status
 * (spec "Native Herdr GPUI Design System"): a spinning `spinner.svg` while
 * the turn is active, a `7px` ring for `idle`, a `7px` filled dot for `done`,
 * a `7px` dimmed dot for `unlaunched` slots, and a tinted `!` badge for
 * `blocked`. Unknown statuses stay a neutral ring — never an exception
 * color. The marker's `tone` carries the color intent; `markerToneColor`
 * resolves it against a theme. */
export function seatStatusMarker(status) {
  switch (status) {
    case "working":
    case "running":
    case "live":
    case "busy":
      return {
        kind: "spinner",
        icon: "mods/workspace-chrome/spinner.svg",
        spinMs: 900,
        tone: "working",
      };
    case "idle":
      return { kind: "ring", size: 7, tone: "idle" };
    case "done":
      return { kind: "dot", size: 7, tone: "done" };
    case "unlaunched":
      return { kind: "dot", size: 7, tone: "dim" };
    case "blocked":
      return { kind: "badge", glyph: "!", tone: "warning" };
    default:
      return { kind: "ring", size: 7, tone: "neutral" };
  }
}

/** The theme color of one marker tone: the working accent while a turn is
 * active, the quiet dim/idle greys at rest, and the warning tint only for
 * the exception (`blocked` / the `!` badge). Falls back through the same
 * theme keys `statusColor` uses, so sparse theme fixtures stay total. */
export function markerToneColor(themeObj, tone) {
  switch (tone) {
    case "working":
      return themeObj.working ?? themeObj.accent;
    case "done":
      return themeObj.done ?? themeObj.ansi2;
    case "warning":
      return themeObj.blocked ?? themeObj.ansi1;
    case "dim":
      return themeObj.dim;
    default:
      return themeObj.idle ?? themeObj.dim;
  }
}

// ---------------------------------------------------------------------------
// Panel restructure: Quiet When Quiet, Start, preset menu, peer tree
// (ticket nui/05)
// ---------------------------------------------------------------------------

/** The Quiet When Quiet section model over one snapshot: which panel sections
 * render, fold, or hide. `attention` collapses to the single `All clear`
 * indicator while quiet; `activity` (the live mailbox) hides entirely while
 * empty; `assignments` folds to its one-line header (`Assignments  0`) while
 * empty. `seats` always renders — its count covers the two always-present
 * slots plus their child peers. Pure and total over any snapshot shape. */
export function sectionVisibility(snapshot) {
  const attention = attentionView(snapshot);
  const tasks = Array.isArray(snapshot?.tasks) ? snapshot.tasks : [];
  const mailbox = Array.isArray(snapshot?.mailbox) ? snapshot.mailbox : [];
  const peers = seatSlotsModel(snapshot).lead.peers;
  return {
    attention: { quiet: attention.quiet, count: attention.items.length },
    seats: { count: peers.length + 2 },
    assignments: { folded: tasks.length === 0, count: tasks.length },
    activity: { hidden: mailbox.length === 0, count: mailbox.length },
  };
}

// `hotpot setup --apply` materializes the whole desired state (nine Role Homes,
// contracts, skill projections, the marker file), which can far outlast the
// panel's default 5s exec budget — the Start sequence grants it a longer one.
export const SETUP_APPLY_TIMEOUT_MS = 120000;

/** The argv of the provisioning pass: `hotpot setup --apply`. */
export function setupApplyArgs() {
  return ["setup", "--apply"];
}

/** The argv that launches one seat's `hc` terminal:
 * `hotpot seat launch <role> --agent <agent> --project <folder>`. Unknown
 * roles, unverified agent families, and empty project folders throw before any
 * process is spawned. */
export function seatLaunchArgs(role, agent, project) {
  ensurePillValue(role, PROFILE_ROLES, "role");
  ensurePillValue(agent, AGENT_OPTIONS, "agent");
  if (typeof project !== "string" || project.trim() === "") {
    throw new Error(`invalid project \`${project}\`: a non-empty project folder is required`);
  }
  return ["seat", "launch", role, "--agent", agent, "--project", project];
}

/** The exec sequence the inline `Start` action on an unlaunched slot runs:
 * provision Hotpot Home first when the snapshot says the Project is not
 * provisioned, then launch the seat with the role's configured profile agent.
 * Each step carries its argv and the exec timeout it needs (`null` for the
 * default). A slot with no configured agent throws before anything execs. */
export function startSlotSteps(slot, snapshot, project) {
  const steps = [];
  if (snapshot?.provisioned === false) {
    steps.push({ args: setupApplyArgs(), timeoutMs: SETUP_APPLY_TIMEOUT_MS });
  }
  steps.push({ args: seatLaunchArgs(slot.role, slot.agent, project), timeoutMs: null });
  return steps;
}

// ---------------------------------------------------------------------------
// Directive Composer, attention feedback, onboarding chip (ticket nui/07)
// ---------------------------------------------------------------------------

// The roles the Directive Composer addresses: the Project's two 1:1 seats
// (GLOSSARY.md: Directive Composer). Peers are dispatched by Lead, never
// typed at by the Human.
const COMPOSER_ROLES = ["lead", "supervisor"];

/** The argv the composer's Send execs: `hotpot seat launch <role> --agent
 * <agent> --project <folder> --prompt <text>` — the CLI (ticket nui/04) that
 * launches or reuses the target seat and delivers the directive in one step.
 * The role defaults to `lead`; anything beyond Lead/Supervisor, an unverified
 * agent family, an empty folder, or empty directive text throws before any
 * process is spawned. */
export function directiveDispatchArgs(role, agent, folder, text) {
  const target = role ?? "lead";
  if (!COMPOSER_ROLES.includes(target)) {
    throw new Error(`invalid role \`${role}\`: permitted values are ${COMPOSER_ROLES.join("|")}`);
  }
  ensurePillValue(agent, AGENT_OPTIONS, "agent");
  if (typeof folder !== "string" || folder.trim() === "") {
    throw new Error(`invalid project \`${folder}\`: a non-empty project folder is required`);
  }
  if (typeof text !== "string" || text.trim() === "") {
    throw new Error("invalid directive: a non-empty directive text is required");
  }
  return ["seat", "launch", target, "--agent", agent, "--project", folder, "--prompt", text];
}

/** The exec sequence one composer Send runs: provision Hotpot Home first when
 * the snapshot says the Project is not provisioned (GLOSSARY.md: the composer
 * "automatically provisioning Hotpot Home and launching the target Seat"),
 * then the launch-then-prompt dispatch. Same step shape as `startSlotSteps`. */
export function directiveDispatchSteps(snapshot, role, agent, folder, text) {
  const steps = [];
  if (snapshot?.provisioned === false) {
    steps.push({ args: setupApplyArgs(), timeoutMs: SETUP_APPLY_TIMEOUT_MS });
  }
  steps.push({ args: directiveDispatchArgs(role, agent, folder, text), timeoutMs: null });
  return steps;
}

// The directive the 1-click onboarding chip delivers: run the built-in Lead
// skill (`src/skills.rs: ONBOARDING_SKILL_NAME`) whose proposal-first body
// builds an Evidence Map and drafts `WORKSPACE_PROTOCOL.md` only after
// explicit Human confirmation. A pure constant so the chip is node-testable.
export const ONBOARDING_DIRECTIVE_TEXT =
  "Run the hotpot-project-onboarding skill for this repository: build the Evidence Map from its manifests, scripts, tests, and docs, then propose a WORKSPACE_PROTOCOL.md draft — write the file only after my explicit confirmation.";

/** The onboarding chip's visibility model: visible exactly while the snapshot
 * says the Project root lacks `WORKSPACE_PROTOCOL.md` (`hotpot status`'
 * `hasWorkspaceProtocol`). A snapshot that predates the flag stays hidden —
 * the chip never guesses. */
export function onboardingChipModel(snapshot) {
  return { visible: snapshot?.hasWorkspaceProtocol === false };
}

/** The exec sequence the onboarding chip's single click runs: the same
 * launch-then-prompt dispatch as the composer, targeting Lead with the
 * onboarding directive (provisioning first when the Project needs it). */
export function onboardingDispatchSteps(snapshot, agent, folder) {
  return directiveDispatchSteps(snapshot, "lead", agent, folder, ONBOARDING_DIRECTIVE_TEXT);
}
