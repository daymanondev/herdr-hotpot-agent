// Shared native chrome render helpers for herdr-hotpot-agent mods.
// Canonical source lives in `shared/chrome.js`; synced into `mods/*/` by `scripts/sync.sh`.
//
// These are the gpui render helpers both mods draw with — the geometric context
// bar, the vendor/chrome icon descriptor rule, and the warning color — kept as
// one source of truth (AGENTS.md: a helper shared across mods lives in
// `shared/`) so a fix to one context bar reaches every mod.

import { div, svg, image } from "gpui-kit";
import { h_flex } from "gpui-base";
import { ROTATION_MARK_TOKENS, agentIconModel, formatTokenCount } from "./protocol.js";

/** The theme's warning tint: the blocked status color, falling back to ANSI red. */
export function warningColor(t) {
  return t.blocked ?? t.ansi1;
}

/** An icon descriptor rendered natively: chrome glyphs as one-color `svg`s in
 * the given color, vendor descriptors through the 06 icon rule against Herdr's
 * program table (`svg(icon)` tinted when `tint`, else the full-color
 * `image(icon)`; the terminal glyph as fallback). `null` descriptors render
 * nothing. */
export function renderIconDescriptor(t, descriptor, agent, registry, size = 12, color = null) {
  if (!descriptor) return null;
  if (descriptor.kind === "vendor") {
    const icon = agentIconModel(agent, registry);
    if (icon.tint) {
      return svg(icon.icon).size(size).flex_none().text_color(t.foreground);
    }
    return image(icon.icon).w(size).h(size).flex_none();
  }
  return svg(descriptor.icon).size(size).flex_none().text_color(color ?? t.dim);
}

/** The seat's context window as one geometric bar (spec "Geometric Context
 * Bar"): a `3px` rounded track carrying proportional `Cache Read` / `Fresh
 * Input` / `Output` fills, a `1px` warning-colored tick at the rotation mark's
 * share of the track, and the compact `150k` badge once usage reaches the
 * mark. The quiet token total stays on the right. `bar` is the segmented
 * model from `contextBarModel`; `rootId` distinguishes the calling surface. */
export function renderContextBar(t, bar, rootId) {
  const segmentColors = { cacheRead: t.muted, freshInput: t.accent, output: t.dim };
  return h_flex()
    .id(rootId)
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
                    .bg(warningColor(t))
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
              .text_color(warningColor(t))
              .child(formatTokenCount(ROTATION_MARK_TOKENS))
          : null,
        div().text_size(10).text_color(t.muted).child(formatTokenCount(bar.total)),
      ].filter(Boolean),
    );
}
