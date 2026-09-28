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
  tcp  :10013   the same CADUs with an FECF, one failing now and then
  tcp  :10012   the uplink: the MIB's telecommands in Type-A TC frames
                (segment header, FECF) in CLTUs, with faults -- a flipped
                bit in a code block, a bad FECF, a frame lost and sent
                again, a missing tail sequence -- and TM(1,x) reports
                answering the ones the spacecraft took
  udp           one packet per datagram to SIM_UDP_TARGET
  nats          one packet per message on SIM_NATS_SUBJECT
  file          packets appended to SIM_FILE, cut back to empty when it
                passes SIM_FILE_MAX octets

Standard library only. `python sim.py --capture cadus.bin --cltus cltus.bin --seconds 60`
writes the streams to files instead of serving anything, and
`python sim.py --selftest` checks the codecs against known values.

It never listens for telecommands: the uplink is simulated too, and
Fenix only reads it.
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
CLTU_START = bytes.fromhex("EB90")
CLTU_TAIL = bytes.fromhex("C5C5C5C5C5C5C579")


def bch_parity(info):
    """BCH(63,56) of 231.0-B: the remainder by x^7+x^6+x^2+1, complemented,
    then the filler bit 0."""
    reg = 0
    for byte in info:
        for k in range(7, -1, -1):
            feedback = ((reg >> 6) & 1) ^ ((byte >> k) & 1)
            reg = (reg << 1) & 0x7F
            if feedback:
                reg ^= 0b1000101
    return ((~reg) & 0x7F) << 1


def cltu(frame):
    """The start sequence, 8-octet code blocks (the last filled with 0x55),
    the tail sequence."""
    out = bytearray(CLTU_START)
    for i in range(0, len(frame), 7):
        info = frame[i:i + 7].ljust(7, b"\x55")
        out += info + bytes([bch_parity(info)])
    return bytes(out + CLTU_TAIL)


def pus_tc(apid, seq, service, subtype, data, ack=0b1001, source=0):
    """A PUS-C TC packet: version 2, the ack flags, the source ID, a CRC."""
    pkt = space_packet(apid, seq, bytes([0x20 | ack, service, subtype]) + struct.pack(">H", source) + data + b"\0\0", tm=False)[:-2]
    return pkt + struct.pack(">H", crc16(pkt))


def tc_frame(scid, vc, seq, data, bypass=False, control=False, segment=True):
    """A TC transfer frame: header, a segment header (unsegmented, MAP 0)
    unless it's a control command, the data, the FECF."""
    body = data if control else b"\xC0" + data
    n = 5 + len(body) + 2
    head = struct.pack(">HHB", (bypass << 13) | (control << 12) | scid, (vc << 10) | (n - 1), seq & 0xFF)
    frame = head + body
    return frame + struct.pack(">H", crc16(frame))

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
        self.pending = []  # (tick, apid, subtype, body) of verification reports

    def next_seq(self, apid, skip=0):
        s = (self.seq.get(apid, -1) + 1 + skip) & 0x3FFF
        self.seq[apid] = s
        return s

    def next_msg(self, key):
        self.msg[key] = (self.msg.get(key, 0) + 1) & 0xFFFF
        return self.msg[key]

    def tm(self, apid, service, subtype, body, unix, skip=0, bad_crc=False):
        return pus_tm(apid, self.next_seq(apid, skip), service, subtype, self.next_msg((apid, service, subtype)), body, unix, bad_crc)

    def received(self, tc, fails=False):
        """A telecommand came through: its verification reports follow,
        acceptance now and completion a second later, or a failed
        acceptance. They carry its packet ID and sequence control."""
        apid = int.from_bytes(tc[:2], "big") & 0x7FF
        request = tc[:4]
        if fails:
            self.pending.append((self.ticks + 1, apid, 2, request + struct.pack(">H", 0x0005)))
        else:
            self.pending.append((self.ticks + 1, apid, 1, request))
            self.pending.append((self.ticks + 3, apid, 7, request))

    def tick(self, unix):
        self.ticks += 1
        t = self.ticks
        out = []
        for due in [p for p in self.pending if p[0] <= t]:
            self.pending.remove(due)
            _, apid, subtype, body = due
            out.append(self.tm(apid, 1, subtype, body, unix))
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


class Link:
    """What the spacecraft's TC receiver (FARM) reports in the CLCW."""

    def __init__(self):
        self.nr = 0  # the next Type-A frame sequence number expected
        self.retransmit = False
        self.lockout = False

    def clcw(self):
        # Control word type 0, COP-1, VC 0; the flags; N(R).
        return (1 << 24) | (self.lockout << 13) | (self.retransmit << 11) | (self.nr & 0xFF)


class Framer:
    """Packets into TM frames on VC 0, idle-filled, as CADUs; with an FECF
    when asked."""

    LENGTH, DEPTH, SCID = 1115, 5, 0x0A5

    def __init__(self, rng, link, fecf=False):
        self.rng = rng
        self.link = link
        self.fecf = fecf
        self.rs = ReedSolomon()
        self.rand = randomizer(self.LENGTH + 32 * self.DEPTH)
        self.stream = bytearray()  # packet octets not yet framed
        self.starts = []  # offsets in self.stream where packets start
        self.mc = self.vc = 0
        self.frames = 0
        self.data_len = self.LENGTH - 6 - 4 - (2 if fecf else 0)  # header, OCF, FECF

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
        # Version 0, SCID, VC 0, OCF present; then the data field status:
        # no secondary header, packets, segment length id 11, the FHP.
        header = struct.pack(">HBBH", (self.SCID << 4) | 1, self.mc, self.vc, 0x1800 | fhp)
        self.mc = (self.mc + 1) & 0xFF
        self.vc = (self.vc + 1) & 0xFF
        frame = header + data + struct.pack(">I", self.link.clcw())
        if self.fecf:
            # Every 90 s, an FECF that doesn't match (after RS, which can't
            # see it: the error is in what was encoded).
            crc = crc16(frame) ^ (0x0001 if self.frames % 180 == 45 else 0)
            frame += struct.pack(">H", crc)
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


class Telecommander:
    """The uplink, as a ground station would put it on the wire: the MIB's
    telecommands as PUS-C packets in Type-A TC frames (VC 0, segment
    header, FECF), each frame in a CLTU after an acquisition sequence.

    The spacecraft end is modelled simply: it takes the Type-A frame it
    expects next, turns away anything else (and frames that fail their
    BCH or FECF), and the ground goes back and sends again when the CLCW
    asks for a retransmission, as FOP-1 does."""

    SCID, VC = 0x0A5, 0
    # name, APID, service, subtype, application data (per the MIB's CDF).
    PLAN = [
        ("ZTC08101", 1010, 8, 1, lambda r: bytes([12, r.randint(1, 8), r.randrange(3)])),
        ("ZTC08102", 1010, 8, 1, lambda r: struct.pack(">Bhh", r.randint(1, 8), -5, 45)),
        ("ZTC17001", 1008, 17, 1, lambda r: b""),
        ("ZTC03005", 1010, 3, 5, lambda r: struct.pack(">BH", 1, r.choice([1, 2]))),
        ("ZTC08001", 1010, 8, 1, lambda r: bytes([r.randint(1, 8)])),
        ("ZTC11004", 1008, 11, 4, lambda r: struct.pack(">BIH", 1, 2169210762 + r.randrange(3600), r.randrange(100))),
    ]

    def __init__(self, rng, craft, link):
        self.rng, self.craft, self.link = rng, craft, link
        self.vs = 0  # the next N(S)
        self.sent = {}  # N(S) -> packet, for going back
        self.queue = []  # packets to send again
        self.count = 0
        self.seq = {}
        self.log = []  # what happened, for --capture

    def packet(self):
        name, apid, service, subtype, data = self.PLAN[self.count % len(self.PLAN)]
        s = self.seq[apid] = (self.seq.get(apid, -1) + 1) & 0x3FFF
        return pus_tc(apid, s, service, subtype, data(self.rng))

    def next(self):
        """The next stretch of uplink: acquisition sequence and a CLTU."""
        self.count += 1
        c, link = self.count, self.link
        acquisition = b"\x55" * 16
        if link.retransmit and not self.queue:
            s = link.nr
            while s != self.vs:
                if s in self.sent:
                    self.queue.append(self.sent[s])
                s = (s + 1) & 0xFF
            self.vs = link.nr
            self.note(f"going back to N(S) {self.vs} for {len(self.queue)} frames")
        if c % 29 == 13:
            # A control command (Type-BC): Unlock. No segment header, no packet.
            self.note("BC Unlock")
            link.lockout = False
            return acquisition + cltu(tc_frame(self.SCID, self.VC, 0, b"\x00", bypass=True, control=True))
        if c % 23 == 11 and not self.queue:
            # A Type-BD frame: taken without the sequence check.
            p = self.packet()
            self.craft.received(p)
            self.note("BD frame")
            return acquisition + cltu(tc_frame(self.SCID, self.VC, 0, p, bypass=True))
        if c % 17 == 7 and not self.queue:
            # A frame lost on the way up: its N(S) is used, it never arrives.
            self.sent[self.vs] = self.packet()
            self.vs = (self.vs + 1) & 0xFF
            self.note("a frame lost on the way")
        packet = self.queue.pop(0) if self.queue else self.packet()
        ns = self.vs
        self.sent[ns] = packet
        self.vs = (ns + 1) & 0xFF
        frame = bytearray(tc_frame(self.SCID, self.VC, ns, packet))
        fault = None
        if c % 13 == 5 and not self.queue:
            frame[-1] ^= 0xFF
            fault = "FECF"
        unit = bytearray(cltu(bytes(frame)))
        if c % 11 == 3 and not self.queue and len(unit) > 2 + 8 * 3:
            unit[2 + 8 + 2] ^= 0x08  # a bit flipped in code block 2
            fault = "BCH"
        if c % 19 == 9 and fault is None:
            unit = unit[:-8]  # no tail sequence: the idle after it ends the CLTU
            self.note("no tail sequence")
        if fault:
            self.note(f"N(S) {ns}: {fault} fails, turned away")
            link.retransmit = True
        elif ns != link.nr:
            self.note(f"N(S) {ns}: expected {link.nr}, turned away")
            link.retransmit = True
        else:
            link.nr = (link.nr + 1) & 0xFF
            link.retransmit = False
            self.craft.received(packet, fails=c % 7 == 2)
        return acquisition + bytes(unit)

    def note(self, what):
        self.log.append((self.count, what))


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
    # A telecommand as fenix-mib encodes it: ZTC08101, line 3, AUTO, seq 7.
    tc = pus_tc(1010, 7, 8, 1, bytes([12, 3, 2]))
    assert tc.hex() == "1bf2c007000929080100000c03023fa3", tc.hex()
    unit = cltu(tc_frame(0x0A5, 0, 0, tc))
    assert unit[:2] == CLTU_START and unit[-8:] == CLTU_TAIL and (len(unit) - 10) % 8 == 0
    assert bch_parity(bytes(7)) == 0xFE and bch_parity(bytes([0xC5] * 7)) != 0x79, "the tail fails its parity"
    link = Link()
    f = Framer(random.Random(1), link, fecf=True)
    for p in Spacecraft(random.Random(1)).tick(unix):
        f.add(p)
    cadu = f.frame()
    assert cadu[:4] == ASM and len(cadu) == 4 + 1115 + 160
    print("selftest ok")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--capture", help="write CADUs to this file instead of serving")
    ap.add_argument("--fecf", help="with --capture: also write CADUs with an FECF to this file")
    ap.add_argument("--cltus", help="with --capture: also write the uplink's CLTUs to this file")
    ap.add_argument("--packets", help="with --capture: also write the TM packets to this file")
    ap.add_argument("--seconds", type=float, default=60, help="with --capture: how much time to simulate")
    ap.add_argument("--seed", type=int, default=int(os.environ.get("SIM_SEED", "0")) or None)
    a = ap.parse_args()
    if a.selftest:
        selftest()
        return
    rng = random.Random(a.seed)
    link = Link()
    craft = Spacecraft(rng)
    framer, framer_fecf = Framer(rng, link), Framer(rng, link, fecf=True)
    uplink = Telecommander(rng, craft, link)
    if a.capture:
        start = datetime(2026, 9, 27, 14, 30, tzinfo=timezone.utc).timestamp()
        sink = lambda path: open(path or os.devnull, "wb")
        with sink(a.capture) as out, sink(a.fecf) as out_fecf, sink(a.cltus) as up, sink(a.packets) as pk:
            for i in range(int(a.seconds * 2)):
                for p in craft.tick(start + i / 2):
                    framer.add(p)
                    framer_fecf.add(p)
                    pk.write(p)
                for fr, o in ((framer, out), (framer_fecf, out_fecf)):
                    cadu = fr.frame()
                    if cadu:
                        o.write(cadu)
                if i % 6 == 3:
                    up.write(uplink.next())
        log(f"wrote {framer.frames} frames to {a.capture}, {uplink.count} CLTUs")
        for n, what in uplink.log:
            log(f"  CLTU {n}: {what}")
        return

    env = os.environ.get
    packets = Fanout("packets", int(env("SIM_TCP_PACKETS", "10010")))
    cadus = Fanout("cadus", int(env("SIM_TCP_CADUS", "10011")))
    cltus = Fanout("cltus", int(env("SIM_TCP_CLTUS", "10012")))
    cadus_fecf = Fanout("cadus with FECF", int(env("SIM_TCP_CADUS_FECF", "10013")))
    sinks = [packets]
    if env("SIM_UDP_TARGET"):
        sinks.append(Udp(env("SIM_UDP_TARGET")))
    if env("SIM_NATS"):
        host, port = env("SIM_NATS").rsplit(":", 1)
        sinks.append(Nats(host, int(port), env("SIM_NATS_SUBJECT", "tm.ops")))
    if env("SIM_FILE"):
        sinks.append(File(env("SIM_FILE"), int(env("SIM_FILE_MAX", str(4 * 1024 * 1024)))))
    log("flying: TM twice a second, a telecommand every 3 s; Ctrl-C lands")
    next_t = time.time()
    while True:
        now = time.time()
        for p in craft.tick(now):
            for s in sinks:
                s.send(p)
            framer.add(p)
            framer_fecf.add(p)
        for fr, out in ((framer, cadus), (framer_fecf, cadus_fecf)):
            cadu = fr.frame()
            if cadu:
                out.send(cadu)
        if craft.ticks % 6 == 3:
            cltus.send(uplink.next())
            for n, what in uplink.log:
                log(f"uplink {n}: {what}")
            uplink.log.clear()
        if craft.ticks % 120 == 0:
            log(f"{craft.ticks // 2} s: {sum(craft.seq.values())} packets sequenced, {framer.frames} frames, {uplink.count} CLTUs")
        next_t += 0.5
        time.sleep(max(0, next_t - time.time()))


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(0)
