import { View, div, TextView } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { exec, hc, spaces, theme, run, shared, home, nextCommand, notify } from "herdr";
import {
  HOTPOT_SELECTED_KEY,
  formatContextBadge,
  statusColor,
  parseHotpotJson,
  transcriptKindMeta,
  isNoticeKind,
  isSeatActive,
  defaultItemExpanded,
  ptyTarget,
} from "./protocol.js";

export default class HotpotSessionInspector extends View {
  init(_props, cx) {
    this.items = [];
    // Per-item collapse overrides for the selected seat: an absent key follows
    // the default expansion (`defaultItemExpanded`); true/false pins the block.
    this.collapsed = {};
    this.lastAgentId = null;
    this.loading = false;
    this.promptInput = InputState.new(cx, { placeholder: "Send directive or message to this session..." });
    cx.timer.every(1500, (timerCx) => this.refresh(timerCx));
    // Key handling for the `Alt+T` Raw PTY jump: `keys.json` binds
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

  activeSpace() {
    const all = spaces();
    return all.find((s) => s.active) ?? all[0] ?? null;
  }

  refresh(cx) {
    const selected = shared(HOTPOT_SELECTED_KEY);
    if (!selected || this.loading) return;
    // A new seat selection starts from a clean transcript and default expansion.
    if (selected.id !== this.lastAgentId) {
      this.lastAgentId = selected.id;
      this.collapsed = {};
      this.items = [];
    }
    const sp = this.activeSpace();
    const folder = sp?.folder || home();
    this.loading = true;

    cx.spawn(async (taskCx) => {
      try {
        const args = [
          "inspect",
          "--workspace",
          folder,
          "--agent",
          selected.id || "seat-lead",
          "--role",
          selected.role || "lead",
        ];
        if (selected.terminal) {
          args.push("--terminal", selected.terminal);
        }
        const res = await exec("hotpot", args, {
          cwd: folder,
          on: sp?.machine ?? null,
          timeout: 5000,
        });
        if (res.code === 0) {
          const parsed = parseHotpotJson(res.stdout, null);
          if (parsed && Array.isArray(parsed.items)) {
            this.items = parsed.items;
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

  // One-click `Raw PTY` / `Alt+T`: focus the selected seat's live `hc`
  // terminal in the main stack. Without a terminal the jump is impossible;
  // say so instead of silently switching to an empty pane.
  focusPty(cx) {
    const target = ptyTarget(shared(HOTPOT_SELECTED_KEY));
    if (!target) {
      notify({
        title: "Raw PTY unavailable",
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

  renderTranscriptItem(t, item, idx, seatActive) {
    const key = `item-${idx}`;

    if (item.kind === "thinking") {
      const meta = transcriptKindMeta(item.kind);
      const hasOverride = Object.prototype.hasOwnProperty.call(this.collapsed, key);
      const isCollapsed = hasOverride ? this.collapsed[key] : !defaultItemExpanded(item, seatActive);
      return v_flex()
        .id(key)
        .p(10)
        .gap(6)
        .rounded(8)
        .bg(t.surface)
        .border(1)
        .border_color(t.border)
        .children([
          h_flex()
            .id(`${key}-toggle`)
            .items_center()
            .justify_between()
            .on_click((_e, clickCx) => {
              this.collapsed[key] = !isCollapsed;
              clickCx.notify();
            })
            .children([
              h_flex()
                .items_center()
                .gap(6)
                .children([
                  div().text_size(12).font_semibold().text_color(t.accent).child(`${meta.icon} ${meta.label}`),
                  div()
                    .text_size(11)
                    .text_color(t.dim)
                    .child(item.duration_ms ? `${(item.duration_ms / 1000).toFixed(1)}s` : "reasoning"),
                ]),
              div().text_size(11).text_color(t.muted).child(isCollapsed ? "▶ Expand" : "▼ Collapse"),
            ]),
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
      return v_flex()
        .id(key)
        .p(10)
        .gap(4)
        .rounded(8)
        .bg(t.raised)
        .border(1)
        .border_color(t.border)
        .children([
          h_flex()
            .items_center()
            .justify_between()
            .children([
              div()
                .text_size(12)
                .font_semibold()
                .text_color(t.foreground)
                .child(`${meta.icon} ${item.title || meta.label}`),
              div()
                .px(6)
                .rounded(4)
                .bg(t.canvas)
                .text_size(10)
                .text_color(statusColor(t, item.status || "done"))
                .child((item.status || "done").toUpperCase()),
            ]),
          div().text_size(11).text_color(t.muted).child(item.body || ""),
        ]);
    }

    if (isNoticeKind(item.kind)) {
      const meta = transcriptKindMeta(item.kind);
      return v_flex()
        .id(key)
        .p(10)
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
            .child(`${meta.icon} ${item.title || meta.label}`),
          TextView.markdown(`${key}-md`, item.body || "").text_size(12).text_color(t.foreground),
        ]);
    }

    return v_flex()
      .id(key)
      .p(10)
      .gap(4)
      .rounded(8)
      .bg(t.surface)
      .border(1)
      .border_color(t.border)
      .children([
        div()
          .text_size(11)
          .font_semibold()
          .text_color(item.role === "user" ? t.accent : t.foreground)
          .child(item.title || (item.role === "user" ? "Directive / Prompt" : "Assistant")),
        TextView.markdown(`${key}-md`, item.body || "").text_size(13).text_color(t.foreground),
      ]);
  }

  render(cx) {
    const t = theme();
    const selected = shared(HOTPOT_SELECTED_KEY) ?? {
      id: "seat-lead",
      role: "lead",
      title: "Lead",
      provider: "codex",
      model: "gpt-5.4",
      status: "idle",
      context_tokens: 42100,
    };
    const seatActive = isSeatActive(selected);
    const target = ptyTarget(selected);

    return v_flex()
      .id("root")
      .size_full()
      .bg(t.background)
      .p(12)
      .gap(10)
      .children([
        h_flex()
          .items_center()
          .justify_between()
          .p(8)
          .rounded(8)
          .bg(t.chrome)
          .border(1)
          .border_color(t.border)
          .children([
            h_flex()
              .items_center()
              .gap(8)
              .children([
                div()
                  .text_size(14)
                  .font_semibold()
                  .text_color(t.foreground)
                  .child(`👁 ${selected.title || "Session Inspector"}`),
                div()
                  .px(6)
                  .rounded(4)
                  .bg(t.raised)
                  .text_size(11)
                  .text_color(t.muted)
                  .child(`${selected.provider || "claude"}/${selected.model || "opus"}`),
                div()
                  .px(6)
                  .rounded(4)
                  .bg(t.surface)
                  .text_size(11)
                  .text_color(t.accent)
                  .child(formatContextBadge(selected.context_tokens ?? 0, selected.role)),
              ]),
            target
              ? div()
                  .id("btn-switch-pty")
                  .px(8)
                  .p(4)
                  .rounded(6)
                  .bg(t.raised)
                  .text_size(11)
                  .text_color(t.foreground)
                  .hover((s) => s.bg(t.hover))
                  .child("⌨ Raw PTY · Alt+T")
                  .on_click(() => this.focusPty(cx))
              : div()
                  .id("btn-switch-pty-disabled")
                  .px(8)
                  .p(4)
                  .rounded(6)
                  .bg(t.canvas)
                  .text_size(11)
                  .text_color(t.dim)
                  .child("⌨ Raw PTY · Alt+T — no live hc terminal"),
          ]),

        v_flex()
          .id("transcript-scroll")
          .flex_1()
          .gap(8)
          .overflow_y_scrollbar()
          .children(
            this.items.length > 0
              ? this.items.map((item, idx) => this.renderTranscriptItem(t, item, idx, seatActive))
              : [
                  v_flex()
                    .p(12)
                    .gap(6)
                    .rounded(8)
                    .bg(t.surface)
                    .border(1)
                    .border_color(t.border)
                    .children([
                      div()
                        .text_size(12)
                        .font_semibold()
                        .text_color(t.accent)
                        .child("💭 Thinking & Tool Call Stream Ready"),
                      div()
                        .text_size(12)
                        .text_color(t.muted)
                        .child(
                          "Select Supervisor, Lead, or any active Peer on the right-hand Hotpot panel to stream its full Thinking blocks, Tool Calls, and Attention / Assignment Notices in real time.",
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
                  .child("Send ↵")
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
