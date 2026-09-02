# Local patch: vnc-rs 0.5.3

Vendored from crates.io and wired in via `[patch.crates-io]` in the
workspace root `Cargo.toml`. Upstream: https://github.com/HsuJv/vnc-rs

Keep this file in sync with any further local change. On a version bump,
re-apply these deliberately rather than rediscovering them.

## Why

RFB's atomic unit is the `FramebufferUpdate` **message**, not the
rectangle. The message header carries a `number-of-rectangles` count, and
`CopyRect` is defined against the framebuffer as it was at the *start* of
the update. A client that paints partway through one therefore shows
genuinely inconsistent frames -- window content visible in two places at
once, regions that should have been erased still showing old pixels.

Upstream reads that rectangle count, uses it as a loop bound, and then
discards it: `VncEvent` has no "this update is complete" variant, so a
consumer sees only a flat stream of per-rectangle events. That makes it
impossible to (a) present whole frames, or (b) implement the usual
one-outstanding-request flow control every other RFB client uses (send a
`FramebufferUpdateRequest`, wait for its response to fully arrive, then
send the next).

`fenix-vnc` needs both, so this vendored copy surfaces the boundary.

## The change

Two edits, both additive:

1. `src/event.rs` -- new `VncEvent::FramebufferUpdateEnd` variant.
   `VncEvent` is `#[non_exhaustive]`, so adding a variant is not a
   breaking change for other consumers.

2. `src/client/connection.rs` -- in `asycn_vnc_read_loop`, emit that
   event once the `ServerMsg::FramebufferUpdate(rect_num)` arm finishes
   its rectangle loop. Placed after the loop so it covers both ways an
   update terminates: the count being exhausted, and a
   `LastRectPseudo` rectangle ending it early via `break`.

Nothing else is modified. `Cargo.toml` is trimmed (upstream
`[dev-dependencies]`, which pull `minifb` for examples this workspace
never builds, and upstream's `[profile.release]`, which is ignored
outside a workspace root).

## Upstreaming

This is worth sending upstream as-is -- it is small, additive, and the
missing boundary is a genuine gap in the public API for any consumer that
wants to render coherent frames.
