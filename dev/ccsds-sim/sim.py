#!/usr/bin/env python3
"""A spacecraft that only talks: PUS-C telemetry for Fenix's CCSDS tools.

It flies the demo mission in ./mission (the OPS MIB): TCS housekeeping
(SPIDs 30211 and 30219), a few packets the MIB doesn't know, and idle
packets, with faults on purpose -- sequence gaps, a bad CRC now and then,
a heater temperature that crosses its limits. Every packet goes out
every way Fenix can listen:

  tcp  :10010   space packets, back to back, to each client
  tcp  :10011   CADUs: ASM, randomized TM frames of 1115 octets with
                Reed-Solomon (255,223) at depth 5 and a CLCW -- with
                symbol errors injected (corrected by Fenix), now and then
                a code word beyond repair, and frames dropped
  udp           one packet per datagram to SIM_UDP_TARGET
  nats          one packet per message on SIM_NATS_SUBJECT
  file          packets appended to SIM_FILE, cut back to empty when it
                passes SIM_FILE_MAX octets

Standard library only. `python sim.py --capture cadus.bin --seconds 60`
writes the CADU stream to a file instead of serving anything, and
`python sim.py --selftest` checks the codecs against known values.

It never listens for telecommands: there is nothing to command.
"""

import argparse
import math
import os
import queue
import random
import socket
import struct
import sys
import threading
import time
from datetime import datetime, timezone

# ---------------------------------------------------------------- codecs


def crc16(data):
    """CRC-16-CCITT (0x1021, initial 0xFFFF): '123456789' gives 0x29B1."""
    crc = 0xFFFF
    for b in data:
        crc ^= b << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) & 0xFFFF if crc & 0x8000 else (crc << 1) & 0xFFFF
    return crc


TAI_MINUS_UTC = 37
EPOCH_1958 = datetime(1958, 1, 1, tzinfo=timezone.utc).timestamp()


def cuc(unix):
    """CUC 4+2 from 1958 TAI (the mission's time format, no P-field)."""
    tai = unix - EPOCH_1958 + TAI_MINUS_UTC
    coarse = int(tai)
    fine = int((tai - coarse) * 65536) & 0xFFFF
    return struct.pack(">IH", coarse & 0xFFFFFFFF, fine)


def space_packet(apid, seq, data, tm=True, sec=True):
    head = struct.pack(">HHH", (0 if tm else 0x1000) | (0x0800 if sec else 0) | apid, 0xC000 | (seq & 0x3FFF), len(data) - 1)
    return head + data


def pus_tm(apid, seq, service, subtype, msg_count, body, unix, bad_crc=False):
    """A PUS-C TM packet: version 2, time reference status 0, destination 0."""
    sec = bytes([0x20, service, subtype]) + struct.pack(">HH", msg_count & 0xFFFF, 0) + cuc(unix)
    pkt = space_packet(apid, seq, sec + body + b"\0\0")[:-2]
    crc = crc16(pkt) ^ (0x0101 if bad_crc else 0)
    return pkt + struct.pack(">H", crc)


def idle_packet(n):
    """An idle packet of exactly n octets (n >= 7), APID 0x7FF."""
    return space_packet(0x7FF, 0, b"\x55" * (n - 6), sec=False)


def randomizer(n):
    """The CCSDS pseudo-randomizer, h(x) = x^8+x^7+x^5+x^3+1, all ones."""
    bits = [1] * 8
    while len(bits) < n * 8:
        k = len(bits) - 8
        bits.append(bits[k + 7] ^ bits[k + 5] ^ bits[k + 3] ^ bits[k])
    return bytes(int("".join(map(str, bits[i:i + 8])), 2) for i in range(0, n * 8, 8))


class ReedSolomon:
    """RS(255,223) of CCSDS 131.0-B, dual basis on the wire (as Karn's)."""

    NN, NROOTS, FCR, PRIM, GFPOLY = 255, 32, 112, 11, 0x187

    def __init__(self):
        nn = self.NN
        self.alpha_to = [0] * 256
        self.index_of = [0] * 256
        self.index_of[0] = nn
        sr = 1
        for i in range(nn):
            self.index_of[sr] = i
            self.alpha_to[i] = sr
            sr <<= 1
            if sr & 0x100:
                sr ^= self.GFPOLY
            sr &= nn
        g = [0] * (self.NROOTS + 1)
        g[0] = 1
        root = self.FCR * self.PRIM
        for i in range(self.NROOTS):
            g[i + 1] = 1
            for j in range(i, 0, -1):
                g[j] = g[j - 1] ^ self.alpha_to[self.modnn(self.index_of[g[j]] + root)] if g[j] else g[j - 1]
            g[0] = self.alpha_to[self.modnn(self.index_of[g[0]] + root)]
            root += self.PRIM
        self.genpoly = [self.index_of[x] for x in g]
        rows = [0x8D, 0xEF, 0xEC, 0x86, 0xFA, 0x99, 0xAF, 0x7B]
        self.tal = [0] * 256
        self.tal1 = [0] * 256
        for i in range(256):
            v = 0
            for j in range(8):
                for k in range(8):
                    if i & (1 << k):
                        v ^= rows[7 - k] & (1 << j)
            self.tal[i] = v
            self.tal1[v] = i

    def modnn(self, x):
        while x >= self.NN:
            x -= self.NN
            x = (x >> 8) + (x & self.NN)
        return x

    def encode(self, data, pad=0):
        parity = [0] * self.NROOTS
        for d in data[: self.NN - self.NROOTS - pad]:
            feedback = self.index_of[self.tal1[d] ^ parity[0]]
            if feedback != self.NN:
                for j in range(1, self.NROOTS):
                    parity[j] ^= self.alpha_to[self.modnn(feedback + self.genpoly[self.NROOTS - j])]
            parity = parity[1:] + [self.alpha_to[self.modnn(feedback + self.genpoly[0])] if feedback != self.NN else 0]
        return bytes(self.tal[p] for p in parity)

    def encode_block(self, data, depth, pad=0):
        k = self.NN - self.NROOTS - pad
        parity = bytearray(self.NROOTS * depth)
        for w in range(depth):
            word = bytes(data[i * depth + w] for i in range(k))
            for i, p in enumerate(self.encode(word, pad)):
                parity[i * depth + w] = p
        return bytes(parity)


ASM = bytes.fromhex("1ACFFC1D")

# ---------------------------------------------------------------- the spacecraft

# Thermistor curve CAF00310 (raw -> degC), inverted to make raw values.
CURVE = [(0, -40), (600, -20), (1400, 5), (2200, 25), (3000, 45), (3600, 65), (4095, 85)]


def raw_for(deg):
    for (x0, y0), (x1, y1) in zip(CURVE, CURVE[1:]):
        if y0 <= deg <= y1:
            return int(round(x0 + (deg - y0) * (x1 - x0) / (y1 - y0)))
    return 0 if deg < CURVE[0][1] else 4095


class Spacecraft:
    """Makes the packets: call tick(unix) twice a second."""

    APID_TCS, APID_OBC = 1010, 1008

    def __init__(self, rng):
        self.rng = rng
        self.seq = {}
        self.msg = {}
        self.ticks = 0
        self.mode = 2  # AUTO

    def next_seq(self, apid, skip=0):
        s = (self.seq.get(apid, -1) + 1 + skip) & 0x3FFF
        self.seq[apid] = s
        return s

    def next_msg(self, key):
        self.msg[key] = (self.msg.get(key, 0) + 1) & 0xFFFF
        return self.msg[key]

    def tm(self, apid, service, subtype, body, unix, skip=0, bad_crc=False):
        return pus_tm(apid, self.next_seq(apid, skip), service, subtype, self.next_msg((apid, service, subtype)), body, unix, bad_crc)

    def tick(self, unix):
        self.ticks += 1
        t = self.ticks
        out = []
        # Heater 3 swings 10..55 degC over five minutes: it crosses the
        # soft high limit (50) at the top. Heater 4 lags 10 degC behind.
        phase = 2 * math.pi * (t % 600) / 600
        h3 = 32.5 - 22.5 * math.cos(phase) + self.rng.uniform(-0.3, 0.3)
        h4 = h3 - 10 + self.rng.uniform(-0.3, 0.3)
        if t % 240 == 0:
            self.mode = (self.mode + 1) % 3
        powered = 0 if t % 400 < 8 else 1
        # A gap of two every 90 s, a bad CRC every 60 s (on other ticks).
        skip = 2 if t % 180 == 0 else 0
        bad = t % 120 == 30
        body = struct.pack(">HBHHB", 1, powered, raw_for(h3), raw_for(h4), self.mode)
        out.append(self.tm(self.APID_TCS, 3, 25, body, unix, skip, bad))
        if t % 10 == 0:  # diagnostic HK every 5 s
            out.append(self.tm(self.APID_TCS, 3, 25, struct.pack(">HBH", 2, 0, raw_for(h3)) + bytes(8), unix))
        if t % 20 == 5:  # an event the MIB doesn't know: TM(5,1)
            out.append(self.tm(self.APID_OBC, 5, 1, struct.pack(">HI", 0x0101, t), unix))
        if t % 20 == 15:  # a connection test report: TM(17,2), also unknown
            out.append(self.tm(self.APID_OBC, 17, 2, b"", unix))
        if t % 8 == 0:
            out.append(idle_packet(12))
        return out


class Framer:
    """Packets into TM frames on VC 0, idle-filled, as CADUs."""

    LENGTH, DEPTH, SCID = 1115, 5, 0x0A5

    def __init__(self, rng):
        self.rng = rng
        self.rs = ReedSolomon()
        self.rand = randomizer(self.LENGTH + 32 * self.DEPTH)
        self.stream = bytearray()  # packet octets not yet framed
        self.starts = []  # offsets in self.stream where packets start
        self.mc = self.vc = 0
        self.frames = 0
        self.nr = 0
        self.data_len = self.LENGTH - 6 - 4  # header, OCF

    def add(self, packet):
        self.starts.append(len(self.stream))
        self.stream += packet

    def frame(self):
        """The next CADU, or None when this frame is dropped on purpose."""
        # Fill the frame with an idle packet when there isn't enough data.
        need = self.data_len - len(self.stream)
        if need > 0:
            self.add(idle_packet(max(7, need)))
        data = bytes(self.stream[: self.data_len])
        first = next((s for s in self.starts if s < self.data_len), None)
        fhp = 0x7FF if first is None else first
        del self.stream[: self.data_len]
        self.starts = [s - self.data_len for s in self.starts if s >= self.data_len]
        self.frames += 1
        self.nr = (self.nr + (1 if self.frames % 4 == 0 else 0)) & 0xFF
        lockout = 1 if self.frames % 300 < 3 else 0
        clcw = (1 << 24) | (0 << 18) | (lockout << 13) | self.nr  # COP-1, VC 0
        # Version 0, SCID, VC 0, OCF present; then the data field status:
        # no secondary header, packets, segment length id 11, the FHP.
        header = struct.pack(">HBBH", (self.SCID << 4) | 1, self.mc, self.vc, 0x1800 | fhp)
        self.mc = (self.mc + 1) & 0xFF
        self.vc = (self.vc + 1) & 0xFF
        frame = header + data + struct.pack(">I", clcw)
        if self.frames % 200 == 100:
            return None  # a frame lost on the link: the VC count jumps
        block = bytearray(frame + self.rs.encode_block(frame, self.DEPTH))
        # Symbol errors: usually a few (corrected), now and then 20 in one
        # code word (beyond the 16 RS corrects).
        if self.frames % 150 == 75:
            word = self.rng.randrange(self.DEPTH)
            for k in self.rng.sample(range(255), 20):
                block[k * self.DEPTH + word] ^= self.rng.randrange(1, 256)
        else:
            for _ in range(self.rng.choice([0, 0, 1, 2, 3, 8])):
                block[self.rng.randrange(len(block))] ^= self.rng.randrange(1, 256)
        return ASM + bytes(b ^ r for b, r in zip(block, self.rand))


# ---------------------------------------------------------------- sinks


LOG = threading.Lock()


def log(*a):
    with LOG:
        print(time.strftime("%H:%M:%S"), *a, flush=True)


class Fanout:
    """Hands each item to every connected TCP client."""

    def __init__(self, name, port):
        self.name, self.port = name, port
        self.clients = []
        self.lock = threading.Lock()
        threading.Thread(target=self.serve, daemon=True).start()

    def serve(self):
        srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        srv.bind(("0.0.0.0", self.port))
        srv.listen()
        log(f"{self.name}: tcp :{self.port}")
        while True:
            conn, addr = srv.accept()
            conn.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            log(f"{self.name}: {addr[0]} connected")
            with self.lock:
                self.clients.append(conn)

    def send(self, data):
        with self.lock:
            for c in list(self.clients):
                try:
                    c.sendall(data)
                except OSError:
                    log(f"{self.name}: a client left")
                    self.clients.remove(c)
                    c.close()


class Nats:
    """Publishes on a NATS subject; reconnects when the server goes away."""

    def __init__(self, host, port, subject):
        self.addr, self.subject = (host, port), subject
        self.q = queue.Queue(maxsize=10000)
        threading.Thread(target=self.run, daemon=True).start()

    def send(self, data):
        try:
            self.q.put_nowait(data)
        except queue.Full:
            pass

    def run(self):
        while True:
            try:
                s = socket.create_connection(self.addr, timeout=5)
                s.recv(4096)  # INFO
                s.sendall(b'CONNECT {"verbose":false,"pedantic":false,"name":"ccsds-sim"}\r\n')
                s.settimeout(0.05)
                log(f"nats: publishing on {self.subject} at {self.addr[0]}:{self.addr[1]}")
                while True:
                    try:
                        if s.recv(4096).startswith(b"PING"):
                            s.sendall(b"PONG\r\n")
                    except socket.timeout:
                        pass
                    try:
                        data = self.q.get(timeout=0.2)
                    except queue.Empty:
                        continue
                    s.sendall(f"PUB {self.subject} {len(data)}\r\n".encode() + data + b"\r\n")
            except OSError as e:
                log(f"nats: {e}; again in 3 s")
                time.sleep(3)


class Udp:
    def __init__(self, target):
        host, port = target.rsplit(":", 1)
        self.addr = (host, int(port))
        self.s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        log(f"udp: sending to {target}")

    def send(self, data):
        try:
            self.s.sendto(data, self.addr)
        except OSError:
            pass  # nothing listening yet, or the name doesn't resolve yet


class File:
    def __init__(self, path, limit):
        self.path, self.limit = path, limit
        os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
        open(path, "wb").close()
        log(f"file: appending to {path}")

    def send(self, data):
        if os.path.exists(self.path) and os.path.getsize(self.path) > self.limit:
            open(self.path, "wb").close()
            log("file: cut back to empty")
        with open(self.path, "ab") as f:
            f.write(data)


# ---------------------------------------------------------------- main


def selftest():
    assert crc16(b"123456789") == 0x29B1
    assert randomizer(4) == bytes.fromhex("FF480EC0")
    # The demo packet: TM(3,25) at 2026-09-27 14:32:05.500 UTC.
    unix = datetime(2026, 9, 27, 14, 32, 5, 500000, tzinfo=timezone.utc).timestamp()
    assert cuc(unix).hex() == "814b878a8000", cuc(unix).hex()
    pkt = pus_tm(1010, 0x123, 3, 25, 0x23, struct.pack(">HBHHB", 1, 1, 3000, 2600, 2), unix)
    assert pkt.hex() == "0bf2c1230016200319002300008" "14b878a80000001010bb80a28023401", pkt.hex()
    # Reed-Solomon: the check symbols of 0..222, as fenix-ccsds decodes
    # them (a capture of this simulator's CADUs corrects cleanly there).
    parity = ReedSolomon().encode(bytes(range(223)))
    assert parity.hex() == "4ffb92dd557ec67f27fb8982cf58f8fd028ad117fcef6b2793d0418826578651", parity.hex()
    f = Framer(random.Random(1))
    for p in Spacecraft(random.Random(1)).tick(unix):
        f.add(p)
    cadu = f.frame()
    assert cadu[:4] == ASM and len(cadu) == 4 + 1115 + 160
    print("selftest ok")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--capture", help="write CADUs to this file instead of serving")
    ap.add_argument("--packets", help="with --capture: also write the packets to this file")
    ap.add_argument("--seconds", type=float, default=60, help="with --capture: how much time to simulate")
    ap.add_argument("--seed", type=int, default=int(os.environ.get("SIM_SEED", "0")) or None)
    a = ap.parse_args()
    if a.selftest:
        selftest()
        return
    rng = random.Random(a.seed)
    craft, framer = Spacecraft(rng), Framer(rng)
    if a.capture:
        start = datetime(2026, 9, 27, 14, 30, tzinfo=timezone.utc).timestamp()
        with open(a.capture, "wb") as out, open(a.packets or os.devnull, "wb") as pk:
            for i in range(int(a.seconds * 2)):
                for p in craft.tick(start + i / 2):
                    framer.add(p)
                    pk.write(p)
                cadu = framer.frame()
                if cadu:
                    out.write(cadu)
        log(f"wrote {framer.frames} frames to {a.capture}")
        return

    env = os.environ.get
    packets = Fanout("packets", int(env("SIM_TCP_PACKETS", "10010")))
    cadus = Fanout("cadus", int(env("SIM_TCP_CADUS", "10011")))
    sinks = [packets]
    if env("SIM_UDP_TARGET"):
        sinks.append(Udp(env("SIM_UDP_TARGET")))
    if env("SIM_NATS"):
        host, port = env("SIM_NATS").rsplit(":", 1)
        sinks.append(Nats(host, int(port), env("SIM_NATS_SUBJECT", "tm.ops")))
    if env("SIM_FILE"):
        sinks.append(File(env("SIM_FILE"), int(env("SIM_FILE_MAX", str(4 * 1024 * 1024)))))
    log("flying: TM twice a second; Ctrl-C lands")
    next_t = time.time()
    while True:
        now = time.time()
        for p in craft.tick(now):
            for s in sinks:
                s.send(p)
            framer.add(p)
        cadu = framer.frame()
        if cadu:
            cadus.send(cadu)
        if craft.ticks % 120 == 0:
            log(f"{craft.ticks // 2} s: {sum(craft.seq.values())} packets sequenced, {framer.frames} frames")
        next_t += 0.5
        time.sleep(max(0, next_t - time.time()))


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(0)
