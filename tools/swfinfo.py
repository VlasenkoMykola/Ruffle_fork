#!/usr/bin/env python3
"""Summarizes how SWF files are structured and how they load each other.

Usage: python swfinfo.py <file or folder> [<file or folder> ...]

For each file it prints the ActionScript version, frame count, scenes, frame
labels, the frames where the main timeline stops, the frames and labels the
code jumps to, the files the code loads (and into which target), and which of
the other given files it mentions. Folders are read recursively. Only the
Python standard library is needed.
"""

import lzma
import os
import re
import struct
import sys
import zlib

MAX_ITEMS = 40

# Calls worth reporting, with the strings pushed for them.
LOAD_CALLS = {
    "loadMovie", "loadMovieNum", "unloadMovie", "unloadMovieNum", "loadClip",
    "unloadClip", "load", "loadVariables", "loadVariablesNum", "loadSound",
    "attachMovie", "duplicateMovieClip", "createEmptyMovieClip",
}
GOTO_CALLS = {"gotoAndPlay", "gotoAndStop", "nextScene", "prevScene", "nextFrame", "prevFrame"}

# Tag codes
END, SHOW_FRAME, DO_ACTION, DEFINE_BUTTON_2, DEFINE_SPRITE = 0, 1, 12, 34, 39
FRAME_LABEL, DO_INIT_ACTION, FILE_ATTRIBUTES, DO_ABC_DEFINE = 43, 59, 69, 72
SYMBOL_CLASS, DO_ABC, SCENE_AND_FRAME_LABELS = 76, 82, 86


class Reader:
    def __init__(self, data, pos=0):
        self.data = data
        self.pos = pos

    def u8(self):
        value = self.data[self.pos]
        self.pos += 1
        return value

    def u16(self):
        value = struct.unpack_from("<H", self.data, self.pos)[0]
        self.pos += 2
        return value

    def u32(self):
        value = struct.unpack_from("<I", self.data, self.pos)[0]
        self.pos += 4
        return value

    def u30(self):
        result, shift = 0, 0
        while True:
            byte = self.u8()
            result |= (byte & 0x7F) << shift
            shift += 7
            if not byte & 0x80 or shift > 35:
                return result

    def cstring(self):
        end = self.data.index(b"\0", self.pos)
        value = self.data[self.pos:end].decode("utf-8", "replace")
        self.pos = end + 1
        return value


def decompress(data):
    signature, version = data[:3], data[3]
    if signature == b"FWS":
        return version, "FWS", data[8:]
    if signature == b"CWS":
        return version, "CWS", zlib.decompressobj().decompress(data[8:])
    if signature == b"ZWS":
        props, dict_size = data[12], struct.unpack_from("<I", data, 13)[0]
        filters = [{
            "id": lzma.FILTER_LZMA1, "dict_size": dict_size,
            "lc": props % 9, "lp": (props // 9) % 5, "pb": props // 45,
        }]
        body = lzma.LZMADecompressor(lzma.FORMAT_RAW, filters=filters).decompress(data[17:])
        return version, "ZWS", body
    return None


def read_tags(data, pos, end):
    """Yields (code, body_start, body_end) for each tag in data[pos:end]."""
    while pos + 2 <= end:
        header = struct.unpack_from("<H", data, pos)[0]
        pos += 2
        code, length = header >> 6, header & 0x3F
        if length == 0x3F:
            length = struct.unpack_from("<I", data, pos)[0]
            pos += 4
        yield code, pos, min(pos + length, end)
        if code == END:
            return
        pos += length


class Actions:
    """What a block of AVM1 actions does, read without running it."""

    def __init__(self):
        self.stops = False
        self.gotos = []
        self.loads = []
        self.calls = []

    def scan(self, data, pos, end):
        constants = []
        recent = []  # Strings and numbers pushed lately, newest last
        while pos < end:
            code = data[pos]
            pos += 1
            if code == 0:
                break
            length = 0
            if code >= 0x80:
                if pos + 2 > end:
                    break
                length = struct.unpack_from("<H", data, pos)[0]
                pos += 2
            body = Reader(data, pos)
            try:
                if code == 0x07:  # Stop
                    self.stops = True
                elif code == 0x81:  # GotoFrame
                    self.gotos.append(str(body.u16() + 1))
                elif code == 0x8C:  # GotoLabel
                    self.gotos.append(repr(body.cstring()))
                elif code == 0x9F:  # GotoFrame2
                    target = recent[-1] if recent else "?"
                    self.gotos.append(str(target) if isinstance(target, int) else repr(target))
                elif code == 0x83:  # GetURL
                    url, target = body.cstring(), body.cstring()
                    self.loads.append(f"getURL({url!r}, {target!r})")
                elif code == 0x9A:  # GetURL2
                    args = [repr(v) for v in recent[-2:]]
                    self.loads.append(f"getURL2/loadMovie({', '.join(args)})")
                    recent = []
                elif code == 0x88:  # ConstantPool
                    constants = [body.cstring() for _ in range(body.u16())]
                elif code == 0x96:  # Push
                    while body.pos < pos + length:
                        kind = body.u8()
                        if kind == 0:
                            recent.append(body.cstring())
                        elif kind == 1:
                            body.pos += 4
                        elif kind in (2, 3):
                            pass
                        elif kind in (4, 5):
                            body.pos += 1
                        elif kind == 6:
                            body.pos += 8
                        elif kind == 7:
                            recent.append(struct.unpack_from("<i", data, body.pos)[0])
                            body.pos += 4
                        elif kind == 8:
                            index = body.u8()
                            recent.append(constants[index] if index < len(constants) else "?")
                        elif kind == 9:
                            index = body.u16()
                            recent.append(constants[index] if index < len(constants) else "?")
                        else:
                            break
                    recent = recent[-8:]
                elif code in (0x52, 0x3D):  # CallMethod, CallFunction
                    if recent and isinstance(recent[-1], str):
                        name = recent[-1]
                        args = [repr(v) for v in recent[:-1] if isinstance(v, str)][-3:]
                        call = f"{name}({', '.join(args)})"
                        if name in LOAD_CALLS:
                            self.loads.append(call)
                        elif name in GOTO_CALLS:
                            self.calls.append(call)
                    recent = []
                elif code in (0x1D, 0x4F, 0x3C, 0x17):  # SetVariable, SetMember, DefineLocal, Pop
                    recent = []
            except (IndexError, ValueError, struct.error):
                pass
            pos += length


def scan_button_2(data, start, end, actions):
    reader = Reader(data, start + 3)  # Button ID, flags
    offset_pos = reader.pos
    offset = reader.u16()
    if offset == 0:
        return
    pos = offset_pos + offset
    while pos + 4 <= end:
        size = struct.unpack_from("<H", data, pos)[0]
        actions.scan(data, pos + 4, pos + size if size else end)
        if size == 0:
            break
        pos += size


def abc_strings(data, start, end):
    reader = Reader(data, start + 4)  # Minor and major version
    try:
        for _ in range(max(reader.u30() - 1, 0)):  # Int pool
            reader.u30()
        for _ in range(max(reader.u30() - 1, 0)):  # Uint pool
            reader.u30()
        double_count = max(reader.u30() - 1, 0)
        reader.pos += 8 * double_count  # Double pool
        strings = []
        for _ in range(max(reader.u30() - 1, 0)):
            length = reader.u30()
            strings.append(data[reader.pos:reader.pos + length].decode("utf-8", "replace"))
            reader.pos += length
        return strings
    except (IndexError, struct.error):
        return []


def limited(items, formatter=str):
    items = list(items)
    shown = " ".join(formatter(item) for item in items[:MAX_ITEMS])
    return shown + (f" ... (+{len(items) - MAX_ITEMS} more)" if len(items) > MAX_ITEMS else "")


def dedupe(items):
    return list(dict.fromkeys(items))


def describe(path, display_name, other_names):
    data = open(path, "rb").read()
    size = f"{len(data) / 1024:.0f} KB"
    unpacked = decompress(data) if len(data) > 8 else None
    if unpacked is None:
        head = data[:48]
        if head.startswith(b"\x89PNG"):
            kind = "PNG image"
        elif head.startswith(b"\xff\xd8"):
            kind = "JPEG image"
        elif head.startswith(b"GIF8"):
            kind = "GIF image"
        elif all(32 <= b < 127 or b in (9, 10, 13) for b in head):
            kind = "text: " + head.decode("ascii").replace("\r", " ").replace("\n", " ")
        else:
            kind = "unknown, starts with " + head[:8].hex(" ")
        print(f"== {display_name}  ({size}, not a SWF: {kind})")
        return

    version, signature, body = unpacked
    nbits = body[0] >> 3
    rect_bytes = (5 + nbits * 4 + 7) // 8
    bits = int.from_bytes(body[:rect_bytes], "big")
    total_bits = rect_bytes * 8
    coords = [(bits >> (total_bits - 5 - nbits * (i + 1))) & ((1 << nbits) - 1) for i in range(4)]
    width, height = (coords[1] - coords[0]) // 20, (coords[3] - coords[2]) // 20
    frame_rate = body[rect_bytes + 1] + body[rect_bytes] / 256
    frame_count = struct.unpack_from("<H", body, rect_bytes + 2)[0]
    pos = rect_bytes + 4

    as3 = False
    scenes, scene_labels, frame_labels, stop_frames = None, [], [], []
    main = Actions()
    nested = Actions()
    abc = []
    classes = []
    sprite_count = 0
    frame = 1
    for code, start, end in read_tags(body, pos, len(body)):
        if code == SHOW_FRAME:
            frame += 1
        elif code == FILE_ATTRIBUTES and start < end:
            as3 = bool(body[start] & 0x08)
        elif code == FRAME_LABEL:
            frame_labels.append((frame, Reader(body, start).cstring()))
        elif code == SCENE_AND_FRAME_LABELS:
            reader = Reader(body, start)
            scenes = [(reader.u30() + 1, reader.cstring()) for _ in range(reader.u30())]
            scene_labels = [(reader.u30() + 1, reader.cstring()) for _ in range(reader.u30())]
        elif code == DO_ACTION:
            frame_actions = Actions()
            frame_actions.scan(body, start, end)
            if frame_actions.stops:
                stop_frames.append(frame)
            main.gotos += frame_actions.gotos
            main.loads += frame_actions.loads
            main.calls += frame_actions.calls
        elif code == DO_INIT_ACTION:
            nested.scan(body, start + 2, end)
        elif code == DEFINE_BUTTON_2:
            scan_button_2(body, start, end, nested)
        elif code == DEFINE_SPRITE:
            sprite_count += 1
            for sub_code, sub_start, sub_end in read_tags(body, start + 4, end):
                if sub_code == DO_ACTION:
                    nested.scan(body, sub_start, sub_end)
        elif code in (DO_ABC, DO_ABC_DEFINE):
            abc_start = start
            if code == DO_ABC:
                abc_start = body.index(b"\0", start + 4) + 1  # Skip the flags and name
            abc += abc_strings(body, abc_start, end)
        elif code == SYMBOL_CLASS:
            reader = Reader(body, start)
            for _ in range(reader.u16()):
                reader.u16()
                classes.append(reader.cstring())

    # Like Flash, FrameLabel tags only count when there's no scene data.
    labels = scene_labels if scenes is not None else frame_labels
    scenes = scenes or []
    print(f"== {display_name}  ({size}, {signature} v{version}, {'AS3' if as3 else 'AS1/AS2'}, "
          f"{width}x{height} @ {frame_rate:g} fps, {frame_count} frames, {sprite_count} sprites)")
    print("   scenes: " + (limited(scenes, lambda s: f"{s[0]}:{s[1]!r}") if scenes else "none"))
    print(f"   labels ({len(labels)}): " + (limited(labels, lambda l: f"{l[0]}:{l[1]!r}") or "none"))
    print(f"   main timeline stops at frames ({len(stop_frames)}): " + (limited(stop_frames) or "none"))
    for title, items in (
        ("main timeline goes to", main.gotos),
        ("main timeline calls", main.calls),
        ("main timeline loads", main.loads),
        ("sprites/buttons go to", nested.gotos),
        ("sprites/buttons call", nested.calls),
        ("sprites/buttons load", nested.loads),
    ):
        items = dedupe(items)
        if items:
            print(f"   {title} ({len(items)}): {limited(items)}")
    if classes:
        print(f"   AS3 classes ({len(classes)}): {limited(dedupe(classes), repr)}")
    if abc:
        pattern = re.compile(r"(\.swf|Loader|URLRequest|load|goto|scene|/[\w.-])", re.I)
        file_names = {os.path.basename(name) for name in other_names}
        # Leave out qualified method names like "pkg:Class/pkg:method".
        method_name = re.compile(r"^[\w$.]*:[\w$]+/")
        interesting = [s for s in dedupe(abc) if 0 < len(s) <= 100 and not method_name.match(s) and (
            pattern.search(s) or any(name in s for name in file_names if len(name) > 1))]
        print(f"   AS3 strings about loading/navigation ({len(interesting)}): {limited(interesting, repr)}")

    # Only look in readable strings, as short names would turn up in binary data by chance.
    text = "\n".join(re.findall(r"[\x20-\x7e]{4,}", body.decode("latin-1")))
    mentioned = [name for name in other_names if name != display_name and
                 re.search(r"(?<![\w-])" + re.escape(os.path.basename(name)) + r"(?![\w-])", text)]
    if mentioned:
        print(f"   mentions other files ({len(mentioned)}): {limited(mentioned)}")


def main(args):
    if not args:
        print(__doc__)
        return 1
    files = []
    for arg in args:
        if os.path.isdir(arg):
            for root, _, names in os.walk(arg):
                for name in sorted(names):
                    full = os.path.join(root, name)
                    rel = os.path.relpath(full, os.path.dirname(os.path.abspath(arg)))
                    files.append((full, rel.replace(os.sep, "/")))
        else:
            files.append((arg, os.path.basename(arg)))
    names = [name for _, name in files]
    for path, name in files:
        try:
            describe(path, name, names)
        except Exception as e:  # Keep going, so one odd file doesn't hide the rest
            print(f"== {name}  (couldn't read: {type(e).__name__}: {e})")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
