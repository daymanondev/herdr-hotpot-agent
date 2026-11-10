// Right-hand Hotpot panel: Seat Slots, peer tree, header presets, and strict
// *Quiet When Quiet* ergonomics (ticket 05).
//
// The panel always shows the Project's Supervisor and Lead Seat Slots — live
// seat data when running, the role's configured profile defaults plus a
// 1-click `Start` when not — with dispatched Peers as indented one-line tree
// rows (├ / └) under Lead. Room presets live in a compact header dropdown,
// and quiet surfaces stay quiet: attention collapses to a single `All clear`
// indicator, the mailbox (Activity) hides while empty, and Assignments folds
// to its one-line header. Section headers are sentence-case with fold
// chevrons and right-aligned counts, over the live `hotpot status` snapshot
// (streamed in through `watchFolder` on the runtime root, with a slow timer
// fallback for changes that have no file event).

import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { exec, spaces, theme, run, shared, setShared, home, watchFolder } from "herdr";
import {
  HOTPOT_SELECTED_KEY,
  HOTPOT_VIEW_MODE_KEY,
  PROFILE_ROLES,
  AGENT_OPTIONS,
  MODEL_OPTIONS,
  THINKING_OPTIONS,
  PRESET_PROFILES,
  activeSpace,
  spaceFolder,
  hotpotExecOptions,
  attentionView,
  contextBarModel,
  formatTokenCount,
  profileSetArgs,
  parseHotpotJson,
  statusColor,
  seatSlotsModel,
  sectionVisibility,
  presetMenu,
  startSlotSteps,
} from "./protocol.js";

function warningColor(t) {
  return t.blocked ?? t.ansi1;
}

export default class HotpotPanel extends View {
  init(_props, cx) {
    this.snapshot = null;
    this.lastFolder = null;
    this.loading = false;
    // The runtime directory currently subscribed via `watchFolder` (null until
    // a snapshot names one, or after a rejected watch re-arms it).
    this.watchedDir = null;
    // The seat role whose inline pill picker is open, plus its draft selection.
    this.pickerFor = null;
    this.picker = null;
    this.presetBusy = false;
    // The header preset dropdown's open state.
    this.presetMenuOpen = false;
    // The seat role whose `Start` sequence is currently exec-ing.
    this.starting = null;
    // Fallback cadence only: ledger changes stream in through `watchRuntime`
    // (`watchFolder` on Hotpot Home's runtime root, spec "Implementation
    // Decisions"; ADR 0001). The timer covers what has no file event — `hc`
    // terminal status flips and attention letters that fall due by time —
    // and keeps the panel alive if the platform refuses the watch.
    cx.timer.every(10000, (timerCx) => this.refresh(timerCx));
    this.refresh(cx);
  }

  refresh(cx) {
    const sp = activeSpace(spaces());
    const folder = spaceFolder(sp, home());
    if (this.loading && this.lastFolder === folder) return;
    this.loading = true;
    this.lastFolder = folder;

    cx.spawn(async (taskCx) => {
      try {
        const res = await exec(
          "hotpot",
          ["status", "--workspace", folder],
          hotpotExecOptions(sp, folder),
        );
        if (res.code === 0) {
          this.snapshot = parseHotpotJson(res.stdout, null);
          this.watchRuntime(this.snapshot?.runtimeDir ?? null, taskCx);
        }
      } catch {
        // Keep last good snapshot; an unreachable engine renders the
        // unlaunched slots with no profile defaults
      } finally {
        this.loading = false;
        taskCx.notify();
      }
    });
  }

  // The event-driven half of the channel between the mod and the engine:
  // once a status snapshot
  // names the runtime root under Hotpot Home, subscribe to it with Herdr's
  // native `watchFolder` and re-run `hotpot status` on every change batch.
  // A new directory replaces the subscription; a rejected watch (e.g. the
  // platform refusing it) re-arms on the next snapshot that names one.
  watchRuntime(dir, cx) {
    if (this.watchedDir === dir) return;
    // Assigning null (runtime state removed from Hotpot Home) retires any
    // prior subscription: its loop exits at the next iteration.
    this.watchedDir = dir ?? null;
    if (!dir) return;
    cx.spawn(async (watchCx) => {
      try {
        while (this.watchedDir === dir) {
          await watchFolder(dir);
          if (this.watchedDir === dir) this.refresh(watchCx);
        }
      } catch {
        this.watchedDir = null;
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
    const sp = activeSpace(spaces());
    const folder = spaceFolder(sp, home());
    cx.spawn(async (taskCx) => {
      try {
        await exec("hotpot", args, hotpotExecOptions(sp, folder));
      } catch {
        // a failed write stays visible through the next status refresh
      } finally {
        this.refresh(taskCx);
      }
    });
  }

  // The 1-click `Start` on an unlaunched Seat Slot: run the argv sequence the
  // pure `startSlotSteps` builder yields — provisioning first when the
  // snapshot says the Project is not provisioned, then the seat launch with
  // the role's profile agent — with a busy state and a refresh afterwards.
  startSlot(cx, slot) {
    if (this.starting) return;
    const sp = activeSpace(spaces());
    const folder = spaceFolder(sp, home());
    let steps;
    try {
      steps = startSlotSteps(slot, this.snapshot, folder);
    } catch {
      return; // no configured agent for the role — nothing to start
    }
    this.starting = slot.role;
    cx.notify();
    cx.spawn(async (taskCx) => {
      try {
        for (const step of steps) {
          try {
            await exec("hotpot", step.args, hotpotExecOptions(sp, folder, step.timeoutMs ?? 5000));
          } catch {
            // stop at the first failed step; the refresh below shows reality
            break;
          }
        }
      } finally {
        this.starting = null;
        this.refresh(taskCx);
      }
    });
  }

  // A named preset applies its coherent per-role map to the room's seats.
  applyPreset(cx, name) {
    const preset = PRESET_PROFILES[name];
    if (!preset || this.presetBusy) return;
    this.presetBusy = true;
    const sp = activeSpace(spaces());
    const folder = spaceFolder(sp, home());
    cx.spawn(async (taskCx) => {
      try {
        for (const role of PROFILE_ROLES) {
          const payload = preset[role];
          if (!payload) continue;
          try {
            await exec(
              "hotpot",
              profileSetArgs(payload.agent, role, payload.model, payload.thinking),
              hotpotExecOptions(sp, folder),
            );
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
  // Shared section chrome: sentence-case headers with fold chevron + count
  // ---------------------------------------------------------------------------

  sectionHeader(t, title, count, folded) {
    return h_flex()
      .items_center()
      .gap(4)
      .children([
        svg(`mods/workspace-chrome/chevron-${folded ? "right" : "down"}.svg`)
          .size(11)
          .flex_none()
          .text_color(t.dim),
        div().text_size(11.5).font_medium().text_color(t.dim).child(title),
        div().flex_1(),
        div().text_size(11).text_color(t.dim).child(String(count)),
      ]);
  }

  // The terminal affordance every launched row keeps on its right side:
  // focuses the seat's live `hc` terminal tab. Rows without a terminal
  // address render no affordance at all.
  terminalButton(t, id, entry) {
    return div()
      .id(id)
      .p(3)
      .rounded(4)
      .bg(t.canvas)
      .child(
        svg("mods/workspace-chrome/terminal.svg").size(11).flex_none().text_color(t.accent),
      )
      .on_click((_e, clickCx) => {
        clickCx.stop_propagation();
        this.selectAgent(entry, "pty");
      });
  }

  // ---------------------------------------------------------------------------
  // Attention (pinned on top; quiet collapses to the All clear indicator)
  // ---------------------------------------------------------------------------

  renderAttention(t) {
    const view = attentionView(this.snapshot);
    if (view.quiet) {
      // Strict Quiet When Quiet: one quiet indicator, no placeholder box.
      return h_flex()
        .id("attention-all-clear")
        .items_center()
        .gap(5)
        .children([
          svg("mods/workspace-chrome/check.svg").size(11).flex_none().text_color(t.dim),
          div().text_size(11).text_color(t.dim).child("All clear"),
        ]);
    }
    return v_flex().gap(6).children([
      h_flex()
        .items_center()
        .justify_between()
        .children([
          div().text_size(11.5).font_medium().text_color(t.dim).child("Attention"),
          div()
            .text_size(10)
            .font_semibold()
            .text_color(warningColor(t))
            .child(`${view.items.length} letter${view.items.length === 1 ? "" : "s"}`),
        ]),
      ...view.items.map((item) => {
        const kind = item.kind
          ? item.kind.charAt(0).toUpperCase() + item.kind.slice(1)
          : "Attention";
        return v_flex()
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
                    .child(kind),
                  item.downranked
                    ? div().text_size(10).text_color(t.dim).child("downranked")
                    : null,
                ].filter(Boolean),
              ),
            div().text_size(11).text_color(t.foreground).truncate().child(item.text || item.key),
          ]);
      }),
    ]);
  }

  // ---------------------------------------------------------------------------
  // Seat Slots: always-present Supervisor / Lead, live or with Start
  // ---------------------------------------------------------------------------

  togglePicker(slot, clickCx) {
    if (this.pickerFor === slot.role) {
      this.pickerFor = null;
      this.picker = null;
    } else {
      const agent = AGENT_OPTIONS.includes(slot.agent) ? slot.agent : AGENT_OPTIONS[0];
      this.pickerFor = slot.role;
      this.picker = {
        agent,
        model:
          MODEL_OPTIONS[agent].includes(slot.model) ? slot.model : MODEL_OPTIONS[agent][0],
        thinking: THINKING_OPTIONS.includes(slot.thinking) ? slot.thinking : THINKING_OPTIONS[0],
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

  renderSeatPicker(t, slot, cx) {
    if (!this.picker) return null;
    return v_flex()
      .id(`picker-${slot.role}`)
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
              .id(`picker-apply-${slot.role}`)
              .px(8)
              .py(3)
              .rounded(6)
              .bg(t.solid)
              .text_size(10)
              .font_semibold()
              .text_color(t.solid_foreground)
              .child(`Apply to ${slot.role} default ✓`)
              .on_click((_e, clickCx) => {
                clickCx.stop_propagation();
                const { agent, model, thinking } = this.picker ?? {};
                this.pickerFor = null;
                this.picker = null;
                clickCx.notify();
                this.runProfileSet(cx, agent, slot.role, model, thinking);
              }),
            div()
              .id(`picker-cancel-${slot.role}`)
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
    return h_flex()
      .id("seat-context-bar")
      .items_center()
      .gap(3)
      .children(
        [
          div()
            .text_size(10)
            .text_color(t.dim)
            .truncate()
            .child(
              bar.segments
                .map((segment) => `${formatTokenCount(segment.value)} ${segment.label}`)
                .join(" · "),
            ),
          div().flex_1(),
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
          div().text_size(10).text_color(t.muted).child(formatTokenCount(bar.total)),
        ].filter(Boolean),
      );
  }

  renderSeatSlot(t, slot, selectedId, cx) {
    const isSelected = slot.launched && selectedId === slot.seat?.id;
    const busy = this.starting === slot.role;
    const canPick = PROFILE_ROLES.includes(slot.role);
    const affordances = [];
    // An unlaunched slot with a configured agent carries the 1-click Start;
    // a launched seat carries its live terminal affordance instead.
    if (!slot.launched && slot.agent) {
      affordances.push(
        div()
          .id(`start-${slot.role}`)
          .px(8)
          .py(2)
          .rounded(6)
          .bg(t.solid)
          .text_size(10)
          .font_semibold()
          .text_color(t.solid_foreground)
          .child(busy ? "Starting …" : "Start")
          .on_click((_e, clickCx) => {
            clickCx.stop_propagation();
            this.startSlot(cx, slot);
          }),
      );
    }
    if (slot.terminal) {
      affordances.push(
        this.terminalButton(t, `seat-pty-${slot.seat?.id ?? slot.role}`, {
          ...(slot.seat ?? {}),
          terminal: slot.terminal,
        }),
      );
    }
    return v_flex()
      .id(`seat-${slot.role}`)
      .p(8)
      .gap(4)
      .rounded(8)
      .bg(isSelected ? t.selected : t.surface)
      .border(1)
      .border_color(isSelected ? t.accent : t.border)
      .when(slot.launched, (el) =>
        el
          .hover((s) => s.bg(t.hover))
          .on_click(() => this.selectAgent(slot.seat, "inspector")),
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
                div()
                  .text_size(11)
                  .text_color(statusColor(t, slot.status))
                  .when(slot.status === "working", (el) =>
                    el.frames(["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"], 90),
                  )
                  .when(slot.status !== "working", (el) => el.child("●")),
                div().text_size(13).font_semibold().text_color(t.foreground).child(slot.title),
                div()
                  .px(5)
                  .rounded(4)
                  .bg(t.raised)
                  .text_size(10)
                  .text_color(t.muted)
                  .child(slot.role),
              ]),
            h_flex().items_center().gap(4).children(affordances),
          ]),
        ...(slot.launched && slot.seat?.context
          ? [this.renderContextBar(t, slot.seat.context)]
          : []),
        h_flex()
          .items_center()
          .justify_between()
          .gap(4)
          .children([
            canPick
              ? div()
                  .id(`seat-pill-${slot.role}`)
                  .px(6)
                  .py(2)
                  .rounded(8)
                  .bg(this.pickerFor === slot.role ? t.selected : t.raised)
                  .border(1)
                  .border_color(this.pickerFor === slot.role ? t.accent : t.border)
                  .text_size(10)
                  .text_color(t.muted)
                  .truncate()
                  .child(`${slot.agent || "–"}/${slot.model || "–"} · ${slot.thinking || "–"}`)
                  .on_click((_e, clickCx) => {
                    clickCx.stop_propagation();
                    this.togglePicker(slot, clickCx);
                  })
              : div()
                  .text_size(10)
                  .text_color(t.dim)
                  .truncate()
                  .child(`${slot.agent || "–"}/${slot.model || "–"}`),
            slot.launched
              ? div()
                  .text_size(10)
                  .text_color(t.dim)
                  .truncate()
                  .child(slot.seat?.activity || "idle")
              : null,
          ].filter(Boolean)),
        ...(this.pickerFor === slot.role ? [this.renderSeatPicker(t, slot, cx)] : []),
      ]);
  }

  // One dispatched Peer under Lead: a single compact indented tree row
  // carrying the assignment, disposition, gate/attempt badge, and terminal
  // affordance on that one line — never a duplicate large card.
  renderPeerRow(t, peer, selectedId) {
    const isSelected = selectedId === peer.seat?.id;
    const badge = `${peer.gate ? `gate ${peer.gate}` : "draft"} · ${peer.attempt ?? 1}/3`;
    return h_flex()
      .id(`peer-${peer.id ?? peer.assignment ?? "row"}`)
      .pl(26)
      .pr(8)
      .h(26)
      .flex_none()
      .gap(4)
      .items_center()
      .rounded(8)
      .when(isSelected, (el) => el.bg(t.selected))
      .hover((s) => s.bg(t.hover))
      .on_click(() => this.selectAgent(peer.seat, "inspector"))
      .children(
        [
          div().w(10).flex_none().text_color(t.dim).child(peer.connector),
          div()
            .text_size(11)
            .font_semibold()
            .text_color(t.foreground)
            .truncate()
            .child(peer.assignment ?? peer.id ?? "peer"),
          peer.disposition
            ? div()
                .px(5)
                .rounded(4)
                .bg(t.raised)
                .text_size(10)
                .text_color(t.muted)
                .child(peer.disposition)
            : null,
          div()
            .px(5)
            .rounded(4)
            .bg(t.canvas)
            .text_size(10)
            .font_semibold()
            .text_color(statusColor(t, peer.gate))
            .child(badge),
          div().flex_1(),
          peer.terminal
            ? this.terminalButton(t, `peer-pty-${peer.id ?? peer.assignment}`, {
                ...(peer.seat ?? {}),
                terminal: peer.terminal,
              })
            : null,
        ].filter(Boolean),
      );
  }

  // ---------------------------------------------------------------------------
  // Presets: a compact dropdown in the panel header (their only surface)
  // ---------------------------------------------------------------------------

  renderPresetMenuButton(t) {
    return div()
      .id("preset-menu-button")
      .px(6)
      .py(2)
      .rounded(6)
      .bg(t.raised)
      .border(1)
      .border_color(t.border)
      .hover((s) => s.bg(t.hover))
      .on_click((_e, clickCx) => {
        clickCx.stop_propagation();
        this.presetMenuOpen = !this.presetMenuOpen;
        clickCx.notify();
      })
      .child(
        h_flex()
          .items_center()
          .gap(4)
          .children([
            div().text_size(10).text_color(t.muted).child("Presets"),
            svg(`mods/workspace-chrome/chevron-${this.presetMenuOpen ? "down" : "right"}.svg`)
              .size(9)
              .flex_none(),
          ]),
      );
  }

  renderPresetMenu(t, cx) {
    return v_flex()
      .id("preset-menu")
      .gap(2)
      .p(4)
      .rounded(8)
      .bg(t.surface)
      .border(1)
      .border_color(t.border)
      .children([
        ...presetMenu().map((item) =>
          div()
            .id(`preset-${item.id}`)
            .px(8)
            .py(4)
            .rounded(6)
            .text_size(11)
            .text_color(t.foreground)
            .hover((s) => s.bg(t.hover))
            .child(this.presetBusy ? `${item.name} …` : item.name)
            .on_click((_e, clickCx) => {
              clickCx.stop_propagation();
              this.presetMenuOpen = false;
              clickCx.notify();
              this.applyPreset(cx, item.name);
            }),
        ),
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
            // Real dispatch-record values only — the inspector renders missing
            // ones as `–` rather than a mocked provider/model.
            provider: task.agent || null,
            model: task.model || null,
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
              .child(`${task.verdict ? `gate ${task.verdict}` : "draft"} · ${task.attempt || 1}/3`),
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

  // ---------------------------------------------------------------------------
  // Mailbox (Activity): hidden entirely while quiet
  // ---------------------------------------------------------------------------

  renderActivity(t, mailbox) {
    return v_flex().gap(6).children([
      this.sectionHeader(t, "Activity", mailbox.length, false),
      ...mailbox.slice(0, 6).map((msg, idx) =>
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
      ),
    ]);
  }

  render(cx) {
    const t = theme();
    const sp = activeSpace(spaces());
    const selected = shared(HOTPOT_SELECTED_KEY);
    const selectedId = selected?.id ?? null;

    const sections = sectionVisibility(this.snapshot);
    const slots = seatSlotsModel(this.snapshot);
    const tasks = Array.isArray(this.snapshot?.tasks) ? this.snapshot.tasks : [];
    const mailbox = Array.isArray(this.snapshot?.mailbox) ? this.snapshot.mailbox : [];

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
                div().text_size(14).font_semibold().text_color(t.foreground).child("Hotpot"),
                div()
                  .px(6)
                  .rounded(4)
                  .bg(t.raised)
                  .text_size(10)
                  .text_color(t.accent)
                  .child(this.snapshot?.mode?.toUpperCase() ?? "SLP"),
              ]),
            h_flex()
              .items_center()
              .gap(6)
              .children([
                div().text_size(11).text_color(t.dim).truncate().child(sp?.name ?? "local"),
                this.renderPresetMenuButton(t),
              ]),
          ]),
        ...(this.presetMenuOpen ? [this.renderPresetMenu(t, cx)] : []),

        // Attention stays pinned above every other section; quiet collapses
        // it to the single All clear indicator.
        this.renderAttention(t),

        // The Seats section always renders: the two Seat Slots plus the
        // child Peer tree under Lead — never an empty placeholder box.
        v_flex()
          .gap(6)
          .children([
            this.sectionHeader(t, "Seats", sections.seats.count, false),
            this.renderSeatSlot(t, slots.supervisor, selectedId, cx),
            this.renderSeatSlot(t, slots.lead, selectedId, cx),
            ...slots.lead.peers.map((peer) => this.renderPeerRow(t, peer, selectedId)),
          ]),

        v_flex()
          .gap(6)
          .children([
            this.sectionHeader(
              t,
              "Assignments",
              sections.assignments.count,
              sections.assignments.folded,
            ),
            ...(sections.assignments.folded
              ? []
              : tasks.map((task) => this.renderTaskCard(t, task, selectedId))),
          ]),

        ...(sections.activity.hidden ? [] : [this.renderActivity(t, mailbox)]),
      ]);
  }
}
