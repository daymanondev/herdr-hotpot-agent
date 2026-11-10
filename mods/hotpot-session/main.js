// Center-stage Session Inspector, in Herdr's native visual grammar (ticket
// nui/08): a compact header (the seat's vendor icon resolved from Herdr's
// `knownAgents()` registry, a role badge, the `3px` geometric context bar with
// its `1px` rotation tick, and a `Terminal · Alt+T` action that focuses the
// seat's live `hc` terminal — disabled with a hint while none exists), a
// compact collapsible timeline (Thinking rows fold on native chevrons and
// expand by default while the seat's turn is active; Tool Call rows stay
// single `28px` lines that expand on click to their full payload), and the
// prompt box to the seat's terminal. Zero emoji characters: every icon is a
// vector chrome glyph or a vendor icon descriptor.

import { View, div, svg, image } from "gpui-kit";
import { v_flex, h_flex, InputState, Input, TextView } from "gpui-base";
import {
  exec,
  hc,
  spaces,
  theme,
  run,
  shared,
  home,
  nextCommand,
  notify,
  watchFolder,
  knownAgents,
} from "herdr";
import {
  HOTPOT_SELECTED_KEY,
  activeSpace,
  spaceFolder,
  hotpotExecOptions,
  parentDir,
  statusColor,
  parseHotpotJson,
  transcriptKindMeta,
  isNoticeKind,
  isSeatActive,
  defaultItemExpanded,
  ptyTarget,
  toolCallCollapsedModel,
  inspectorHeaderModel,
  agentIconModel,
  ROTATION_MARK_TOKENS,
  formatTokenCount,
} from "./protocol.js";

export default class HotpotSessionInspector extends View {
  init(_props, cx) {
    this.items = [];
    // The last parsed `hotpot inspect` payload: its live `context` segments
    // back the header's geometric context bar.
    this.inspection = null;
    // Per-item collapse overrides for the selected seat: an absent key follows
    // the default expansion (`defaultItemExpanded` for Thinking,
    // `toolCallCollapsedModel` for Tool Calls); true/false pins the row.
    this.collapsed = {};
    this.lastAgentId = null;
    this.loading = false;
    // The transcript directory currently subscribed via `watchFolder` (null
    // until an inspection names a transcript, after a seat switch, or when a
    // watch is rejected).
    this.watchedDir = null;
    // Herdr's program table (`knownAgents()`): the vendor icon registry the
    // header and message rows resolve their agent icons against, asked for
    // once per view. A rejected ask leaves it empty — every icon then falls
    // back to the terminal glyph.
    this.agentRegistry = [];
    cx.spawn(async (taskCx) => {
      try {
        this.agentRegistry = await knownAgents({
          on: activeSpace(spaces())?.machine ?? null,
        });
      } catch {
        this.agentRegistry = [];
      }
      taskCx.notify();
    });
    this.promptInput = InputState.new(cx, { placeholder: "Send directive or message to this session..." });
    // Fallback cadence only: transcript writes stream in through
    // `watchTranscript` (`watchFolder` on the session directory the
    // architecture doc prescribes). The timer covers selection changes,
    // seats whose transcript appears later, and a refused watch.
    cx.timer.every(3000, (timerCx) => this.refresh(timerCx));
    // Key handling for the `Alt+T` terminal jump: `keys.json` binds
    // `alt-t` to `hotpot-session:pty-focus`, delivered here.
    cx.spawn(async (taskCx) => {
      for (;;) {
        const command = await nextCommand();
        if (command === "pty-focus") {
          this.focusPty(taskCx);
        }
      }
    });
    this.refresh(cx);
  }

  refresh(cx) {
    const selected = shared(HOTPOT_SELECTED_KEY);
    // A selection without a real seat id has nothing to inspect — no
    // fabricated fallback id reaches the engine.
    if (!selected?.id || this.loading) return;
    // A new seat selection starts from a clean transcript and default expansion.
    if (selected.id !== this.lastAgentId) {
      this.lastAgentId = selected.id;
      this.collapsed = {};
      this.items = [];
      this.inspection = null;
    }
    const sp = activeSpace(spaces());
    const folder = spaceFolder(sp, home());
    this.loading = true;

    cx.spawn(async (taskCx) => {
      try {
        const args = [
          "inspect",
          "--workspace",
          folder,
          "--agent",
          selected.id,
          // `--role` is accepted and ignored by the engine (the seat resolves
          // by correlation id); sent for CLI readability only.
          "--role",
          selected.role || "lead",
        ];
        if (selected.terminal) {
          args.push("--terminal", selected.terminal);
        }
        const res = await exec("hotpot", args, hotpotExecOptions(sp, folder));
        if (res.code === 0) {
          const parsed = parseHotpotJson(res.stdout, null);
          if (parsed && Array.isArray(parsed.items)) {
            this.items = parsed.items;
            this.inspection = parsed;
            this.watchTranscript(parentDir(parsed.transcript), taskCx);
          }
        }
      } catch {
        // Keep existing transcript slice on transient error
      } finally {
        this.loading = false;
        taskCx.notify();
      }
    });
  }

  // The event-driven half of the inspector's data channel: once an inspection
  // names the transcript file it parsed, subscribe to the session directory
  // holding it with Herdr's native `watchFolder` and re-inspect on every
  // change batch — new transcript lines reach the view as they land. A seat
  // switch replaces the subscription; a rejected watch re-arms on the next
  // inspection that finds a transcript.
  watchTranscript(dir, cx) {
    if (this.watchedDir === dir) return;
    // Assigning null (a seat without a transcript yet) retires any prior
    // subscription: its loop exits at the next iteration.
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

  // One-click `Terminal · Alt+T`: focus the selected seat's live `hc`
  // terminal in the main stack. Without a terminal the jump is impossible;
  // say so instead of silently switching to an empty pane.
  focusPty(cx) {
    const target = ptyTarget(shared(HOTPOT_SELECTED_KEY));
    if (!target) {
      notify({
        title: "Terminal unavailable",
        body: "This seat has no live hc terminal to jump to.",
        level: "warning",
      });
      cx.notify();
      return;
    }
    run("layout.select", { stack: "main-stack", child: "main" });
    run("pane.focus", { terminal: target });
  }

  sendPrompt(cx) {
    const selected = shared(HOTPOT_SELECTED_KEY);
    const target = ptyTarget(selected);
    const text = (this.promptInput?.value?.() ?? "").trim();
    if (!selected || !target || !text) return;
    this.promptInput?.set_value?.("", cx);

    cx.spawn(async (taskCx) => {
      await hc(["send", target, text]);
      this.refresh(taskCx);
    });
  }

  // ---------------------------------------------------------------------------
  // Native chrome helpers (the panel's design system, ticket 06)
  // ---------------------------------------------------------------------------

  warningColor(t) {
    return t.blocked ?? t.ansi1;
  }

  // An icon descriptor rendered natively: chrome glyphs as one-color `svg`s
  // in the given color, vendor descriptors through the 06 icon rule against
  // Herdr's program table (`svg(icon)` tinted when `tint`, else the
  // full-color `image(icon)`; the terminal glyph as fallback). `null`
  // descriptors render nothing.
  renderIconDescriptor(t, descriptor, agent, size = 12, color = null) {
    if (!descriptor) return null;
    if (descriptor.kind === "vendor") {
      const icon = agentIconModel(agent, this.agentRegistry);
      if (icon.tint) {
        return svg(icon.icon).size(size).flex_none().text_color(t.foreground);
      }
      return image(icon.icon).w(size).h(size).flex_none();
    }
    return svg(descriptor.icon).size(size).flex_none().text_color(color ?? t.dim);
  }

  // The row's fold affordance: a native chevron pointing down while the row
  // is expanded and right while it is collapsed.
  chevronIcon(t, expanded, size = 11) {
    const icon = expanded
      ? "mods/workspace-chrome/chevron-down.svg"
      : "mods/workspace-chrome/chevron-right.svg";
    return svg(icon).size(size).flex_none().text_color(t.dim);
  }

  // The seat's context window as one geometric bar (same construction as the
  // panel, ticket 06): a `3px` rounded track carrying proportional
  // `Cache Read` / `Fresh Input` / `Output` fills, a `1px` warning-colored
  // tick at the rotation mark's share of the track, and the compact `150k`
  // badge once usage reaches the mark.
  renderContextBar(t, bar) {
    const segmentColors = { cacheRead: t.muted, freshInput: t.accent, output: t.dim };
    return h_flex()
      .id("inspector-context-bar")
      .items_center()
      .gap(4)
      .children(
        [
          div()
            .id("context-bar-track")
            .relative()
            .flex_1()
            .h(3)
            .rounded(1.5)
            .overflow_hidden()
            .bg(t.canvas)
            .children(
              [
                ...bar.segments
                  .filter((segment) => segment.fraction > 0)
                  .map((segment) =>
                    div()
                      .id(`context-bar-${segment.kind}`)
                      .h(3)
                      .flex_grow(segment.fraction)
                      .bg(segmentColors[segment.kind] ?? t.dim),
                  ),
                bar.rotationTick
                  ? div()
                      .id("bar-rotation-tick")
                      .absolute()
                      .top(0)
                      .bottom(0)
                      .w(1)
                      .left(`${((100 * ROTATION_MARK_TOKENS) / bar.total).toFixed(2)}%`)
                      .bg(this.warningColor(t))
                  : null,
              ].filter(Boolean),
            ),
          bar.rotationTick
            ? div()
                .id("bar-rotation-badge")
                .px(4)
                .rounded(4)
                .bg(t.raised)
                .text_size(10)
                .font_semibold()
                .text_color(this.warningColor(t))
                .child(formatTokenCount(ROTATION_MARK_TOKENS))
            : null,
          div().text_size(10).text_color(t.muted).child(formatTokenCount(bar.total)),
        ].filter(Boolean),
      );
  }

  // ---------------------------------------------------------------------------
  // The compact native header (ticket nui/08)
  // ---------------------------------------------------------------------------

  renderHeader(t, cx, header) {
    const action = header.terminalAction;
    return v_flex()
      .id("session-header")
      .flex_none()
      .p(8)
      .gap(6)
      .rounded(8)
      .bg(t.chrome)
      .border(1)
      .border_color(t.border)
      .children([
        h_flex()
          .items_center()
          .gap(8)
          .children(
            [
              this.renderIconDescriptor(t, header.icon, header.agent, 14),
              div()
                .text_size(13)
                .font_semibold()
                .text_color(t.foreground)
                .child(header.title || "Session Inspector"),
              header.role
                ? div()
                    .px(5)
                    .rounded(4)
                    .bg(t.raised)
                    .text_size(10)
                    .text_color(t.muted)
                    .child(header.role)
                : div()
                    .id("header-no-seat")
                    .px(5)
                    .rounded(4)
                    .bg(t.raised)
                    .text_size(10)
                    .text_color(t.dim)
                    .child("no seat selected"),
              header.agent || header.model
                ? div()
                    .px(5)
                    .rounded(4)
                    .bg(t.raised)
                    .text_size(10)
                    .text_color(t.muted)
                    .child(`${header.agent || "–"}/${header.model || "–"}`)
                : null,
              div().flex_1(),
              action.enabled
                ? div()
                    .id("btn-switch-pty")
                    .items_center()
                    .gap(5)
                    .px(8)
                    .p(4)
                    .rounded(6)
                    .bg(t.raised)
                    .hover((s) => s.bg(t.hover))
                    .child(
                      h_flex()
                        .items_center()
                        .gap(5)
                        .children([
                          svg("mods/workspace-chrome/terminal.svg")
                            .size(11)
                            .flex_none()
                            .text_color(t.accent),
                          div().text_size(11).text_color(t.foreground).child(action.label),
                        ]),
                    )
                    .on_click(() => this.focusPty(cx))
                : div()
                    .id("btn-switch-pty-disabled")
                    .items_center()
                    .gap(5)
                    .px(8)
                    .p(4)
                    .rounded(6)
                    .bg(t.canvas)
                    .child(
                      h_flex()
                        .items_center()
                        .gap(5)
                        .children([
                          svg("mods/workspace-chrome/terminal.svg")
                            .size(11)
                            .flex_none()
                            .text_color(t.dim),
                          div()
                            .text_size(11)
                            .text_color(t.dim)
                            .child(`${action.label} — ${action.hint}`),
                        ]),
                    ),
            ].filter(Boolean),
          ),
        this.renderContextBar(t, header.context),
      ]);
  }

  // ---------------------------------------------------------------------------
  // The collapsible timeline
  // ---------------------------------------------------------------------------

  renderTranscriptItem(t, item, idx, seatActive, agent) {
    const key = `item-${idx}`;

    if (item.kind === "thinking") {
      const meta = transcriptKindMeta(item.kind);
      const hasOverride = Object.prototype.hasOwnProperty.call(this.collapsed, key);
      const isCollapsed = hasOverride
        ? this.collapsed[key]
        : !defaultItemExpanded(item, seatActive);
      return v_flex()
        .id(key)
        .p(2)
        .px(6)
        .rounded(6)
        .bg(t.surface)
        .border(1)
        .border_color(t.border)
        .children([
          h_flex()
            .id(`${key}-toggle`)
            .items_center()
            .gap(6)
            .h(22)
            .rounded(4)
            .hover((s) => s.bg(t.hover))
            .on_click((_e, clickCx) => {
              this.collapsed[key] = !isCollapsed;
              clickCx.notify();
            })
            .children(
              [
                this.chevronIcon(t, !isCollapsed),
                this.renderIconDescriptor(t, meta.icon, agent, 12, t.accent),
                div().text_size(11.5).font_medium().text_color(t.dim).child(meta.label),
                item.duration_ms
                  ? div()
                      .text_size(10.5)
                      .text_color(t.dim)
                      .child(`${(item.duration_ms / 1000).toFixed(1)}s`)
                  : null,
                isCollapsed
                  ? div()
                      .flex_1()
                      .text_size(10.5)
                      .text_color(t.dim)
                      .truncate()
                      .child(item.body || "")
                  : div().flex_1(),
              ].filter(Boolean),
            ),
          // The full reasoning body: long thinking is never truncated.
          ...(!isCollapsed
            ? [
                TextView.markdown(`${key}-md`, item.body || "")
                  .text_size(12)
                  .text_color(t.muted),
              ]
            : []),
        ]);
    }

    if (item.kind === "tool_call") {
      const meta = transcriptKindMeta(item.kind);
      const hasOverride = Object.prototype.hasOwnProperty.call(this.collapsed, key);
      const model = toolCallCollapsedModel(
        item,
        seatActive,
        hasOverride ? this.collapsed[key] : null,
      );
      const status = item.status || "done";
      return v_flex()
        .id(key)
        .rounded(6)
        .bg(t.raised)
        .border(1)
        .border_color(t.border)
        .children([
          // The compact one-line row: chevron + tool name + status badge.
          h_flex()
            .id(`${key}-toggle`)
            .items_center()
            .gap(6)
            .h(28)
            .px(4)
            .rounded(4)
            .hover((s) => s.bg(t.hover))
            .on_click((_e, clickCx) => {
              this.collapsed[key] = model.toggle();
              clickCx.notify();
            })
            .children(
              [
                this.chevronIcon(t, !model.collapsed),
                this.renderIconDescriptor(t, meta.icon, agent, 12, t.accent),
                div()
                    .text_size(11.5)
                    .font_medium()
                    .text_color(t.foreground)
                    .truncate()
                    .child(item.title || meta.label),
                div().flex_1(),
                div()
                    .px(5)
                    .rounded(4)
                    .bg(t.canvas)
                    .text_size(10)
                    .font_semibold()
                    .text_color(statusColor(t, status))
                    .child(status),
              ].filter(Boolean),
            ),
          // Expanded: the full input/output payload, never truncated.
          ...(!model.collapsed
            ? [
                TextView.markdown(`${key}-md`, item.body || "")
                  .text_size(11)
                  .text_color(t.muted),
              ]
            : []),
        ]);
    }

    if (isNoticeKind(item.kind)) {
      const meta = transcriptKindMeta(item.kind);
      return v_flex()
        .id(key)
        .p(8)
        .gap(4)
        .rounded(8)
        .bg(t.surface)
        .border(1)
        .border_color(t.accent)
        .children([
          div()
            .text_size(11)
            .font_semibold()
            .text_color(t.accent)
            .child(item.title || meta.label),
          TextView.markdown(`${key}-md`, item.body || "").text_size(12).text_color(t.foreground),
        ]);
    }

    const meta = transcriptKindMeta("message");
    return v_flex()
      .id(key)
      .p(8)
      .gap(4)
      .rounded(8)
      .bg(t.surface)
      .border(1)
      .border_color(t.border)
      .children([
        h_flex()
          .items_center()
          .gap(6)
          .children(
            [
              // The vendor icon marks the seat's own messages; Human
              // directives keep their accent label as the marker.
              item.role === "user"
                ? null
                : this.renderIconDescriptor(t, meta.icon, agent, 12),
              div()
                .text_size(11)
                .font_semibold()
                .text_color(item.role === "user" ? t.accent : t.foreground)
                .child(item.title || (item.role === "user" ? "Directive / Prompt" : "Assistant")),
            ].filter(Boolean),
          ),
        TextView.markdown(`${key}-md`, item.body || "").text_size(13).text_color(t.foreground),
      ]);
  }

  render(cx) {
    const t = theme();
    // No default seat: with nothing selected the header stays real — a
    // "no seat selected" hint instead of a fabricated Lead/model/context
    // (spec Slice 3: replace mocked data with real runtime state).
    const selected = shared(HOTPOT_SELECTED_KEY) ?? null;
    const seatActive = isSeatActive(selected);
    const header = inspectorHeaderModel(selected, this.agentRegistry, this.inspection);
    const target = ptyTarget(selected);

    return v_flex()
      .id("root")
      .size_full()
      .bg(t.background)
      .p(12)
      .gap(10)
      .children([
        this.renderHeader(t, cx, header),

        v_flex()
          .id("transcript-scroll")
          .flex_1()
          .min_h(0)
          .gap(6)
          .overflow_y_scrollbar()
          .children(
            this.items.length > 0
              ? this.items.map((item, idx) =>
                  this.renderTranscriptItem(t, item, idx, seatActive, header.agent),
                )
              : [
                  // A quiet empty state: no bordered placeholder box, no emoji.
                  v_flex()
                    .id("transcript-empty")
                    .gap(4)
                    .p(8)
                    .children([
                      div()
                        .text_size(12)
                        .font_medium()
                        .text_color(t.dim)
                        .child("No transcript yet"),
                      div()
                        .text_size(11)
                        .text_color(t.dim)
                        .child(
                          "Select Supervisor, Lead, or any active Peer on the Hotpot panel to stream its Thinking, Tool Calls, and notices in real time.",
                        ),
                    ]),
                ],
          ),

        h_flex()
          .items_center()
          .gap(8)
          .p(8)
          .rounded(8)
          .bg(t.surface)
          .border(1)
          .border_color(t.input)
          .children([
            div().flex_1().child(Input.new(this.promptInput)),
            target
              ? div()
                  .id("btn-send-prompt")
                  .px(10)
                  .p(6)
                  .rounded(6)
                  .bg(t.solid)
                  .text_size(12)
                  .font_semibold()
                  .text_color(t.solid_foreground)
                  .hover((s) => s.bg(t.hover))
                  .child("Send")
                  .on_click(() => this.sendPrompt(cx))
              : div()
                  .id("btn-send-prompt-disabled")
                  .px(10)
                  .p(6)
                  .rounded(6)
                  .bg(t.canvas)
                  .text_size(11)
                  .text_color(t.dim)
                  .child("Send — no live hc terminal"),
          ]),
      ]);
  }
}
