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

## Patch 2: client-requested resize (`ExtendedDesktopSizePseudo`)

### Why

Upstream only implements the *old* `DesktopSizePseudo` (-223) extension:
a server can unilaterally resize and tell the client, but the client has
no way to ask for a size of its own. `fenix-vnc`'s "remote resizing"
(matching what TigerVNC/RealVNC call the same feature) needs the other
direction too -- the client requesting a resize (RFB's `SetDesktopSize`
message) so the guest's desktop matches the pane it's shown in instead
of always being scaled to fit.

That extension is `ExtendedDesktopSizePseudo` (-308). A server that
supports it sends one spontaneous `ExtendedDesktopSize` rectangle
(reason "server-initiated") as part of the very first
`FramebufferUpdate`, once it sees -308 in the client's `SetEncodings` --
RFB has no explicit capability-negotiation ack, so that spontaneous
rectangle is the *only* signal a client gets that `SetDesktopSize` is
safe to send at all. Sending it to a server that never sent that
rectangle risks desyncing the connection: RFB's client-message framing
has no length prefix a server could use to skip a message type it
doesn't recognize.

### The change

1. `src/config.rs` -- new `VncEncoding::ExtendedDesktopSizePseudo = -308`
   variant plus both `From` conversions.

2. `src/event.rs` -- new `VncEvent::ExtendedDesktopSize { reason,
   result, width, height }` (the server's reply/announcement) and new
   `X11Event::SetDesktopSize { width, height }` (the client's request).
   Both enums are already `#[non_exhaustive]`.

3. `src/client/messages.rs` -- new `ClientMsg::SetDesktopSize { width,
   height }` and its wire encoder (message type 251, always describing
   exactly one screen at (0, 0) covering the whole framebuffer -- this
   crate has no multi-monitor guest layout to preserve).

4. `src/client/connection.rs`:
   - `asycn_vnc_read_loop` gets a new `VncEncoding::
     ExtendedDesktopSizePseudo` rectangle arm: reads past the
     screen-layout body (a count plus that many 16-byte `Screen`
     structures) to stay in sync with the stream, then emits
     `VncEvent::ExtendedDesktopSize` with `reason`/`result` taken from
     the rectangle's `x`/`y` fields (not a position, for this
     encoding -- see the event's own doc comment) and the new
     width/height from the rectangle's own `width`/`height`.
   - `VncInner::input` gains a `X11Event::SetDesktopSize` arm mapping
     straight to `ClientMsg::SetDesktopSize`.
   - Incidental fix bundled with this patch (found while wiring it up,
     not a pre-existing separate issue worth its own patch entry):
     `VncInner::screen` was a plain `(u16, u16)` set once at connect and
     never updated, so `input()`'s `Refresh`/`FullRefresh` kept sizing
     their `FramebufferUpdateRequest` from the *original* resolution
     forever after any later resize (server-initiated `DesktopSizePseudo`
     already had this bug; a successful client-requested resize would
     have hit it too). Now `Arc<std::sync::Mutex<(u16, u16)>>`, updated
     by the decoding task on every `SetResolution`/`ExtendedDesktopSize`
     event, read through the lock in `input()`.

`fenix_vnc::VncClient` is where the "have we actually seen that
spontaneous rectangle yet" gate lives (not in this vendored copy) --
see its `resize_supported`/`request_resize`.

### Upstreaming

Also worth sending upstream, though larger than patch 1 -- it's a real
protocol extension, not just a missing event boundary.
