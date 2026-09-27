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
(`ops/`), the frame layout and the five sources. Use `SPC k l` to pick
one:

| Source | Address | What arrives |
|---|---|---|
| sim packets | `tcp://localhost:10010` | Space packets, back to back |
| sim CADUs | `tcp://localhost:10011` | ASM + randomized TM frames of 1115 octets, RS(255,223) at depth 5, a CLCW |
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

Faults on purpose, so there is something to find:

- A sequence gap of 2 every 90 s.
- A bad CRC every 60 s.
- Frames:
  - A few symbol errors in most frames, which Reed-Solomon corrects.
  - A code word with 20 errors every 75 s, beyond repair.
  - A frame dropped every 100 s, so the VC count jumps.
  - The CLCW shows lockout for a moment every 150 s.

The MIB also has two problems of its own on purpose: a telecommand
parameter with no CPC row, and `TM(17,2)` missing. Press `!` on the MIB
page to see them.

## Without Docker

`sim.py` needs only Python 3:

```bash
python dev/ccsds-sim/sim.py --selftest
python dev/ccsds-sim/sim.py --capture cadus.bin --packets packets.bin --seconds 300
```

`--capture` writes the CADU stream (and the packets it carried) to files
instead of serving anything. Open `cadus.bin` with `SPC k f`: Fenix
works out the framing (sync marker, randomization, RS depth) by itself.
Running the script without arguments serves the TCP ports. The
`SIM_UDP_TARGET`, `SIM_NATS` (host:port), `SIM_NATS_SUBJECT`, `SIM_FILE`
and `SIM_SEED` environment variables turn on the other outputs.

## Check Fenix against it

With the containers up:

```bash
cargo test -p fenix-gui simulator -- --ignored --nocapture
```

This follows each of the five sources for 8 s through Fenix's own live
code and prints what arrived. It fails if a source delivers fewer than
10 housekeeping packets that the MIB identifies.

## Stop it

```bash
docker compose -f dev/ccsds-sim/docker-compose.yml down
```
