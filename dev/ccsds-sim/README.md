# A simulated spacecraft, for Fenix's CCSDS tools

Fenix's live sources, recordings and packet inspector (`SPC k`) were
built against the standards and checked with unit tests. This directory
gives them something real to listen to: a container that flies the demo
mission in `mission/` and sends its telemetry every way Fenix can
receive it.

Development scaffolding: it never ships, its ports are bound to
`localhost`, and it only talks. There is no telecommand path to it;
Fenix never sends any.

## Run it

```bash
docker compose -f dev/ccsds-sim/docker-compose.yml up -d --build
docker logs -f fenix-ccsds-sim
```

Then open `dev/ccsds-sim/mission/thermal_checkout.tcl` in Fenix. The
project's settings (`mission/.fenix/settings.toml`) already name the MIB
(`ops/`), the frame layout and the eight sources. Use `SPC k l` to pick
one:

| Source | Address | What arrives |
|---|---|---|
| sim packets | `tcp://localhost:10010` | Space packets, back to back |
| sim CADUs | `tcp://localhost:10011` | ASM + randomized TM frames of 1115 octets, RS(255,223) at depth 5, a CLCW |
| sim CADUs + FECF | `tcp://localhost:10013` | The same, with an FECF at the end of each frame (framing `frames fecf`) |
| sim CLTUs (uplink) | `tcp://localhost:10012` | The uplink: CLTUs carrying TC frames, which carry the MIB's telecommands |
| sim CLTUs (randomized) | `tcp://localhost:10014` | The same uplink with the TC frames randomized (framing `guess`: Fenix recognizes it) |
| sim UDP | `udp://:10015` | One packet per datagram (Fenix listens; the container sends to the host) |
| sim NATS | `nats://localhost:4222`, `tm.ops` | One packet per message, through the `nats` container |
| sim file | `file://recordings/live.bin` | Packets appended to a file, which is cut back to empty past 4 MB |

## What it flies

Twice a second:

- TCS fast housekeeping, `TM(3,25)` SID 1 (SPID 30211). Heater 3
  swings from 10 to 55 degC over five minutes, so it crosses the soft
  high limit of 50. Heater 4 trails it by 10 degC, the heater mode
  steps every two minutes, and the thermistor bank drops out briefly.
- Every 5 s, diagnostic housekeeping, SID 2 (SPID 30219).
- `TM(5,1)` events and `TM(17,2)` connection reports from APID 1008.
  The MIB doesn't define them, so they show as unidentified.
- Idle packets (APID 0x7FF).

Every 3 s, a telecommand goes up, as a ground station would put it on
the wire. It's a PUS-C packet from the MIB (ZTC08101, ZTC08102,
ZTC17001, ZTC03005, ZTC08001, ZTC11004) in a Type-A TC frame on VC 0,
with a segment header and an FECF. The frame is sent in a CLTU (start
sequence, BCH code blocks, tail sequence) after an acquisition sequence.
The spacecraft answers accepted telecommands with TM(1,1) and TM(1,7)
verification reports, or TM(1,2) for one in seven. Its CLCW reports the
next frame it expects, N(R), and sets the retransmit flag after it turns
a frame away. The ground then goes back and sends the frame again, as
FOP-1 does.

Faults on purpose, so there is something to find:

- A sequence gap of 2 every 90 s.
- A bad CRC every 60 s.
- Frames:
  - A few symbol errors in most frames, which Reed-Solomon corrects.
  - A code word with 20 errors every 75 s, beyond repair.
  - A frame dropped every 100 s, so the VC count jumps.
  - With an FECF, one that doesn't match every 90 s.
- The uplink:
  - One bit wrong in a code block: put right by the decoder, which
    works in error-correcting mode (`ccsds.tc_bch = "correct"`, the
    default), so the frame gets through.
  - Two bits wrong in a code block: beyond correction, so the CLTU is
    cut short there and the frame turned away.
  - An FECF that doesn't match.
  - A frame lost on the way up, then a retransmission.
  - A CLTU with no tail sequence. The idle after it ends it, and the
    frame still gets through.
  - Type-BD frames, and a BC Unlock control command.

The MIB also has two problems of its own on purpose: a telecommand
parameter with no CPC row, and `TM(17,2)` missing. Press `!` on the MIB
page to see them.

## Seeing the layers

Press `f` on any packet of a stream page to open the CADU or CLTU it
came in, taken apart in the packet inspector. A CADU shows the sync
marker, de-randomization, each Reed-Solomon code word (clean, corrected
or beyond repair), the frame header, the CLCW and the FECF, then the
packets. A CLTU shows the start sequence, every code block with its BCH
parity, the tail sequence, then the TC frame (header, segment header,
FECF, fill) and the telecommand with its MIB arguments. The TC frame's
fields are outlined on the octets they occupy inside the code blocks.
Problems (a failed FECF, a CLTU cut short, a missing tail, frames lost)
are listed in the page's Problems tab (`4`).

## Without Docker

`sim.py` needs only Python 3:

```bash
python dev/ccsds-sim/sim.py --selftest
python dev/ccsds-sim/sim.py --capture cadus.bin --fecf cadus-fecf.bin --cltus cltus.bin --cltus-randomized cltus-r.bin --packets packets.bin --seconds 300
```

`--capture` writes the streams to files instead of serving anything,
and prints what happened on the uplink, CLTU by CLTU. Open them with
`SPC k f`, and Fenix works out the framing by itself: sync marker,
randomization and RS depth for CADUs, and CLTUs by their start and tail
sequences. The one thing it can't guess is an FECF on TM frames:
`cadus-fecf.bin` needs `ccsds.frame_fecf` on.

Running the script without arguments serves the five TCP ports. The
`SIM_UDP_TARGET`, `SIM_NATS` (host:port), `SIM_NATS_SUBJECT`, `SIM_FILE`
and `SIM_SEED` environment variables turn on the other outputs.

## Check Fenix against it

With the containers up:

```bash
cargo test -p fenix-gui simulator -- --ignored --nocapture
```

This follows each of the eight sources for 8 s through Fenix's own
live code and prints what arrived. It fails if a downlink source
delivers fewer than 10 housekeeping packets that the MIB identifies, or
if the uplink gives fewer than 2 telecommands it names.

## Stop it

```bash
docker compose -f dev/ccsds-sim/docker-compose.yml down
```
