import { View, div, TextView } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { exec, hc, spaces, theme, run, shared, home } from "herdr";
import {
  HOTPOT_SELECTED_KEY,
  formatContextBadge,
  statusColor,
  parseHotpotJson,
} from "./protocol.js";

export default class HotpotSessionInspector extends View {
  init(_props, cx) {
    this.items = [];
    this.collapsed = {};
    this.lastAgentId = null;
    this.loading = false;
    this.promptInput = InputState.new(cx, { placeholder: "Send directive or message to this session..." });
    cx.timer.every(1500, (timerCx) => this.refresh(timerCx));
    this.refresh(cx);
  }

  activeSpace() {
    const all = spaces();
    return all.find((s) => s.active) ?? all[0] ?? null;
  }

  refresh(cx) {
    const selected = shared(HOTPOT_SELECTED_KEY);
    if (!selected || this.loading) return;
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

  sendPrompt(cx) {
    const selected = shared(HOTPOT_SELECTED_KEY);
    const text = (this.promptInput?.value?.() ?? "").trim();
    if (!selected || !text) return;
    this.promptInput?.set_value?.("", cx);

    cx.spawn(async (taskCx) => {
      const sp = this.activeSpace();
      const folder = sp?.folder || home();
      if (selected.terminal) {
        await hc(["send", selected.terminal, text]);
      } else {
        await exec(
          "hotpot",
          ["send", "--workspace", folder, "--role", selected.role || "lead", "--message", text],
          { cwd: folder, on: sp?.machine ?? null },
        );
      }
      this.refresh(taskCx);
    });
  }

  renderTranscriptItem(t, item, idx) {
    const key = `item-${idx}`;
    const isCollapsed = Boolean(this.collapsed[key]);

    if (item.kind === "thinking") {
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
                  div().text_size(12).font_semibold().text_color(t.accent).child("💭 Thinking"),
                  div()
                    .text_size(11)
                    .text_color(t.dim)
                    .child(item.duration_ms ? `${(item.duration_ms / 1000).toFixed(1)}s` : "reasoning"),
                ]),
              div().text_size(11).text_color(t.muted).child(isCollapsed ? "▶ Expand" : "▼ Collapse"),
            ]),
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
                .child(`🔧 ${item.title || "Tool Call"}`),
              div()
                .px(6)
                .rounded(4)
                .bg(t.canvas)
                .text_size(10)
                .text_color(statusColor(t, item.status || "done"))
                .child((item.status || "OK").toUpperCase()),
            ]),
          div().text_size(11).text_color(t.muted).child(item.body || ""),
        ]);
    }

    if (item.kind === "mailbox") {
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
            .child(`📩 ${item.title || "Mailbox"}`),
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
            h_flex()
              .items_center()
              .gap(6)
              .children([
                div()
                  .id("btn-switch-pty")
                  .px(8)
                  .p(4)
                  .rounded(6)
                  .bg(t.raised)
                  .text_size(11)
                  .text_color(t.foreground)
                  .hover((s) => s.bg(t.hover))
                  .child("⌨ Open Raw Terminal (PTY)")
                  .on_click(() => {
                    run("layout.select", { stack: "main-stack", child: "main" });
                    if (selected.terminal) {
                      run("pane.focus", { terminal: selected.terminal });
                    }
                  }),
              ]),
          ]),

        v_flex()
          .id("transcript-scroll")
          .flex_1()
          .gap(8)
          .overflow_y_scrollbar()
          .children(
            this.items.length > 0
              ? this.items.map((item, idx) => this.renderTranscriptItem(t, item, idx))
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
                          "Select Supervisor, Lead, or any active Peer on the right-hand Hotpot panel to stream its full Thinking blocks, Tool Calls, and Mailbox history in real time.",
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
            div()
              .id("btn-send-prompt")
              .px(10)
              .p(6)
              .rounded(6)
              .bg(t.solid)
              .text_size(12)
              .font_semibold()
              .text_color(t.solid_foreground)
              .child("Send ↵")
              .on_click(() => this.sendPrompt(cx)),
          ]),
      ]);
  }
}
