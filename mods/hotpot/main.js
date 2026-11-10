import { View, div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { exec, spaces, agents, theme, run, shared, setShared, home } from "herdr";
import {
  HOTPOT_SELECTED_KEY,
  HOTPOT_VIEW_MODE_KEY,
  formatContextBadge,
  statusColor,
  parseHotpotJson,
} from "./protocol.js";

export default class HotpotPanel extends View {
  init(_props, cx) {
    this.snapshot = null;
    this.lastFolder = null;
    this.loading = false;
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
        // Keep last good snapshot or fallback to live Herdr agents
      } finally {
        this.loading = false;
        taskCx.notify();
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

  renderSeatCard(t, seat, selectedId) {
    const isSelected = selectedId === seat.id;
    const dotColor = statusColor(t, seat.status);
    return v_flex()
      .id(`seat-${seat.role}`)
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
              ]),
            div()
              .px(6)
              .rounded(4)
              .bg(t.raised)
              .text_size(11)
              .text_color(t.muted)
              .child(formatContextBadge(seat.context_tokens ?? 0, seat.role)),
          ]),
        h_flex()
          .items_center()
          .justify_between()
          .children([
            div()
              .text_size(11)
              .text_color(t.dim)
              .truncate()
              .child(`${seat.provider}/${seat.model}`),
            div()
              .id(`seat-pty-${seat.role}`)
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
        div()
          .text_size(11)
          .text_color(t.muted)
          .truncate()
          .child(seat.activity || "Idle — click to inspect Thinking & session"),
      ]);
  }

  renderTaskCard(t, task, selectedId) {
    const isSelected = selectedId === task.agent_id;
    const verdictColor = statusColor(t, task.verdict || task.status);
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
            id: task.agent_id || task.id,
            role: "agent",
            task: task.id,
            title: `Peer · ${task.id}`,
            provider: task.provider || "claude",
            model: task.model || "sonnet",
            terminal: task.terminal || null,
            status: task.status || "idle",
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
              .child(`${(task.verdict || task.status || "IDLE").toUpperCase()} · ${task.attempt || 1}/3`),
          ]),
        div().text_size(11).text_color(t.muted).truncate().child(task.summary || "Bounded SLP Task"),
        div()
          .text_size(11)
          .text_color(t.dim)
          .truncate()
          .child(task.activity || "Click to inspect Peer Thinking & Tool Calls"),
      ]);
  }

  render(_cx) {
    const t = theme();
    const sp = this.activeSpace();
    const selected = shared(HOTPOT_SELECTED_KEY);
    const selectedId = selected?.id ?? null;

    const liveAgents = agents().filter((a) => !sp || a.space === sp.id);
    const seats = this.snapshot?.seats ?? [
      {
        id: "seat-supervisor",
        role: "supervisor",
        title: "Supervisor",
        provider: "claude",
        model: "claude-opus-4-6",
        status: liveAgents[0]?.status ?? "idle",
        terminal: liveAgents[0]?.address ?? null,
        context_tokens: 18400,
        activity: "Ready for Owner directive",
      },
      {
        id: "seat-lead",
        role: "lead",
        title: "Lead",
        provider: "codex",
        model: "gpt-5.4",
        status: liveAgents[1]?.status ?? "idle",
        terminal: liveAgents[1]?.address ?? null,
        context_tokens: 42100,
        activity: "Awaiting wave dispatch or Peer claim",
      },
    ];

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

        v_flex()
          .gap(6)
          .children([
            div().text_size(11).font_semibold().text_color(t.muted).child("SEATS (CLICK TO INSPECT THINKING)"),
            ...seats.map((seat) => this.renderSeatCard(t, seat, selectedId)),
          ]),

        v_flex()
          .gap(6)
          .children([
            h_flex()
              .items_center()
              .justify_between()
              .children([
                div().text_size(11).font_semibold().text_color(t.muted).child("TASKS & PEERS"),
                div().text_size(11).text_color(t.dim).child(`${tasks.length} active`),
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
                    .child("No active Task worktrees in this wave."),
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
