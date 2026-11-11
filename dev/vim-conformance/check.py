#!/usr/bin/env python3
"""Runs every case in crates/fenix-vim/tests/vim_conformance.txt through
real Vim and compares what Vim does with what the case expects.

    python3 dev/vim-conformance/check.py           # report differences
    python3 dev/vim-conformance/check.py --write   # take Vim's results

It needs `vim` and `script` (util-linux) on the PATH.

`--write` fills in (or corrects) each case's `end:` and `reg:` from Vim,
except for a case marked `differs:`, whose expectation is Fenix's own and
says why (see docs/VIM_DIFFERENCES.md). The cargo test
`vim_conformance` then holds Fenix to the same file.
"""
import json
import os
import shlex
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
CASES = os.path.join(ROOT, "crates", "fenix-vim", "tests", "vim_conformance.txt")

# Fenix's defaults, said in Vim: 4-space indents, Backspace over
# anything, one space after a join, no wrapping while typing.
SETUP = "set nocompatible noswapfile backspace=indent,eol,start shiftwidth=4 softtabstop=0 expandtab autoindent nojoinspaces textwidth=0 startofline"


def parse(path):
    """The cases, in order: each a dict of its fields, plus `name` and the
    line its name is on."""
    cases, case = [], None
    with open(path, encoding="utf-8") as f:
        for number, raw in enumerate(f, 1):
            line = raw.rstrip("\n")
            if not line.strip():
                case = None
                continue
            if line.startswith("#"):
                continue
            if case is None:
                case = {"name": line.strip(), "line": number, "fields": []}
                cases.append(case)
                continue
            key, _, value = line.strip().partition(":")
            case["fields"].append((key.strip(), value.strip()))
    return cases


def field(case, key):
    for k, v in case["fields"]:
        if k == key:
            return v
    return None


def unquote(value):
    return json.loads(value)


def quote(text):
    return json.dumps(text, ensure_ascii=False)


def split_cursor(text):
    at = text.index("|")
    text = text[:at] + text[at + 1:]
    before = text[:at]
    line = before.count("\n")
    col = len(before) - (before.rfind("\n") + 1)
    return text, line, col


def with_cursor(lines, line, col):
    lines = list(lines)
    lines[line] = lines[line][:col] + "|" + lines[line][col:]
    return "\n".join(lines)


def vim_string(keys):
    """Vim notation (`dw<Esc>`) as a double-quoted Vim string."""
    out, i = [], 0
    while i < len(keys):
        c = keys[i]
        if c == "<":
            end = keys.find(">", i)
            if end > i + 1:
                name = keys[i + 1:end]
                out.append("<" if name.lower() == "lt" else "\\<" + name + ">")
                i = end + 1
                continue
        out.append({"\\": "\\\\", '"': '\\"'}.get(c, c))
        i += 1
    return '"' + "".join(out) + '"'


def run_vim(case):
    text, line, col = split_cursor(unquote(field(case, "start")))
    lines = text.split("\n")
    with tempfile.TemporaryDirectory() as tmp:
        out = os.path.join(tmp, "out")
        script = os.path.join(tmp, "case.vim")
        register = field(case, "reg-before")
        with open(script, "w", encoding="utf-8") as f:
            f.write(SETUP + "\n")
            f.write("call setline(1, " + json.dumps(lines) + ")\n")
            # The window starts at line 1 and shows the whole case, as
            # Fenix's tests take it (`H`, `M`, `L`).
            f.write("set lines=50\n")
            f.write(f"call setcharpos('.', [0, {line + 1}, {col + 1}, 0])\n")
            f.write("call winrestview({'topline': 1})\n")
            f.write("call setreg('\"', " + (json.dumps(unquote(register)) if register else "''") + ")\n")
            # As typed (`t`), so undo steps fall where they would; `x`
            # runs them all, and ends Insert if they leave it open.
            f.write("call feedkeys(" + vim_string(field(case, "keys")) + ", 'tx')\n")
            f.write("call writefile([json_encode(getline(1, '$')), json_encode(getcharpos('.')), json_encode(getreg('\"'))], " + json.dumps(out) + ")\n")
            f.write("qa!\n")
        # On a terminal (`script` gives it one), not in Ex mode: without a
        # window, `H`, `M` and `L` don't go where they would.
        command = f"vim -N -u NONE -i NONE -n -S {shlex.quote(script)}"
        subprocess.run(["script", "-qc", command, "/dev/null"], check=False, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, timeout=20)
        with open(out, encoding="utf-8") as f:
            got_lines, pos, reg = [json.loads(l) for l in f.read().splitlines()]
    col = min(pos[2] - 1, max(len(got_lines[pos[1] - 1]), 0))
    return with_cursor(got_lines, pos[1] - 1, col), reg


def main():
    write = "--write" in sys.argv
    cases = parse(CASES)
    bad = 0
    updates = {}
    for case in cases:
        if field(case, "differs") is not None:
            continue
        end, reg = run_vim(case)
        want_end, want_reg = field(case, "end"), field(case, "reg")
        if want_end is None or unquote(want_end) != end or (want_reg is not None and unquote(want_reg) != reg):
            bad += 1
            print(f"{CASES}:{case['line']}: {case['name']}\n    vim:  end {quote(end)} reg {quote(reg)}\n    case: end {want_end} reg {want_reg}")
            updates[case["line"]] = (end, reg, want_reg is not None)
    if write and updates:
        rewrite(updates)
    print(f"{len(cases)} cases, {bad} {'updated' if write else 'differ from Vim'}")
    return 1 if bad and not write else 0


def rewrite(updates):
    """Puts Vim's `end:` (and `reg:`, where the case has one) into the
    cases the updates name, by the line their name is on, right after
    their `keys:`."""
    with open(CASES, encoding="utf-8") as f:
        lines = f.read().split("\n")
    out, current = [], None
    for number, line in enumerate(lines, 1):
        stripped = line.strip()
        if number in updates:
            current = updates[number]
            out.append(line)
            continue
        if not stripped:
            current = None
        if current:
            if stripped.startswith("end:") or (stripped.startswith("reg:") and current[2]):
                continue
            out.append(line)
            if stripped.startswith("keys:"):
                out.append(f"  end: {quote(current[0])}")
                if current[2]:
                    out.append(f"  reg: {quote(current[1])}")
            continue
        out.append(line)
    with open(CASES, "w", encoding="utf-8") as f:
        f.write("\n".join(out))


if __name__ == "__main__":
    sys.exit(main())
