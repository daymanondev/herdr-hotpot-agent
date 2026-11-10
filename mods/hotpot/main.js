// Attention-First right-hand Hotpot panel (ticket 13).
//
// Layout per the spec's UI/UX ergonomics: *Needs Attention* pinned on top
// (collapsing to a neutral "All quiet" line — colour only the exception), seat
// cards with segmented `Cache Read` / `Fresh Input` / `Output` context bars and
// a rotation-tick marker, inline `agent/model · thinking` pill pickers plus
// named preset profiles persisting through `hotpot profile set`, and the
// assignment master–detail list selecting into the Session Inspector via the
// shared-key flow. Everything renders over the live `hotpot status` snapshot
// (ticket 12) — empty state stays empty, never mocked.

import { View, div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { exec, spaces, theme, run, shared, setShared, home } from "herdr";
import {
  HOTPOT_SELECTED_KEY,
  HOTPOT_VIEW_MODE_KEY,
  PROFILE_ROLES,
  AGENT_OPTIONS,
  MODEL_OPTIONS,
  THINKING_OPTIONS,
  PRESET_NAMES,
  PRESET_PROFILES,
  attentionView,
  contextBarModel,
  formatTokenCount,
  profileSetArgs,
  parseHotpotJson,
  statusColor,
} from "./protocol.js";

// Character cells across one context bar; each segment draws a proportional
// run of block glyphs, so the bar stays segmented without fixed widths.
const BAR_CELLS = 24;

// Neutral informational tones for the three context segments; the rotation
// tick is the only context mark that earns a warning colour.
function segmentColor(t, kind) {
  switch (kind) {
    case "cacheRead":
      return t.muted;
    case "freshInput":
      return t.accent;
    default:
      return t.dim;
  }
}

function warningColor(t) {
  return t.blocked ?? t.ansi1;
}

export default class HotpotPanel extends View {
  init(_props, cx) {
    this.snapshot = null;
    this.lastFolder = null;
    this.loading = false;
    // The seat whose inline pill picker is open, plus its draft selection.
    this.pickerFor = null;
    this.picker = null;
    this.presetBusy = false;
    cx.timer.every(2500, (timerCx) => this.refresh(timerCx));
    this.refresh(cx);
  }

  activeSpace() {
    const all = spaces();
    return all.find((s) => s.active) ?? all[0] ?? null;
  }

  refresh(cx) {
    const sp = this.activeSpace();
    const folder = sp?.folder || home();
    if (this.loading && this.lastFolder === folder) return;
    this.loading = true;
    this.lastFolder = folder;

    cx.spawn(async (taskCx) => {
      try {
        const res = await exec("hotpot", ["status", "--workspace", folder], {
          cwd: folder,
          on: sp?.machine ?? null,
          timeout: 5000,
        });
        if (res.code === 0) {
          this.snapshot = parseHotpotJson(res.stdout, null);
        }
      } catch {
        // Keep last good snapshot; an unreachable engine renders quiet state
      } finally {
        this.loading = false;
        taskCx.notify();
      }
    });
  }

  // Persists one pill selection as the seat role's profile default through the
  // minimal `hotpot profile set` write path. An invalid payload throws inside
  // the builder and never reaches the engine.
  runProfileSet(cx, agent, role, model, thinking) {
    let args;
    try {
      args = profileSetArgs(agent, role, model, thinking);
    } catch {
      return;
    }
    const sp = this.activeSpace();
    const folder = sp?.folder || home();
    cx.spawn(async (taskCx) => {
      try {
        await exec("hotpot", args, {
          cwd: folder,
          on: sp?.machine ?? null,
          timeout: 5000,
        });
      } catch {
        // a failed write stays visible through the next status refresh
      } finally {
        this.refresh(taskCx);
      }
    });
  }

  // A named preset applies its coherent per-role map to the room's seats.
  applyPreset(cx, name) {
    const preset = PRESET_PROFILES[name];
    if (!preset || this.presetBusy) return;
    this.presetBusy = true;
    const sp = this.activeSpace();
    const folder = sp?.folder || home();
    cx.spawn(async (taskCx) => {
      try {
        for (const role of PROFILE_ROLES) {
          const payload = preset[role];
          if (!payload) continue;
          try {
            await exec("hotpot", profileSetArgs(payload.agent, role, payload.model, payload.thinking), {
              cwd: folder,
              on: sp?.machine ?? null,
              timeout: 5000,
            });
          } catch {
            // keep applying the remaining roles; status refresh shows reality
          }
        }
      } finally {
        this.presetBusy = false;
        this.refresh(taskCx);
      }
    });
  }

  selectAgent(entry, mode) {
    setShared(HOTPOT_SELECTED_KEY, entry);
    setShared(HOTPOT_VIEW_MODE_KEY, mode);
    if (mode === "pty" && entry.terminal) {
      run("layout.select", { stack: "main-stack", child: "main" });
      run("pane.focus", { terminal: entry.terminal });
    } else {
      run("layout.select", { stack: "main-stack", child: "inspector" });
    }
  }

  // ---------------------------------------------------------------------------
  // Needs Attention (pinned on top; quiet stays neutral)
  // ---------------------------------------------------------------------------

  renderAttention(t) {
    const view = attentionView(this.snapshot);
    const header = h_flex()
      .items_center()
      .justify_between()
      .children([
        div().text_size(11).font_semibold().text_color(t.muted).child("NEEDS ATTENTION"),
        view.quiet
          ? div().text_size(10).text_color(t.dim).child("✓ all clear")
          : div()
              .text_size(10)
              .font_semibold()
              .text_color(warningColor(t))
              .child(`${view.items.length} letter${view.items.length === 1 ? "" : "s"}`),
      ]);
    if (view.quiet) {
      return v_flex().gap(6).children([
        header,
        div()
          .id("attention-all-quiet")
          .p(8)
          .rounded(8)
          .bg(t.surface)
          .border(1)
          .border_color(t.border)
          .text_size(11)
          .text_color(t.dim)
          .child("All quiet — no stalled permissions, unread handbacks, or Human letters."),
      ]);
    }
    return v_flex().gap(6).children([
      header,
      ...view.items.map((item) =>
        v_flex()
          .id(`att-${item.key}`)
          .p(8)
          .gap(2)
          .rounded(8)
          .bg(t.surface)
          .border(1)
          .border_color(item.exception ? warningColor(t) : t.border)
          .children([
            h_flex()
              .items_center()
              .justify_between()
              .children(
                [
                  div()
                    .text_size(10)
                    .font_semibold()
                    .text_color(item.exception ? warningColor(t) : t.dim)
                    .child(item.kind.toUpperCase()),
                  item.downranked
                    ? div().text_size(10).text_color(t.dim).child("downranked")
                    : null,
                ].filter(Boolean),
              ),
            div().text_size(11).text_color(t.foreground).truncate().child(item.text || item.key),
          ]),
      ),
    ]);
  }

  // ---------------------------------------------------------------------------
  // Seat cards with segmented context bars + inline agent/model · thinking pill
  // ---------------------------------------------------------------------------

  togglePicker(seat, clickCx) {
    if (this.pickerFor === seat.id) {
      this.pickerFor = null;
      this.picker = null;
    } else {
      const agent = AGENT_OPTIONS.includes(seat.agent) ? seat.agent : AGENT_OPTIONS[0];
      this.pickerFor = seat.id;
      this.picker = {
        agent,
        model:
          MODEL_OPTIONS[agent].includes(seat.model) ? seat.model : MODEL_OPTIONS[agent][0],
        thinking: THINKING_OPTIONS.includes(seat.thinking) ? seat.thinking : THINKING_OPTIONS[0],
      };
    }
    clickCx.notify();
  }

  pick(field, value, clickCx) {
    if (!this.picker) return;
    this.picker[field] = value;
    if (field === "agent" && !MODEL_OPTIONS[value].includes(this.picker.model)) {
      this.picker.model = MODEL_OPTIONS[value][0];
    }
    clickCx.notify();
  }

  renderPickerChips(t, options, field, current) {
    return h_flex().gap(4).flex_wrap().children([
      ...options.map((option) =>
        div()
          .id(`pick-${field}-${option}`)
          .px(6)
          .py(2)
          .rounded(8)
          .bg(option === current ? t.selected : t.raised)
          .border(1)
          .border_color(option === current ? t.accent : t.border)
          .text_size(10)
          .text_color(option === current ? t.accent : t.muted)
          .child(option)
          .on_click((_e, clickCx) => {
            clickCx.stop_propagation();
            this.pick(field, option, clickCx);
          }),
      ),
    ]);
  }

  renderSeatPicker(t, seat, cx) {
    if (!this.picker) return null;
    return v_flex()
      .id(`picker-${seat.id}`)
      .p(6)
      .gap(4)
      .rounded(6)
      .bg(t.canvas)
      .border(1)
      .border_color(t.border)
      .children([
        div().text_size(10).font_semibold().text_color(t.dim).child("agent"),
        this.renderPickerChips(t, AGENT_OPTIONS, "agent", this.picker.agent),
        div().text_size(10).font_semibold().text_color(t.dim).child("model"),
        this.renderPickerChips(t, MODEL_OPTIONS[this.picker.agent], "model", this.picker.model),
        div().text_size(10).font_semibold().text_color(t.dim).child("thinking"),
        this.renderPickerChips(t, THINKING_OPTIONS, "thinking", this.picker.thinking),
        h_flex()
          .gap(6)
          .children([
            div()
              .id(`picker-apply-${seat.id}`)
              .px(8)
              .py(3)
              .rounded(6)
              .bg(t.solid)
              .text_size(10)
              .font_semibold()
              .text_color(t.solid_foreground)
              .child(`Apply to ${seat.role} default ✓`)
              .on_click((_e, clickCx) => {
                clickCx.stop_propagation();
                const { agent, model, thinking } = this.picker ?? {};
                this.pickerFor = null;
                this.picker = null;
                clickCx.notify();
                this.runProfileSet(cx, agent, seat.role, model, thinking);
              }),
            div()
              .id(`picker-cancel-${seat.id}`)
              .px(8)
              .py(3)
              .rounded(6)
              .bg(t.raised)
              .text_size(10)
              .text_color(t.muted)
              .child("✕")
              .on_click((_e, clickCx) => {
                clickCx.stop_propagation();
                this.pickerFor = null;
                this.picker = null;
                clickCx.notify();
              }),
          ]),
      ]);
  }

  renderContextBar(t, context) {
    const bar = contextBarModel(context);
    const cells = bar.segments.map((segment) => ({
      ...segment,
      cells: Math.round(segment.fraction * BAR_CELLS),
    }));
    return v_flex()
      .id("seat-context-bar")
      .gap(2)
      .children([
        h_flex()
          .items_center()
          .gap(3)
          .children(
            [
              ...cells.map((segment) =>
                div()
                  .id(`bar-${segment.kind}`)
                  .text_size(10)
                  .text_color(segmentColor(t, segment.kind))
                  .child("█".repeat(segment.cells)),
              ),
              bar.rotationTick
                ? div()
                    .id("bar-rotation-tick")
                    .px(4)
                    .rounded(4)
                    .bg(t.raised)
                    .text_size(10)
                    .font_semibold()
                    .text_color(warningColor(t))
                    .child(`⟳ rotate ${formatTokenCount(bar.total)}`)
                : null,
            ].filter(Boolean),
          ),
        h_flex()
          .items_center()
          .justify_between()
          .children([
            div()
              .text_size(10)
              .text_color(t.dim)
              .truncate()
              .child(
                bar.segments
                  .map((segment) => `${formatTokenCount(segment.value)} ${segment.label}`)
                  .join(" · "),
              ),
            div().text_size(10).text_color(t.muted).child(formatTokenCount(bar.total)),
          ]),
      ]);
  }

  renderSeatCard(t, seat, selectedId, cx) {
    const isSelected = selectedId === seat.id;
    const dotColor = statusColor(t, seat.status);
    const canPick = PROFILE_ROLES.includes(seat.role);
    return v_flex()
      .id(`seat-${seat.role}-${seat.id}`)
      .p(8)
      .gap(4)
      .rounded(8)
      .bg(isSelected ? t.selected : t.surface)
      .border(1)
      .border_color(isSelected ? t.accent : t.border)
      .hover((s) => s.bg(t.hover))
      .on_click(() => this.selectAgent(seat, "inspector"))
      .children([
        h_flex()
          .items_center()
          .justify_between()
          .children([
            h_flex()
              .items_center()
              .gap(6)
              .children([
                div()
                  .text_size(11)
                  .text_color(dotColor)
                  .when(seat.status === "working", (el) =>
                    el.frames(["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"], 90),
                  )
                  .when(seat.status !== "working", (el) => el.child("●")),
                div().text_size(13).font_semibold().text_color(t.foreground).child(seat.title),
                div()
                  .px(5)
                  .rounded(4)
                  .bg(t.raised)
                  .text_size(10)
                  .text_color(t.muted)
                  .child(seat.role),
              ]),
            div()
              .id(`seat-pty-${seat.id}`)
              .px(5)
              .rounded(4)
              .bg(t.canvas)
              .text_size(10)
              .text_color(t.accent)
              .child("PTY ↗")
              .on_click((_e, clickCx) => {
                clickCx.stop_propagation();
                this.selectAgent(seat, "pty");
              }),
          ]),
        this.renderContextBar(t, seat.context),
        h_flex()
          .items_center()
          .justify_between()
          .gap(4)
          .children([
            canPick
              ? div()
                  .id(`seat-pill-${seat.id}`)
                  .px(6)
                  .py(2)
                  .rounded(8)
                  .bg(this.pickerFor === seat.id ? t.selected : t.raised)
                  .border(1)
                  .border_color(this.pickerFor === seat.id ? t.accent : t.border)
                  .text_size(10)
                  .text_color(t.muted)
                  .truncate()
                  .child(`${seat.agent || "–"}/${seat.model || "–"} · ${seat.thinking || "–"} ⚙`)
                  .on_click((_e, clickCx) => {
                    clickCx.stop_propagation();
                    this.togglePicker(seat, clickCx);
                  })
              : div()
                  .text_size(10)
                  .text_color(t.dim)
                  .truncate()
                  .child(`${seat.agent || "–"}/${seat.model || "–"}`),
            div().text_size(10).text_color(t.dim).truncate().child(seat.activity || "idle"),
          ]),
        ...(this.pickerFor === seat.id ? [this.renderSeatPicker(t, seat, cx)] : []),
      ]);
  }

  // ---------------------------------------------------------------------------
  // Named room presets
  // ---------------------------------------------------------------------------

  renderPresets(t, cx) {
    return v_flex().gap(6).children([
      div().text_size(11).font_semibold().text_color(t.muted).child("ROOM PRESETS"),
      h_flex()
        .gap(4)
        .flex_wrap()
        .children([
          ...PRESET_NAMES.map((name) =>
            div()
              .id(`preset-${name.replace(/\s+/g, "-").toLowerCase()}`)
              .px(8)
              .py(4)
              .rounded(8)
              .bg(t.raised)
              .border(1)
              .border_color(t.border)
              .text_size(10)
              .text_color(t.foreground)
              .hover((s) => s.bg(t.hover))
              .child(this.presetBusy ? `${name} …` : name)
              .on_click((_e, clickCx) => {
                clickCx.stop_propagation();
                this.applyPreset(cx, name);
              }),
          ),
        ]),
    ]);
  }

  // ---------------------------------------------------------------------------
  // Assignment master–detail (state / disposition / verdict / attempt)
  // ---------------------------------------------------------------------------

  renderTaskCard(t, task, selectedId) {
    const isSelected = selectedId === task.correlation;
    const verdictColor = statusColor(t, task.verdict || task.state);
    return v_flex()
      .id(`task-${task.id}`)
      .p(8)
      .gap(4)
      .rounded(8)
      .bg(isSelected ? t.selected : t.surface)
      .border(1)
      .border_color(isSelected ? t.accent : t.border)
      .hover((s) => s.bg(t.hover))
      .on_click(() =>
        this.selectAgent(
          {
            id: task.correlation || task.id,
            role: "peer",
            task: task.id,
            title: `Peer · ${task.id}`,
            provider: task.agent || "claude",
            model: task.model || "sonnet",
            terminal: task.terminal || null,
            status: task.state || "idle",
            context_tokens: task.context_tokens || 0,
          },
          "inspector",
        ),
      )
      .children([
        h_flex()
          .items_center()
          .justify_between()
          .children([
            h_flex()
              .items_center()
              .gap(6)
              .children([
                div().text_size(12).font_semibold().text_color(t.foreground).child(task.id),
                div()
                  .px(5)
                  .rounded(4)
                  .bg(t.raised)
                  .text_size(10)
                  .text_color(t.muted)
                  .child(task.disposition || "engineer"),
              ]),
            div()
              .px(6)
              .rounded(4)
              .bg(t.canvas)
              .text_size(10)
              .font_semibold()
              .text_color(verdictColor)
              .child(
                `${(task.verdict ? `gate ${task.verdict}` : "DRAFT").toUpperCase()} · ${task.attempt || 1}/3`,
              ),
          ]),
        div().text_size(10).text_color(t.dim).truncate().child(task.state || "Draft"),
        div().text_size(11).text_color(t.muted).truncate().child(task.summary || "Bounded SLP Task"),
        div()
          .text_size(11)
          .text_color(t.dim)
          .truncate()
          .child(task.activity || "Click to inspect Peer Thinking & Tool Calls"),
      ]);
  }

  render(cx) {
    const t = theme();
    const sp = this.activeSpace();
    const selected = shared(HOTPOT_SELECTED_KEY);
    const selectedId = selected?.id ?? null;

    const seats = this.snapshot?.seats ?? [];
    const tasks = this.snapshot?.tasks ?? [];
    const mailbox = this.snapshot?.mailbox ?? [];

    return v_flex()
      .id("root")
      .size_full()
      .bg(t.chrome)
      .p(10)
      .gap(10)
      .overflow_y_scrollbar()
      .children([
        h_flex()
          .items_center()
          .justify_between()
          .children([
            h_flex()
              .items_center()
              .gap(6)
              .children([
                div().text_size(14).font_semibold().text_color(t.foreground).child("🍲 Hotpot SLP"),
                div()
                  .px(6)
                  .rounded(4)
                  .bg(t.raised)
                  .text_size(10)
                  .text_color(t.accent)
                  .child(this.snapshot?.mode?.toUpperCase() ?? "SLP"),
              ]),
            div()
              .text_size(11)
              .text_color(t.dim)
              .truncate()
              .child(sp?.name ?? "local"),
          ]),

        // Needs Attention stays pinned above every other section.
        this.renderAttention(t),

        v_flex()
          .gap(6)
          .children([
            div()
              .text_size(11)
              .font_semibold()
              .text_color(t.muted)
              .child("SEATS (CLICK TO INSPECT THINKING)"),
            ...(seats.length > 0
              ? seats.map((seat) => this.renderSeatCard(t, seat, selectedId, cx))
              : [
                  div()
                    .p(8)
                    .rounded(8)
                    .bg(t.surface)
                    .text_size(11)
                    .text_color(t.dim)
                    .child(this.snapshot ? "No seats launched for this Project yet." : "Waiting for hotpot …"),
                ]),
          ]),

        this.renderPresets(t, cx),

        v_flex()
          .gap(6)
          .children([
            h_flex()
              .items_center()
              .justify_between()
              .children([
                div().text_size(11).font_semibold().text_color(t.muted).child("ASSIGNMENTS"),
                div().text_size(11).text_color(t.dim).child(`${tasks.length} total`),
              ]),
            ...(tasks.length > 0
              ? tasks.map((task) => this.renderTaskCard(t, task, selectedId))
              : [
                  div()
                    .p(8)
                    .rounded(8)
                    .bg(t.surface)
                    .text_size(11)
                    .text_color(t.dim)
                    .child("No Assignments in this Project yet."),
                ]),
          ]),

        v_flex()
          .gap(6)
          .children([
            div().text_size(11).font_semibold().text_color(t.muted).child("LIVE MAILBOX (LEAD ↔ PEER)"),
            ...(mailbox.length > 0
              ? mailbox.slice(0, 6).map((msg, idx) =>
                  v_flex()
                    .id(`mail-${idx}`)
                    .p(6)
                    .gap(2)
                    .rounded(6)
                    .bg(t.surface)
                    .border(1)
                    .border_color(t.border)
                    .children([
                      h_flex()
                        .items_center()
                        .justify_between()
                        .children([
                          div()
                            .text_size(10)
                            .font_semibold()
                            .text_color(t.accent)
                            .child(`${msg.from} → ${msg.to}`),
                          div().text_size(10).text_color(t.dim).child(msg.kind || "mail"),
                        ]),
                      div().text_size(11).text_color(t.foreground).truncate().child(msg.summary || ""),
                    ]),
                )
              : [
                  div()
                    .p(8)
                    .rounded(8)
                    .bg(t.surface)
                    .text_size(11)
                    .text_color(t.dim)
                    .child("Mailbox idle — claims and directives stream here."),
                ]),
          ]),
      ]);
  }
}
