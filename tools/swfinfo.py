#!/usr/bin/env python3
"""Summarizes how SWF files are structured and how they load each other.

Usage: python swfinfo.py [--dump | --dump-main] <file or folder> [<file or folder> ...]
       python swfinfo.py --classify [--onefolderdeep] <folder> [<folder> ...]
       python swfinfo.py --unsupported [--onefolderdeep] <folder> [<folder> ...]
       python swfinfo.py --unsupported-names [--onefolderdeep] <folder> [<folder> ...]
       python swfinfo.py --scenes <file> [<file> ...]
       python swfinfo.py --variables <file> [<file> ...]

For each file it prints the ActionScript version, frame count, scenes, frame
labels, the frames where the main timeline stops, the frames and labels the
code jumps to, the files the code loads (and into which target), the assets it
imports from other files, the movie clips with frame labels or long timelines,
and which of the other given files it mentions. SWFs embedded in a file are
described after it. Folders are read recursively.

With --dump, it also lists the tags of each file, outlines its AS3 classes with
what their methods use, and prints all of its AS1/AS2 code, which gets long for
big files. --dump-main prints only the code of the main timeline, which is enough
to see how many games move between their scenes.

--classify goes through folders of games instead, and prints for each game how it
handles its scenes: in one of the ways Ruffle's scene menu supports, with frame
labels only, or in a way it doesn't know yet. SWFs in a subfolder that a game
names, like parts/, or named by another SWF next to them, are taken for parts of
that game. --unsupported does the same, but leaves the supported games out of the
list (they're still counted). --unsupported-names prints only the paths of the games
that aren't supported, one per line, to run this script on one at a time. With
--onefolderdeep, these only look at the SWFs in the given folders and in their direct
subfolders, like one folder per game, and leave out SWFs further down, like those
bundled in HTML games.

--scenes lists the scenes of games that keep their own list of them, with what the
movie has under each name: the movie clip the game shows for it, its labels, and
what its code uses. The code of the main timeline follows, where the game sets up
the list and switches between its scenes.

--variables lists the code that picks the frame of a clip with a variable or a
condition, like the version of a character a game shows once the player picked it,
and where the game sets the variables that code reads. Then the clips that other code
moves between frames (gotoAndStop, nextFrame, stop... on them, or in tellTarget) or
shows and hides (_visible), like a character's versions picked by buttons, with
whether each stops by itself. A scene opened from the scene menu skips that code,
which leaves such clips playing through all of their versions.

Names with * or ? in them are expanded, as the Windows command line doesn't, so
python swfinfo.py "parts/mq*" describes every file whose name starts with mq.

Only the Python standard library is needed.
"""

import glob
import lzma
import os
import re
import struct
import sys
import zlib

MAX_ITEMS = 40
MAX_CLIP_LABELS = 12
# Movie clips with this many frames are listed even when they have no labels.
LONG_TIMELINE = 30

# Calls worth reporting, with the strings pushed for them.
LOAD_CALLS = {
    "loadMovie", "loadMovieNum", "unloadMovie", "unloadMovieNum", "loadClip",
    "unloadClip", "load", "loadVariables", "loadVariablesNum", "loadSound",
    "attachMovie", "duplicateMovieClip", "createEmptyMovieClip",
}
GOTO_CALLS = {"gotoAndPlay", "gotoAndStop", "nextScene", "prevScene", "nextFrame", "prevFrame"}

# Strings in code that look like file names, paths or URLs.
FILE_LIKE = re.compile(
    r"\.(swf|jpe?g|png|gif|mp3|wav|flv|mp4|xml|txt)\b|^https?:|^\w[\w.-]*/[\w./-]*$", re.I)

# Tag codes
END, SHOW_FRAME, PLACE_OBJECT, DEFINE_BUTTON, DO_ACTION, PLACE_OBJECT_2 = 0, 1, 4, 7, 12, 26
DEFINE_BUTTON_2, DEFINE_SPRITE, FRAME_LABEL, EXPORT_ASSETS = 34, 39, 43, 56
IMPORT_ASSETS, DO_INIT_ACTION, FILE_ATTRIBUTES, IMPORT_ASSETS_2 = 57, 59, 69, 71
DO_ABC_DEFINE, SYMBOL_CLASS, DO_ABC, SCENE_AND_FRAME_LABELS = 72, 76, 82, 86
PLACE_OBJECT_3, DEFINE_BINARY_DATA = 70, 87

TAG_NAMES = {
    0: "End", 1: "ShowFrame", 2: "DefineShape", 4: "PlaceObject", 5: "RemoveObject",
    6: "DefineBits", 7: "DefineButton", 8: "JPEGTables", 9: "SetBackgroundColor",
    10: "DefineFont", 11: "DefineText", 12: "DoAction", 13: "DefineFontInfo",
    14: "DefineSound", 15: "StartSound", 17: "DefineButtonSound", 18: "SoundStreamHead",
    19: "SoundStreamBlock", 20: "DefineBitsLossless", 21: "DefineBitsJPEG2",
    22: "DefineShape2", 23: "DefineButtonCxform", 24: "Protect", 26: "PlaceObject2",
    28: "RemoveObject2", 32: "DefineShape3", 33: "DefineText2", 34: "DefineButton2",
    35: "DefineBitsJPEG3", 36: "DefineBitsLossless2", 37: "DefineEditText",
    39: "DefineSprite", 41: "ProductInfo", 43: "FrameLabel", 45: "SoundStreamHead2",
    46: "DefineMorphShape", 48: "DefineFont2", 56: "ExportAssets", 57: "ImportAssets",
    58: "EnableDebugger", 59: "DoInitAction", 60: "DefineVideoStream", 61: "VideoFrame",
    62: "DefineFontInfo2", 63: "DebugID", 64: "EnableDebugger2", 65: "ScriptLimits",
    66: "SetTabIndex", 69: "FileAttributes", 70: "PlaceObject3", 71: "ImportAssets2",
    72: "DoABCDefine", 73: "DefineFontAlignZones", 74: "CSMTextSettings",
    75: "DefineFont3", 76: "SymbolClass", 77: "Metadata", 78: "DefineScalingGrid",
    82: "DoABC", 83: "DefineShape4", 84: "DefineMorphShape2",
    86: "DefineSceneAndFrameLabelData", 87: "DefineBinaryData", 88: "DefineFontName",
    89: "StartSound2", 90: "DefineBitsJPEG4", 91: "DefineFont4", 93: "EnableTelemetry",
    94: "PlaceObject4",
}

ACTION_NAMES = {
    0x00: "End", 0x04: "NextFrame", 0x05: "PrevFrame", 0x06: "Play", 0x07: "Stop",
    0x08: "ToggleQuality", 0x09: "StopSounds", 0x0A: "Add", 0x0B: "Subtract",
    0x0C: "Multiply", 0x0D: "Divide", 0x0E: "Equals", 0x0F: "Less", 0x10: "And",
    0x11: "Or", 0x12: "Not", 0x13: "StringEquals", 0x14: "StringLength",
    0x15: "StringExtract", 0x17: "Pop", 0x18: "ToInteger", 0x1C: "GetVariable",
    0x1D: "SetVariable", 0x20: "SetTarget2", 0x21: "StringAdd", 0x22: "GetProperty",
    0x23: "SetProperty", 0x24: "CloneSprite", 0x25: "RemoveSprite", 0x26: "Trace",
    0x27: "StartDrag", 0x28: "EndDrag", 0x29: "StringLess", 0x2A: "Throw",
    0x2B: "CastOp", 0x2C: "ImplementsOp", 0x30: "RandomNumber", 0x31: "MBStringLength",
    0x32: "CharToAscii", 0x33: "AsciiToChar", 0x34: "GetTime", 0x35: "MBStringExtract",
    0x36: "MBCharToAscii", 0x37: "MBAsciiToChar", 0x3A: "Delete", 0x3B: "Delete2",
    0x3C: "DefineLocal", 0x3D: "CallFunction", 0x3E: "Return", 0x3F: "Modulo",
    0x40: "NewObject", 0x41: "DefineLocal2", 0x42: "InitArray", 0x43: "InitObject",
    0x44: "TypeOf", 0x45: "TargetPath", 0x46: "Enumerate", 0x47: "Add2", 0x48: "Less2",
    0x49: "Equals2", 0x4A: "ToNumber", 0x4B: "ToString", 0x4C: "PushDuplicate",
    0x4D: "StackSwap", 0x4E: "GetMember", 0x4F: "SetMember", 0x50: "Increment",
    0x51: "Decrement", 0x52: "CallMethod", 0x53: "NewMethod", 0x54: "InstanceOf",
    0x55: "Enumerate2", 0x60: "BitAnd", 0x61: "BitOr", 0x62: "BitXor", 0x63: "BitLShift",
    0x64: "BitRShift", 0x65: "BitURShift", 0x66: "StrictEquals", 0x67: "Greater",
    0x68: "StringGreater", 0x69: "Extends", 0x81: "GotoFrame", 0x83: "GetURL",
    0x87: "StoreRegister", 0x88: "ConstantPool", 0x8A: "WaitForFrame", 0x8B: "SetTarget",
    0x8C: "GotoLabel", 0x8D: "WaitForFrame2", 0x8E: "DefineFunction2", 0x8F: "Try",
    0x94: "With", 0x96: "Push", 0x99: "Jump", 0x9A: "GetURL2", 0x9B: "DefineFunction",
    0x9D: "If", 0x9E: "Call", 0x9F: "GotoFrame2",
}

# onClipEvent flags of PlaceObject2
CLIP_EVENTS = [
    (0x1, "load"), (0x2, "enterFrame"), (0x4, "unload"), (0x8, "mouseMove"),
    (0x10, "mouseDown"), (0x20, "mouseUp"), (0x40, "keyDown"), (0x80, "keyUp"),
    (0x100, "data"), (0x200, "initialize"), (0x400, "press"), (0x800, "release"),
    (0x1000, "releaseOutside"), (0x2000, "rollOver"), (0x4000, "rollOut"),
    (0x8000, "dragOver"), (0x10000, "dragOut"), (0x20000, "keyPress"), (0x40000, "construct"),
]
KEY_PRESS = 0x20000


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

    def i16(self):
        value = struct.unpack_from("<h", self.data, self.pos)[0]
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


class Bits:
    """Reads the bit fields of matrices and color transforms."""

    def __init__(self, data, pos):
        self.data = data
        self.pos = pos
        self.bit = 0

    def ub(self, count):
        value = 0
        for _ in range(count):
            value = (value << 1) | ((self.data[self.pos] >> (7 - self.bit)) & 1)
            self.bit += 1
            if self.bit == 8:
                self.bit = 0
                self.pos += 1
        return value

    def end(self):
        return self.pos + (1 if self.bit else 0)


def skip_matrix(data, pos):
    bits = Bits(data, pos)
    for _ in range(2):  # Scale, rotation
        if bits.ub(1):
            bits.ub(2 * bits.ub(5))
    bits.ub(2 * bits.ub(5))  # Translation
    return bits.end()


def skip_color_transform(data, pos):
    bits = Bits(data, pos)
    has_add, has_mult = bits.ub(1), bits.ub(1)
    bits.ub(bits.ub(4) * 4 * (has_add + has_mult))
    return bits.end()


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


def decode_actions(data, start, end):
    """The action records in data[start:end] that can run, in address order, as
    (offset, code, body_start, body_length).

    It follows jumps and branches, as protected SWFs put junk after jumps that
    would otherwise end the code early or be read as more code."""
    records = {}
    pending = [(start, start, end)]  # Where to read from, within the code of a function or not
    while pending:
        pos, low, high = pending.pop()
        # A jump out of a function's code ends the function, like reaching its end.
        while low <= pos < high and pos not in records:
            code = data[pos]
            body, length = pos + 1, 0
            if code >= 0x80:
                if pos + 3 > high:
                    break
                length = struct.unpack_from("<H", data, pos + 1)[0]
                body = pos + 3
            next_pos = body + length
            if next_pos > high:
                break
            records[pos] = (code, body, length)
            if code in (0x00, 0x3E, 0x2A):  # End, Return, Throw
                break
            if code in (0x99, 0x9D) and length >= 2:  # Jump, If
                target = next_pos + struct.unpack_from("<h", data, body)[0]
                if code == 0x99:
                    pos = target
                    continue
                pending.append((target, low, high))
            elif code in (0x9B, 0x8E) and length >= 2:  # DefineFunction(2)
                # The function's code follows, and is skipped where it's defined.
                function_end = min(next_pos + struct.unpack_from("<H", data, next_pos - 2)[0], high)
                pending.append((next_pos, next_pos, function_end))
                pos = function_end
                continue
            elif code == 0x8F and length >= 7:  # Try: the catch and finally blocks follow
                try_size, catch_size = struct.unpack_from("<HH", data, body + 1)
                pending.append((next_pos + try_size, low, high))
                pending.append((next_pos + try_size + catch_size, low, high))
            pos = next_pos
    return [(offset, *records[offset]) for offset in sorted(records)]


def looks_like_decoy(constants):
    """Whether a constant pool is mostly garbled, like the decoys that protectors put
    in code that never runs, which would otherwise replace the real names."""
    garbled = sum(1 for c in constants
                  if "\ufffd" in c or any(ord(ch) < 32 and ch not in "\t\n\r" for ch in c))
    return garbled * 2 > len(constants)


class Special(str):
    """A pushed value that isn't a string, like a register or null."""

    def __repr__(self):
        return str(self)


def push_values(data, pos, end, constants):
    values = []
    while pos < end:
        kind = data[pos]
        pos += 1
        if kind == 0:
            stop = data.index(b"\0", pos)
            values.append(data[pos:stop].decode("utf-8", "replace"))
            pos = stop + 1
        elif kind in (1, 6):  # Float, double (stored as two words, high one first)
            if kind == 1:
                value = struct.unpack_from("<f", data, pos)[0]
                pos += 4
            else:
                value = struct.unpack("<d", data[pos + 4:pos + 8] + data[pos:pos + 4])[0]
                pos += 8
            values.append(int(value) if value == value and value.is_integer() else value)
        elif kind == 2:
            values.append(Special("null"))
        elif kind == 3:
            values.append(Special("undefined"))
        elif kind == 4:
            values.append(Special(f"r{data[pos]}"))
            pos += 1
        elif kind == 5:
            values.append(Special("true" if data[pos] else "false"))
            pos += 1
        elif kind == 7:
            values.append(struct.unpack_from("<i", data, pos)[0])
            pos += 4
        elif kind in (8, 9):
            if kind == 8:
                index = data[pos]
                pos += 1
            else:
                index = struct.unpack_from("<H", data, pos)[0]
                pos += 2
            values.append(constants[index] if index < len(constants) else Special(f"c{index}?"))
        else:
            break
    return values


class Actions:
    """What blocks of AVM1 actions do, read without running them."""

    def __init__(self):
        self.stops = False
        self.gotos = []
        self.loads = []
        self.calls = []
        self.strings = []
        self.arrays = []  # (variable name, values) of array literals
        self.functions = []  # Names of the functions defined
        # Frame numbers the code jumps its own timeline to, and the root timeline to
        # (with _root.gotoAndPlay(n), or inside tellTarget("_root")), as Ruffle's menu reads them
        self.own_frame_jumps = []
        self.root_frame_jumps = []

    def merge(self, other):
        self.stops |= other.stops
        self.gotos += other.gotos
        self.loads += other.loads
        self.calls += other.calls
        self.strings += other.strings
        self.arrays += other.arrays
        self.functions += other.functions
        self.own_frame_jumps += other.own_frame_jumps
        self.root_frame_jumps += other.root_frame_jumps

    def scan(self, data, start, end):
        constants = []
        recent = []  # Values pushed lately, newest last
        told = None  # The target of tellTarget: None for the code's own, "root" or "other"
        jumps = {None: self.own_frame_jumps, "root": self.root_frame_jumps, "other": []}
        for _, code, pos, length in decode_actions(data, start, end):
            body = Reader(data, pos)
            try:
                if code == 0x07:  # Stop
                    self.stops = True
                elif code == 0x81:  # GotoFrame
                    frame = body.u16() + 1
                    self.gotos.append(str(frame))
                    jumps[told].append(frame)
                elif code == 0x8C:  # GotoLabel
                    self.gotos.append(repr(body.cstring()))
                elif code == 0x9F:  # GotoFrame2
                    target = recent[-1] if recent else "?"
                    self.gotos.append(str(target) if isinstance(target, int) else repr(target))
                    if type(target) is int:
                        jumps[told].append(target)
                elif code == 0x8B:  # SetTarget
                    target = body.cstring()
                    told = None if not target else "root" if is_root_path(target) else "other"
                elif code == 0x83:  # GetURL
                    url, target = body.cstring(), body.cstring()
                    self.strings.append(url)
                    self.loads.append(f"getURL({url!r}, {target!r})")
                elif code == 0x9A:  # GetURL2
                    args = [repr(v) for v in recent[-2:]]
                    self.loads.append(f"getURL2/loadMovie({', '.join(args)})")
                    recent = []
                elif code == 0x88:  # ConstantPool
                    pool = [body.cstring() for _ in range(body.u16())]
                    if not looks_like_decoy(pool):
                        constants = pool
                        self.strings += constants
                elif code in (0x9B, 0x8E):  # DefineFunction(2)
                    name = body.cstring()
                    if name:
                        self.functions.append(name)
                elif code == 0x96:  # Push
                    values = push_values(data, pos, pos + length, constants)
                    self.strings += [v for v in values if type(v) is str]
                    recent = (recent + values)[-64:]
                elif code == 0x42:  # InitArray
                    count = recent[-1] if recent else None
                    if type(count) is int and 0 < count < len(recent):
                        values = recent[-1 - count:-1][::-1]
                        name = recent[-2 - count] if count + 2 <= len(recent) else None
                        self.arrays.append((name if type(name) is str else "?", values))
                        recent = recent[:-1 - count] + [Special("[array]")]
                elif code in (0x52, 0x3D):  # CallMethod, CallFunction
                    # _root.gotoAndPlay(n): n, 1, "_root" (got), "gotoAndPlay"
                    if (code == 0x52 and len(recent) >= 4 and recent[-1] in ("gotoAndPlay", "gotoAndStop")
                            and type(recent[-2]) is str and is_root_path(recent[-2])
                            and recent[-3] == 1 and type(recent[-4]) is int):
                        self.root_frame_jumps.append(recent[-4])
                    if recent and type(recent[-1]) is str:
                        name = recent[-1]
                        args = [repr(v) for v in recent[-8:-1] if type(v) is str][-3:]
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


def is_root_path(path):
    return path.lower() in ("_root", "_level0", "/", "_root/", "_level0/")


def disassemble(data, start, end):
    """The actions in data[start:end] as lines of text."""
    constants = []
    lines = []
    for offset, code, pos, length in decode_actions(data, start, end):
        body = Reader(data, pos)
        args = ""
        try:
            if code == 0x88:
                pool = [body.cstring() for _ in range(body.u16())]
                args = " ".join(f"c{i}={c!r}" for i, c in enumerate(pool))
                if looks_like_decoy(pool):
                    args += " (garbled, so taken for a decoy and ignored)"
                else:
                    constants = pool
            elif code == 0x96:
                args = ", ".join(repr(v) for v in push_values(data, pos, pos + length, constants))
            elif code == 0x81:
                args = str(body.u16() + 1)
            elif code in (0x8C, 0x8B):  # GotoLabel, SetTarget
                args = repr(body.cstring())
            elif code == 0x83:
                args = f"{body.cstring()!r} target={body.cstring()!r}"
            elif code in (0x99, 0x9D):
                args = f"-> {pos + length + body.i16() - start:04x}"
            elif code == 0x9B:
                name = body.cstring()
                params = [body.cstring() for _ in range(body.u16())]
                args = f"{name or '(anonymous)'}({', '.join(params)}) code {body.u16()} bytes"
            elif code == 0x8E:
                name = body.cstring()
                param_count = body.u16()
                body.u8()  # Register count
                body.u16()  # Flags
                params = []
                for _ in range(param_count):
                    register = body.u8()
                    params.append(body.cstring() + (f"=r{register}" if register else ""))
                args = f"{name or '(anonymous)'}({', '.join(params)}) code {body.u16()} bytes"
            elif code == 0x87:
                args = f"r{body.u8()}"
            elif code == 0x9F:
                flags = body.u8()
                args = ("play" if flags & 1 else "stop") + (f" scene bias {body.u16()}" if flags & 2 else "")
            elif code == 0x9A:
                flags = body.u8()
                kinds = ["loadVariables" if flags & 0x80 else "", "into clip" if flags & 0x40 else ""]
                method = ["", "GET", "POST", "?"][flags & 3]
                args = " ".join(k for k in kinds + [method] if k)
            elif code == 0x8A:
                args = f"frame {body.u16() + 1} else skip {body.u8()}"
            elif code == 0x8D:
                args = f"else skip {body.u8()}"
            elif code == 0x94:
                args = f"{body.u16()} bytes"
            elif code == 0x8F:
                body.u8()
                args = "try {} catch {} finally {} bytes".format(body.u16(), body.u16(), body.u16())
        except (IndexError, ValueError, struct.error):
            args = "(unreadable)"
        name = ACTION_NAMES.get(code, f"Unknown 0x{code:02X}")
        lines.append(f"{offset - start:04x} {name} {args}".rstrip())
    return lines


# Names that stand for timelines in paths, which aren't variables of their own
PATH_NAMES = re.compile(r"^(_root|_parent|_global|_level\d+|this|super)$", re.I)
PROPERTIES = [
    "_x", "_y", "_xscale", "_yscale", "_currentframe", "_totalframes", "_alpha", "_visible",
    "_width", "_height", "_rotation", "_target", "_framesloaded", "_name", "_droptarget", "_url",
    "_highquality", "_focusrect", "_soundbuftime", "_quality", "_xmouse", "_ymouse",
]
# Global names that are constants
CONSTANT_NAMES = {"NaN", "Infinity", "newline"}
BINARY_OPERATORS = {
    0x0A: "+", 0x47: "+", 0x0B: "-", 0x0C: "*", 0x0D: "/", 0x3F: "%", 0x21: "add", 0x0E: "==",
    0x49: "==", 0x66: "===", 0x0F: "<", 0x48: "<", 0x67: ">", 0x29: "lt", 0x68: "gt", 0x13: "eq",
    0x10: "and", 0x11: "or", 0x60: "&", 0x61: "|", 0x62: "^", 0x63: "<<", 0x64: ">>", 0x65: ">>>",
    0x54: "instanceof",
}
UNARY_FUNCTIONS = {
    0x18: "int", 0x4A: "Number", 0x4B: "String", 0x14: "length", 0x31: "mblength", 0x32: "ord",
    0x33: "chr", 0x36: "mbord", 0x37: "mbchr", 0x44: "typeof", 0x45: "targetPath",
    0x30: "random", 0x2B: "cast",
}


class Value:
    """A value that code works out, read without running it: its code, as text, the
    variables it reads, and the constant it is, if it is one."""

    def __init__(self, text, reads=(), constant=None, leaf=None, call=False):
        self.text = text
        self.reads = set(reads)  # Variables read to work it out, but the one it is
        self.constant = constant
        self.leaf = leaf  # The variable or member it is, last in its path
        self.call = call  # Whether it's what a call returns

    def variables(self):
        return self.reads | ({self.leaf} if self.leaf else set())


class Statement:
    def __init__(self, offset, kind, text, reads=(), sets=None, frame=None, function=None,
                 target=None):
        self.offset = offset
        self.function = function  # The name of the function it's in, if it's in one
        # The timeline that it moves between frames, or shows or hides, when it's another's
        # than the code's own, like `_root.hero` in _root.hero.gotoAndStop(2)
        self.target = target
        self.kind = kind  # "set", "goto", "if" or "other"
        self.text = text
        self.reads = set(reads)  # The variables the value set, the frame or the condition reads
        self.sets = sets  # The variable or member a "set" sets, last in its path
        self.frame = frame  # The Value of the frame a "goto" goes to


def path_leaf(path):
    """The variable that a path like _root.hero.hat, /hero:hat or hat names."""
    leaf = re.split(r"[.:/]", path)[-1]
    return None if not leaf or PATH_NAMES.match(leaf) else leaf


def read_function(data, pos, code):
    """(name, parameter names, registers they and the preloaded values go in, code size) of a
    DefineFunction(2) action whose body starts at `pos`."""
    body = Reader(data, pos)
    name = body.cstring()
    count = body.u16()
    registers, params = {}, []
    if code == 0x9B:
        params = [body.cstring() for _ in range(count)]
    else:
        body.u8()  # Register count
        flags = body.u16()
        # Preloaded values go in registers from 1, in this order.
        register = 1
        for flag, preloaded in ((0x01, "this"), (0x04, "arguments"), (0x10, "super"),
                                (0x40, "_root"), (0x80, "_parent"), (0x100, "_global")):
            if flags & flag:
                registers[register] = Value(preloaded)
                register += 1
        for _ in range(count):
            param_register = body.u8()
            param = body.cstring()
            params.append(param)
            if param_register:
                registers[param_register] = Value(param)
    return name, params, registers, body.u16()


def statements(data, start, end):
    """The statements of the AVM1 code in data[start:end], as far as reading it without
    running it tells, in address order. Values it can't work out are "?"."""
    constants, stack, registers, result = [], [], {}, []
    # (end, stack, registers, name, local names) of the functions being read, innermost last,
    # with what they replace
    functions = []
    local_names = None  # The parameters and local variables of the function being read
    told = None  # The target of tellTarget, if the code is in one

    def pop():
        return stack.pop() if stack else Value("?")

    def pop_count():
        count = pop().constant
        return count if type(count) is int and 0 <= count <= 256 else None

    def add(offset, kind, text, reads=(), sets=None, frame=None, target=None):
        indent = "  " * len(functions)
        function = (functions[-1][3] or "(anonymous)") if functions else None
        result.append(Statement(offset - start, kind, indent + text, reads, sets, frame, function,
                                target))

    def name_of(value):
        return value.constant if type(value.constant) is str else f"[{value.text}]"

    for offset, code, pos, length in decode_actions(data, start, end):
        while functions and offset >= functions[-1][0]:
            _, stack, registers, function, local_names = functions.pop()
            add(offset, "other", "}")
            if not function:
                stack.append(Value("function"))
        body = Reader(data, pos)
        try:
            if code == 0x88:  # ConstantPool
                pool = [body.cstring() for _ in range(body.u16())]
                if not looks_like_decoy(pool):
                    constants = pool
            elif code == 0x96:  # Push
                for value in push_values(data, pos, pos + length, constants):
                    if isinstance(value, Special) and re.match(r"r\d+$", value):
                        stack.append(registers.get(int(value[1:]), Value(value)))
                    else:
                        stack.append(Value(repr(value) if type(value) is str else str(value),
                                           constant=value))
            elif code == 0x1C:  # GetVariable
                name = pop()
                if type(name.constant) is str:
                    local = local_names is not None and name.constant in local_names
                    leaf = None if local or name.constant in CONSTANT_NAMES else path_leaf(name.constant)
                    stack.append(Value(name.constant, leaf=leaf))
                else:
                    stack.append(Value(f"eval({name.text})", name.variables()))
            elif code == 0x4E:  # GetMember
                member, target = pop(), pop()
                if type(member.constant) is str:
                    leaf = None if PATH_NAMES.match(member.constant) else member.constant
                    stack.append(Value(f"{target.text}.{member.constant}", target.reads, leaf=leaf))
                else:
                    stack.append(Value(f"{target.text}[{member.text}]",
                                       target.variables() | member.variables()))
            elif code == 0x1D:  # SetVariable
                value, name = pop(), pop()
                if type(name.constant) is str:
                    add(offset, "set", f"{name.constant} = {value.text}", value.variables(),
                        path_leaf(name.constant))
                else:
                    add(offset, "set", f"set({name.text}, {value.text})", value.variables())
            elif code == 0x4F:  # SetMember
                value, member, target = pop(), pop(), pop()
                if type(member.constant) is str:
                    shown = target.text if member.constant.lower() == "_visible" else None
                    add(offset, "set", f"{target.text}.{member.constant} = {value.text}",
                        value.variables(), member.constant, target=shown)
                else:
                    add(offset, "set", f"{target.text}[{member.text}] = {value.text}", value.variables())
            elif code in (0x3C, 0x41):  # DefineLocal(2)
                value = pop() if code == 0x3C else None
                name = name_of(pop())
                # In a function, it's a local variable, which isn't the game's; elsewhere, one
                # of the timeline.
                if local_names is not None:
                    local_names.add(name)
                text = f"var {name}" + (f" = {value.text}" if value else "")
                add(offset, "set", text, value.variables() if value else (),
                    None if local_names is not None else name)
            elif code == 0x23:  # SetProperty
                value, index, target = pop(), pop(), pop()
                prop = PROPERTIES[index.constant] if type(index.constant) is int and \
                    0 <= index.constant < len(PROPERTIES) else index.text
                shown = (target.constant if type(target.constant) is str else target.text) \
                    if prop == "_visible" else None
                add(offset, "set", f"setProperty({target.text}, {prop}, {value.text})", value.variables(),
                    target=shown)
            elif code == 0x22:  # GetProperty
                index, target = pop(), pop()
                prop = PROPERTIES[index.constant] if type(index.constant) is int and \
                    0 <= index.constant < len(PROPERTIES) else index.text
                stack.append(Value(f"getProperty({target.text}, {prop})", target.variables()))
            elif code in BINARY_OPERATORS:
                right, left = pop(), pop()
                stack.append(Value(f"({left.text} {BINARY_OPERATORS[code]} {right.text})",
                                   left.variables() | right.variables()))
            elif code in UNARY_FUNCTIONS:
                value = pop()
                stack.append(Value(f"{UNARY_FUNCTIONS[code]}({value.text})", value.variables()))
            elif code == 0x12:  # Not
                value = pop()
                stack.append(Value(f"!{value.text}", value.variables()))
            elif code in (0x50, 0x51):  # Increment, Decrement
                value = pop()
                stack.append(Value(f"({value.text} {'+' if code == 0x50 else '-'} 1)", value.variables()))
            elif code == 0x4C:  # PushDuplicate
                value = pop()
                stack += [value, value]
            elif code == 0x4D:  # StackSwap
                top, below = pop(), pop()
                stack += [top, below]
            elif code == 0x87:  # StoreRegister
                registers[body.u8()] = stack[-1] if stack else Value("?")
            elif code == 0x17:  # Pop
                value = pop()
                if value.call:
                    add(offset, "other", value.text, value.variables())
            elif code in (0x3D, 0x52, 0x40, 0x53):  # CallFunction, CallMethod, NewObject, NewMethod
                name = pop()
                target = pop() if code in (0x52, 0x53) else None
                count = pop_count()
                if count is None:
                    stack = [Value("?")]
                    continue
                args = [pop() for _ in range(count)]
                reads = set().union(*(arg.variables() for arg in args))
                function = name.constant if type(name.constant) is str else None
                if target is None:
                    callee = function or f"[{name.text}]"
                else:
                    callee = target.text + (f".{function}" if function else "")
                text = ("new " if code in (0x40, 0x53) else "") + \
                    f"{callee}({', '.join(arg.text for arg in args)})"
                moved = target.text if target is not None and code == 0x52 else None
                if function in ("gotoAndPlay", "gotoAndStop") and args:
                    add(offset, "goto", text, args[0].variables(), frame=args[0], target=moved)
                    stack.append(Value("undefined"))
                elif function in ("nextFrame", "prevFrame", "play", "stop") and moved:
                    add(offset, "other", text, target=moved)
                    stack.append(Value("undefined"))
                else:
                    stack.append(Value(text, reads | (target.reads if target else set()), call=True))
            elif code == 0x9F:  # GotoFrame2
                flags = body.u8()
                frame = pop()
                add(offset, "goto", f"{'gotoAndPlay' if flags & 1 else 'gotoAndStop'}({frame.text})",
                    frame.variables(), frame=frame, target=told)
            elif code == 0x81:  # GotoFrame
                frame = body.u16() + 1
                add(offset, "goto", f"gotoFrame({frame})", frame=Value(str(frame), constant=frame),
                    target=told)
            elif code == 0x8C:  # GotoLabel
                label = body.cstring()
                add(offset, "goto", f"gotoLabel({label!r})", frame=Value(repr(label), constant=label),
                    target=told)
            elif code in (0x04, 0x05, 0x06, 0x07):
                add(offset, "other", {0x04: "nextFrame()", 0x05: "prevFrame()", 0x06: "play()",
                                      0x07: "stop()"}[code], target=told)
            elif code == 0x8B:  # SetTarget
                target = body.cstring()
                add(offset, "other", f"tellTarget({target!r}) {{" if target else "}")
                told = target or None
            elif code == 0x20:  # SetTarget2
                told = pop().text
                add(offset, "other", f"tellTarget({told}) {{")
            elif code == 0x9D:  # If
                condition = pop()
                add(offset, "if", f"if {condition.text} -> {pos + length + body.i16() - start:04x}",
                    condition.variables())
            elif code == 0x99:  # Jump
                add(offset, "other", f"-> {pos + length + body.i16() - start:04x}")
            elif code == 0x26:  # Trace
                add(offset, "other", f"trace({pop().text})")
            elif code == 0x3E:  # Return
                value = pop()
                add(offset, "other", f"return {value.text}", value.variables())
            elif code == 0x42:  # InitArray
                count = pop_count()
                if count is None:
                    stack = [Value("?")]
                    continue
                items = [pop() for _ in range(count)]
                stack.append(Value(f"[{', '.join(item.text for item in items)}]",
                                   set().union(*(item.variables() for item in items))))
            elif code == 0x43:  # InitObject
                count = pop_count()
                if count is None:
                    stack = [Value("?")]
                    continue
                pairs = [(pop(), pop()) for _ in range(count)]
                stack.append(Value("{" + ", ".join(f"{name_of(name)}: {value.text}"
                                                   for value, name in pairs) + "}",
                                   set().union(*(value.variables() for value, _ in pairs))))
            elif code == 0x3A:  # Delete
                member, target = pop(), pop()
                stack.append(Value(f"delete {target.text}.{name_of(member)}", call=True))
            elif code == 0x3B:  # Delete2
                stack.append(Value(f"delete {name_of(pop())}", call=True))
            elif code in (0x9B, 0x8E):  # DefineFunction(2)
                name, params, function_registers, size = read_function(data, pos, code)
                add(offset, "other", f"function {name}({', '.join(params)}) {{")
                functions.append((pos + length + size, stack, registers, name, local_names))
                stack, registers, local_names = [], dict(function_registers), set(params)
            elif code == 0x9E:  # Call
                add(offset, "other", f"call({pop().text})")
            elif code == 0x9A:  # GetURL2
                target, url = pop(), pop()
                add(offset, "other", f"getURL({url.text}, {target.text})")
            elif code == 0x34:  # GetTime
                stack.append(Value("getTimer()"))
            elif code == 0x00:  # End
                pass
            else:  # What it does to the stack isn't followed, so it's forgotten.
                stack = []
        except (IndexError, ValueError, struct.error):
            stack = []
    while functions:
        functions.pop()
        add(end, "other", "}")
    return result


def button_1_actions(data, start, end):
    pos = start + 2  # Button ID
    while pos < end and data[pos]:  # Button records end with a 0
        pos = skip_matrix(data, pos + 5)  # Flags, character ID, depth
    return [(pos + 1, end)]


def button_2_actions(data, start, end):
    reader = Reader(data, start + 3)  # Button ID, flags
    offset_pos = reader.pos
    offset = reader.u16()
    if offset == 0:
        return []
    pos = offset_pos + offset
    blocks = []
    while pos + 4 <= end:
        size = struct.unpack_from("<H", data, pos)[0]
        blocks.append((pos + 4, pos + size if size else end))
        if size == 0:
            break
        pos += size
    return blocks


def clip_actions(data, start, end, version):
    """(depth, event names, start, end) of the onClipEvent handlers in a PlaceObject2 tag."""
    flags = data[start]
    if not flags & 0x80:
        return []
    reader = Reader(data, start + 1)
    depth = reader.u16()
    if flags & 0x02:
        reader.u16()  # Character ID
    if flags & 0x04:
        reader.pos = skip_matrix(data, reader.pos)
    if flags & 0x08:
        reader.pos = skip_color_transform(data, reader.pos)
    if flags & 0x10:
        reader.u16()  # Ratio
    if flags & 0x20:
        reader.cstring()  # Name
    if flags & 0x40:
        reader.u16()  # Clip depth
    read_events = reader.u32 if version >= 6 else reader.u16
    reader.u16()  # Reserved
    read_events()  # All events
    handlers = []
    try:
        while reader.pos < end:
            events = read_events()
            if not events:
                break
            size = reader.u32()
            actions_start = reader.pos
            reader.pos += size
            if events & KEY_PRESS:
                actions_start += 1  # Key code
            names = "/".join(name for bit, name in CLIP_EVENTS if events & bit)
            handlers.append((depth, names, actions_start, min(reader.pos, end)))
    except struct.error:  # Cut off, which Flash Player tolerates
        pass
    return handlers


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


# AVM2 instructions with operands, and their operands: "u" for a variable-length
# integer, "b" for a byte, "j" for a jump offset, "s" for a lookupswitch.
AVM2_OPERANDS = {
    0x04: "u", 0x05: "u", 0x06: "u", 0x08: "u", 0x1B: "s", 0x24: "b", 0x25: "u",
    0x2C: "u", 0x2D: "u", 0x2E: "u", 0x2F: "u", 0x31: "u", 0x32: "uu", 0x40: "u",
    0x41: "u", 0x42: "u", 0x43: "uu", 0x44: "uu", 0x45: "uu", 0x46: "uu", 0x49: "u",
    0x4A: "uu", 0x4C: "uu", 0x4E: "uu", 0x4F: "uu", 0x53: "u", 0x55: "u", 0x56: "u",
    0x58: "u", 0x59: "u", 0x5A: "u", 0x5D: "u", 0x5E: "u", 0x5F: "u", 0x60: "u",
    0x61: "u", 0x62: "u", 0x63: "u", 0x65: "b", 0x66: "u", 0x68: "u", 0x6A: "u",
    0x6C: "u", 0x6D: "u", 0x6E: "u", 0x6F: "u", 0x80: "u", 0x86: "u", 0x92: "u",
    0x94: "u", 0xB2: "u", 0xC2: "u", 0xC3: "u", 0xEF: "bubu", 0xF0: "u", 0xF1: "u",
    **{code: "j" for code in range(0x0C, 0x1B)},
}
# Instructions whose first operand is a property name: getsuper, setsuper,
# callsuper, callproperty, constructprop, callproplex, callsupervoid,
# callpropvoid, getdescendants, findpropstrict, findproperty, getlex,
# setproperty, getproperty, initproperty, deleteproperty.
AVM2_NAME_OPS = {0x04, 0x05, 0x45, 0x46, 0x4A, 0x4C, 0x4E, 0x4F, 0x59, 0x5D, 0x5E, 0x60,
                 0x61, 0x66, 0x68, 0x6A}
PUSH_STRING = 0x2C
TRAIT_KINDS = {0: "var", 1: "function", 2: "get", 3: "set", 4: "class", 5: "function", 6: "const"}


class Abc:
    """The classes of an AS3 bytecode block, and what their methods use."""

    def __init__(self, data, start):
        reader = self.reader = Reader(data, start + 4)  # Minor and major version
        self.data = data
        for _ in range(max(reader.u30() - 1, 0)):  # Int pool
            reader.u30()
        for _ in range(max(reader.u30() - 1, 0)):  # Uint pool
            reader.u30()
        double_count = max(reader.u30() - 1, 0)
        reader.pos += 8 * double_count  # Double pool
        self.strings = [""]
        for _ in range(max(reader.u30() - 1, 0)):
            length = reader.u30()
            self.strings.append(data[reader.pos:reader.pos + length].decode("utf-8", "replace"))
            reader.pos += length
        for _ in range(max(reader.u30() - 1, 0)):  # Namespaces
            reader.u8()
            reader.u30()
        for _ in range(max(reader.u30() - 1, 0)):  # Namespace sets
            for _ in range(reader.u30()):
                reader.u30()
        self.multinames = [None]
        for _ in range(max(reader.u30() - 1, 0)):
            kind = reader.u8()
            if kind in (0x07, 0x0D):  # QName
                reader.u30()
                self.multinames.append(reader.u30())
            elif kind in (0x0F, 0x10):  # RTQName
                self.multinames.append(reader.u30())
            elif kind in (0x09, 0x0E):  # Multiname
                self.multinames.append(reader.u30())
                reader.u30()
            elif kind in (0x11, 0x12, 0x1B, 0x1C):  # Runtime names
                if kind in (0x1B, 0x1C):
                    reader.u30()
                self.multinames.append(None)
            elif kind == 0x1D:  # Generic type, like Vector.<int>
                base = reader.u30()
                self.multinames.append((base, [reader.u30() for _ in range(reader.u30())]))
            else:
                raise ValueError(f"unknown multiname kind {kind}")

        self.methods = []  # (parameter types, parameter names, return type)
        for _ in range(reader.u30()):
            param_count = reader.u30()
            return_type = reader.u30()
            param_types = [reader.u30() for _ in range(param_count)]
            reader.u30()  # Name
            flags = reader.u8()
            if flags & 0x08:  # Optional parameters
                for _ in range(reader.u30()):
                    reader.u30()
                    reader.u8()
            param_names = [reader.u30() for _ in range(param_count)] if flags & 0x80 else None
            self.methods.append((param_types, param_names, return_type))
        for _ in range(reader.u30()):  # Metadata
            reader.u30()
            for _ in range(reader.u30()):
                reader.u30()
                reader.u30()
        class_count = reader.u30()
        self.classes = []  # (name, superclass, constructor, instance traits)
        for _ in range(class_count):
            name, superclass, flags = reader.u30(), reader.u30(), reader.u8()
            if flags & 0x08:  # Protected namespace
                reader.u30()
            for _ in range(reader.u30()):  # Interfaces
                reader.u30()
            constructor = reader.u30()
            self.classes.append((name, superclass, constructor, self.read_traits()))
        self.statics = []
        for _ in range(class_count):
            reader.u30()  # Class initializer
            self.statics.append(self.read_traits())
        for _ in range(reader.u30()):  # Scripts
            reader.u30()
            self.read_traits()
        self.bodies = {}  # Method index -> (names used, strings pushed)
        for _ in range(reader.u30()):
            method = reader.u30()
            for _ in range(4):  # Max stack, locals, scope depths
                reader.u30()
            code_length = reader.u30()
            self.bodies[method] = self.read_code(reader.pos, reader.pos + code_length)
            reader.pos += code_length
            for _ in range(reader.u30()):  # Exceptions
                for _ in range(5):
                    reader.u30()
            self.read_traits()

    def read_traits(self):
        reader = self.reader
        traits = []
        for _ in range(reader.u30()):
            name, kind_byte = reader.u30(), reader.u8()
            kind = kind_byte & 0x0F
            if kind in (0, 6):  # Slot, const
                reader.u30()
                info = reader.u30()  # Type
                if reader.u30():
                    reader.u8()
            else:  # Method, getter, setter, class, function
                reader.u30()
                info = reader.u30()
            if kind_byte & 0x40:  # Metadata
                for _ in range(reader.u30()):
                    reader.u30()
            traits.append((kind, name, info))
        return traits

    def read_code(self, pos, end):
        reader = Reader(self.data, pos)
        names, strings = [], []
        while reader.pos < end:
            code = reader.u8()
            for index, operand in enumerate(AVM2_OPERANDS.get(code, "")):
                if operand == "u":
                    value = reader.u30()
                    if index == 0 and code in AVM2_NAME_OPS:
                        names.append(self.name(value))
                    elif code == PUSH_STRING:
                        strings.append(self.strings[value] if value < len(self.strings) else "?")
                elif operand == "b":
                    reader.pos += 1
                elif operand == "j":
                    reader.pos += 3
                elif operand == "s":
                    reader.pos += 3
                    case_count = reader.u30() + 1
                    reader.pos += 3 * case_count
        return names, strings

    def name(self, index):
        entry = self.multinames[index] if 0 < index < len(self.multinames) else None
        if entry is None:
            return "*"
        if isinstance(entry, tuple):
            base, params = entry
            return f"{self.name(base)}.<{', '.join(self.name(param) for param in params)}>"
        return self.strings[entry] if entry < len(self.strings) else "?"

    def signature(self, method):
        if method >= len(self.methods):
            return "(?)"
        param_types, param_names, return_type = self.methods[method]
        params = []
        for i, param_type in enumerate(param_types):
            param_name = self.strings[param_names[i]] if param_names else ""
            params.append(f"{param_name or f'arg{i + 1}'}: {self.name(param_type)}")
        return f"({', '.join(params)}): {self.name(return_type)}"

    def outline(self):
        """The classes as lines of text."""
        lines = []
        for (name, superclass, constructor, traits), statics in zip(self.classes, self.statics):
            lines.append(f"class {self.name(name)} extends {self.name(superclass)}")
            members = [("", "constructor", name, constructor, 1)]
            members += [("", TRAIT_KINDS.get(kind, "?"), trait, info, kind) for kind, trait, info in traits]
            members += [("static ", TRAIT_KINDS.get(kind, "?"), trait, info, kind) for kind, trait, info in statics]
            for prefix, kind_name, trait, info, kind in members:
                if kind in (0, 6):
                    lines.append(f"   {prefix}{kind_name} {self.name(trait)}: {self.name(info)}")
                    continue
                if kind == 4:
                    lines.append(f"   {prefix}class {self.name(trait)}")
                    continue
                shown = "" if kind_name == "constructor" else f" {self.name(trait)}"
                lines.append(f"   {prefix}{kind_name}{shown}{self.signature(info)}")
                names, strings = self.bodies.get(info, ([], []))
                names = dedupe(name for name in names if name != "*")
                if names:
                    lines.append(f"      uses: {limited(names, limit=60)}")
                if strings:
                    lines.append(f"      strings: {limited(dedupe(strings), repr, 30)}")
        return lines


def limited(items, formatter=str, limit=MAX_ITEMS):
    items = list(items)
    shown = " ".join(formatter(item) for item in items[:limit])
    return shown + (f" ... (+{len(items) - limit} more)" if len(items) > limit else "")


def dedupe(items):
    return list(dict.fromkeys(items))


def format_labels(labels, limit=MAX_ITEMS):
    return limited(labels, lambda label: f"{label[0]}:{label[1]!r}", limit)


class Timeline:
    """Scans the code of a timeline and of the clips placed on it."""

    def __init__(self, data, version, frame_actions, nested, blocks):
        self.data = data
        self.version = version
        self.frame_actions = frame_actions  # Where the code of its frames goes
        self.nested = nested  # Where the code of clips placed on it goes
        self.blocks = blocks  # (title, start, end) of each block of code, for --dump

    def frame_code(self, title, start, end):
        actions = Actions()
        actions.scan(self.data, start, end)
        self.frame_actions.merge(actions)
        self.blocks.append((title, start, end))
        return actions

    def placed_code(self, where, start, end):
        actions = Actions()
        try:
            handlers = clip_actions(self.data, start, end, self.version)
        except (IndexError, ValueError, struct.error):
            return actions
        try:
            character = placed_character(PLACE_OBJECT_2, self.data, start)
            name = placed_name(PLACE_OBJECT_2, self.data, start)
        except (IndexError, ValueError, struct.error):
            character, name = None, None
        clip = ("the clip" if character is None else f"clip #{character}") + (f" {name!r}" if name else "")
        for depth, events, code_start, code_end in handlers:
            actions.scan(self.data, code_start, code_end)
            self.blocks.append((f"{where}, onClipEvent({events}) of {clip} at depth {depth}",
                                code_start, code_end))
        self.nested.merge(actions)
        return actions


def placed_character(code, data, start):
    """The ID of the character that a PlaceObject tag places, if it names one."""
    if code == PLACE_OBJECT:
        return struct.unpack_from("<H", data, start)[0]
    flags = data[start]
    reader = Reader(data, start + 1)
    if code == PLACE_OBJECT_3:
        flags_2 = reader.u8()
        reader.u16()  # Depth
        if flags_2 & 0x08 or (flags_2 & 0x10 and flags & 0x02):
            reader.cstring()  # Class name
    else:
        reader.u16()  # Depth
    return reader.u16() if flags & 0x02 else None


def placed_name(code, data, start):
    """The instance name that a PlaceObject tag gives what it places, if it gives one."""
    if code == PLACE_OBJECT:
        return None
    flags = data[start]
    reader = Reader(data, start + 1)
    if code == PLACE_OBJECT_3:
        flags_2 = reader.u8()
        reader.u16()  # Depth
        if flags_2 & 0x08 or (flags_2 & 0x10 and flags & 0x02):
            reader.cstring()  # Class name
    else:
        reader.u16()  # Depth
    if not flags & 0x20:
        return None
    if flags & 0x02:
        reader.u16()  # Character ID
    if flags & 0x04:
        reader.pos = skip_matrix(data, reader.pos)
    if flags & 0x08:
        reader.pos = skip_color_transform(data, reader.pos)
    if flags & 0x10:
        reader.u16()  # Ratio
    return reader.cstring()


def not_swf_kind(data):
    """What a file that isn't a SWF seems to be."""
    head = data[:48]
    if head.startswith(b"\x89PNG"):
        return "PNG image"
    if head.startswith(b"\xff\xd8"):
        return "JPEG image"
    if head.startswith(b"GIF8"):
        return "GIF image"
    if all(32 <= b < 127 or b in (9, 10, 13) for b in head):
        return "text: " + head.decode("ascii").replace("\r", " ").replace("\n", " ")
    return "unknown, starts with " + head[:8].hex(" ")


class Movie:
    """What a SWF file has and does, as far as this script reads it."""

    def __init__(self, data):
        self.size = len(data)
        self.version, self.signature, body = decompress(data)
        self.body = body
        nbits = body[0] >> 3
        rect_bytes = (5 + nbits * 4 + 7) // 8
        bits = int.from_bytes(body[:rect_bytes], "big")
        total_bits = rect_bytes * 8
        coords = [(bits >> (total_bits - 5 - nbits * (i + 1))) & ((1 << nbits) - 1) for i in range(4)]
        self.width, self.height = (coords[1] - coords[0]) // 20, (coords[3] - coords[2]) // 20
        self.frame_rate = body[rect_bytes + 1] + body[rect_bytes] / 256
        self.frame_count = struct.unpack_from("<H", body, rect_bytes + 2)[0]
        pos = rect_bytes + 4

        self.as3 = False
        scenes, scene_labels, frame_labels, self.stop_frames = None, [], [], []
        self.main, self.nested, self.blocks = Actions(), Actions(), []
        main, nested, blocks = self.main, self.nested, self.blocks
        timeline = Timeline(body, self.version, main, nested, blocks)
        self.abc, self.abc_blocks, self.classes = [], [], []
        # Embedded: (character ID, start, end) of SWFs in binary data
        self.symbol_names, self.embedded = {}, []
        self.sprites = []  # (ID, frame count, labels)
        # Strings in the code of each clip and button, and the characters placed in each clip
        self.own_strings, self.children = {}, {}
        # The characters placed on the main timeline, and the frames each button's code
        # jumps its timeline to
        self.root_children, self.button_jumps = set(), {}
        # Where each character is placed, and the instance names it's given
        self.placements, self.instance_names = {}, {}
        self.exports, self.imports = {}, []
        self.tag_counts = {}
        frame = 1
        for code, start, end in read_tags(body, pos, len(body)):
            self.tag_counts[code] = self.tag_counts.get(code, 0) + 1
            if code == SHOW_FRAME:
                frame += 1
            elif code == FILE_ATTRIBUTES and start < end:
                self.as3 = bool(body[start] & 0x08)
            elif code == FRAME_LABEL:
                frame_labels.append((frame, Reader(body, start).cstring()))
            elif code == SCENE_AND_FRAME_LABELS:
                reader = Reader(body, start)
                scenes = []
                try:  # Counts can claim more than the tag holds; what fits is kept
                    for _ in range(reader.u30()):
                        scenes.append((reader.u30() + 1, reader.cstring()))
                    for _ in range(reader.u30()):
                        scene_labels.append((reader.u30() + 1, reader.cstring()))
                except (IndexError, ValueError):
                    pass
            elif code == DO_ACTION:
                if timeline.frame_code(f"frame {frame}", start, end).stops:
                    self.stop_frames.append(frame)
            elif code == PLACE_OBJECT_2:
                timeline.placed_code(f"frame {frame}", start, end)
            if code in (PLACE_OBJECT, PLACE_OBJECT_2, PLACE_OBJECT_3):
                self.root_children.add(self.note_placement(code, start, f"frame {frame}"))
            elif code == DO_INIT_ACTION:
                sprite_id = struct.unpack_from("<H", body, start)[0]
                init = Actions()
                init.scan(body, start + 2, end)
                nested.merge(init)
                self.own_strings.setdefault(sprite_id, []).extend(init.strings)
                blocks.append((f"#initclip of clip #{sprite_id}", start + 2, end))
            elif code in (DEFINE_BUTTON, DEFINE_BUTTON_2):
                button_id = struct.unpack_from("<H", body, start)[0]
                find = button_1_actions if code == DEFINE_BUTTON else button_2_actions
                for block_start, block_end in find(body, start, end):
                    button = Actions()
                    button.scan(body, block_start, block_end)
                    nested.merge(button)
                    self.own_strings.setdefault(button_id, []).extend(button.strings)
                    self.button_jumps.setdefault(button_id, []).extend(button.own_frame_jumps)
                    blocks.append((f"button #{button_id}", block_start, block_end))
            elif code == DEFINE_SPRITE:
                sprite_id, sprite_frames = struct.unpack_from("<HH", body, start)
                sprite = Timeline(body, self.version, nested, nested, blocks)
                own = Actions()
                children = self.children.setdefault(sprite_id, set())
                labels, sub_frame = [], 1
                for sub_code, sub_start, sub_end in read_tags(body, start + 4, end):
                    if sub_code == SHOW_FRAME:
                        sub_frame += 1
                    elif sub_code == FRAME_LABEL:
                        labels.append((sub_frame, Reader(body, sub_start).cstring()))
                    elif sub_code == DO_ACTION:
                        own.merge(sprite.frame_code(f"clip #{sprite_id} frame {sub_frame}", sub_start, sub_end))
                    if sub_code == PLACE_OBJECT_2:
                        own.merge(sprite.placed_code(f"clip #{sprite_id} frame {sub_frame}", sub_start, sub_end))
                    if sub_code in (PLACE_OBJECT, PLACE_OBJECT_2, PLACE_OBJECT_3):
                        child = self.note_placement(sub_code, sub_start, f"clip #{sprite_id} frame {sub_frame}")
                        if child is not None:
                            children.add(child)
                self.own_strings.setdefault(sprite_id, []).extend(own.strings)
                self.sprites.append((sprite_id, sprite_frames, labels))
            elif code == EXPORT_ASSETS:
                reader = Reader(body, start)
                for _ in range(reader.u16()):
                    character_id = reader.u16()
                    self.exports[character_id] = reader.cstring()
            elif code in (IMPORT_ASSETS, IMPORT_ASSETS_2):
                reader = Reader(body, start)
                url = reader.cstring()
                if code == IMPORT_ASSETS_2:
                    reader.u16()  # Reserved
                names = []
                for _ in range(reader.u16()):
                    reader.u16()
                    names.append(reader.cstring())
                self.imports.append((url, names))
            elif code in (DO_ABC, DO_ABC_DEFINE):
                abc_start = start
                if code == DO_ABC:
                    abc_start = body.index(b"\0", start + 4) + 1  # Skip the flags and name
                self.abc += abc_strings(body, abc_start, end)
                self.abc_blocks.append(abc_start)
            elif code == SYMBOL_CLASS:
                reader = Reader(body, start)
                for _ in range(reader.u16()):
                    character_id = reader.u16()
                    self.classes.append(reader.cstring())
                    self.symbol_names[character_id] = self.classes[-1]
            elif code == DEFINE_BINARY_DATA and end - start > 14:
                if body[start + 6:start + 9] in (b"FWS", b"CWS", b"ZWS"):  # After the ID and 4 reserved bytes
                    self.embedded.append((struct.unpack_from("<H", body, start)[0], start + 6, end))

        # Like Flash, FrameLabel tags only count when there's no scene data.
        self.labels = scene_labels if scenes is not None else frame_labels
        self.scenes = scenes or []

    def note_placement(self, code, start, where):
        """The ID of the character that the PlaceObject tag at `start` places, if it names one,
        after noting where it's placed and its name."""
        try:
            character = placed_character(code, self.body, start)
            name = placed_name(code, self.body, start)
        except (IndexError, ValueError, struct.error):
            return None
        if character is not None:
            self.placements.setdefault(character, []).append(where)
            if name:
                self.instance_names.setdefault(character, []).append(name)
        return character

    def text(self):
        """The readable strings in the file, where file names would appear."""
        # Short names would turn up in binary data by chance, so only longer runs count.
        return "\n".join(re.findall(r"[\x20-\x7e]{4,}", self.body.decode("latin-1")))


def describe(path, display_name, other_names, dump, data=None):
    """Prints what the SWF file at `path`, or the SWF `data`, has and does."""
    if data is None:
        data = open(path, "rb").read()
    size = f"{len(data) / 1024:.0f} KB"
    if len(data) <= 8 or decompress(data) is None:
        print(f"== {display_name}  ({size}, not a SWF: {not_swf_kind(data)})")
        return
    m = Movie(data)
    main, nested, body = m.main, m.nested, m.body

    print(f"== {display_name}  ({size}, {m.signature} v{m.version}, {'AS3' if m.as3 else 'AS1/AS2'}, "
          f"{m.width}x{m.height} @ {m.frame_rate:g} fps, {m.frame_count} frames, {len(m.sprites)} sprites)")
    print("   scenes: " + (limited(m.scenes, lambda s: f"{s[0]}:{s[1]!r}") if m.scenes else "none"))
    print(f"   labels ({len(m.labels)}): " + (format_labels(m.labels) or "none"))
    print(f"   main timeline stops at frames ({len(m.stop_frames)}): " + (limited(m.stop_frames) or "none"))
    for title, items in (
        ("main timeline goes to", main.gotos),
        ("main timeline calls", main.calls),
        ("main timeline loads", main.loads),
        ("clips/buttons go to", nested.gotos),
        ("clips/buttons call", nested.calls),
        ("clips/buttons load", nested.loads),
    ):
        items = dedupe(items)
        if items:
            print(f"   {title} ({len(items)}): {limited(items)}")
    functions = dedupe(main.functions)
    if functions:
        print(f"   main timeline functions ({len(functions)}): {limited(functions)}")
    for url, names in m.imports:
        print(f"   imports from {url!r} ({len(names)}): {limited(names, repr)}")
    file_like = [s for s in dedupe(main.strings + nested.strings)
                 if len(s) <= 120 and not re.search("[<>\"']", s) and FILE_LIKE.search(s)]
    if file_like:
        print(f"   file names in code ({len(file_like)}): {limited(file_like, repr)}")
    fscommands = dedupe(s[len("fscommand:"):] for s in main.strings + nested.strings
                        if s.lower().startswith("fscommand:"))
    if fscommands:
        print(f"   fscommands in code ({len(fscommands)}): {limited(fscommands, repr)}")
    arrays = [a for a in main.arrays + nested.arrays if len(a[1]) >= 3]
    if arrays:
        print(f"   arrays in code ({len(arrays)}):")
        for name, values in arrays[:MAX_CLIP_LABELS]:
            print(f"      {name} ({len(values)}): {limited(values, repr, 20)}")
        if len(arrays) > MAX_CLIP_LABELS:
            print(f"      ... (+{len(arrays) - MAX_CLIP_LABELS} more)")

    clips = sorted((s for s in m.sprites if s[2] or s[1] >= LONG_TIMELINE), key=lambda s: -s[1])
    if clips:
        print(f"   clips with labels or {LONG_TIMELINE}+ frames ({len(clips)}):")
        for sprite_id, sprite_frames, sprite_labels in clips[:MAX_ITEMS]:
            name = f" {m.exports[sprite_id]!r}" if sprite_id in m.exports else ""
            label_text = f", labels {format_labels(sprite_labels, MAX_CLIP_LABELS)}" if sprite_labels else ""
            print(f"      #{sprite_id}{name}: {sprite_frames} frames{label_text}")
        if len(clips) > MAX_ITEMS:
            print(f"      ... (+{len(clips) - MAX_ITEMS} more)")
    if m.exports:
        print(f"   exported symbols ({len(m.exports)}): {limited(m.exports.values(), repr)}")

    if m.classes:
        print(f"   AS3 classes ({len(m.classes)}): {limited(dedupe(m.classes), repr)}")
    if m.abc:
        pattern = re.compile(r"(\.swf|Loader|URLRequest|load|goto|scene|/[\w.-])", re.I)
        file_names = {os.path.basename(name) for name in other_names}
        # Leave out qualified method names like "pkg:Class/pkg:method".
        method_name = re.compile(r"^[\w$.]*:[\w$]+/")
        interesting = [s for s in dedupe(m.abc) if 0 < len(s) <= 100 and not method_name.match(s) and (
            pattern.search(s) or any(name in s for name in file_names if len(name) > 1))]
        print(f"   AS3 strings about loading/navigation ({len(interesting)}): {limited(interesting, repr)}")

    text = m.text()
    mentioned = [name for name in other_names if name != display_name and
                 re.search(r"(?<![\w-])" + re.escape(os.path.basename(name)) + r"(?![\w-])", text)]
    if mentioned:
        print(f"   mentions other files ({len(mentioned)}): {limited(mentioned)}")

    if dump:
        print("   tags: " + ", ".join(f"{TAG_NAMES.get(code, code)} x{count}"
                                      for code, count in m.tag_counts.items()))
        for abc_start in m.abc_blocks:
            try:
                for line in Abc(body, abc_start).outline():
                    print("   " + line)
            except (IndexError, ValueError, struct.error) as e:
                print(f"   (couldn't read the AS3 classes: {type(e).__name__}: {e})")
        for title, start, end in m.blocks:
            if dump == "main" and not title.startswith("frame "):
                continue
            print(f"   -- {title} ({end - start} bytes)")
            for line in disassemble(body, start, end):
                print("      " + line)

    # Movies often embed others to load with Loader.loadBytes(), which are described too.
    for character_id, start, end in m.embedded:
        name = m.symbol_names.get(character_id, f"#{character_id}")
        describe(None, f"{display_name} > embedded {name}", other_names, dump, data=body[start:end])


# The variables the versions of the engine keep their scene lists in, as Ruffle's core
# looks for them (on the root timeline or in _global).
SCENE_LISTS = ("Qvar", "scenelist", "screens", "screens_list")
# The versions of the engine whose own scene lists Ruffle's scene menu reads, by a
# variable that only that version has, newest first. All of them keep the list in one of
# SCENE_LISTS, and switch scenes with a `setScreen` function.
ENGINE_VERSIONS = [
    ("Qmod", "Hitomi's"),
    ("Qfrm", "Mortal Combat's"),
    ("Qvar", "NC2's"),
    ("scenelist", "Olympic Contest's"),
    # Launchers with setScreen and a list with the names Evil Sorceress keeps in _global
    ("screens_list", "The Iron Giant 3's"),
    ("screens", "The Iron Giant 3's"),
]
# The lists of scenes that launchers which load their parts over themselves keep in
# _global, with the parts' setScreen switching scenes, by the version of the engine.
LAUNCHER_LISTS = {"screens": "Big Queen 1's", "screens_list": "Evil Sorceress's"}
SUPPORTED, LABELS_ONLY, JUMPS_ONLY, UNKNOWN, UNREADABLE = (
    "supported", "frame labels only", "only frame jumps supported", "unknown", "couldn't read")
STATUSES = (SUPPORTED, LABELS_ONLY, JUMPS_ONLY, UNKNOWN, UNREADABLE)


def game_scene_list(movie):
    """(where the code keeps it, names) of a game's own scene list, if the code has it."""
    for name, values in movie.main.arrays:
        if name in SCENE_LISTS:
            return f"an array in {name}, in the code of the main timeline", values
    for name, values in movie.nested.arrays:
        if name in ("Qvar", "scenelist"):
            return f"an array in {name}, set by the code of a clip", values
    # Some versions split a string of the scene names, which are exported symbols (which
    # Flash looks up ignoring case). Take the one naming the most.
    exported = {name.lower() for name in movie.exports.values()}
    best, best_count = None, 1
    for string in dedupe(movie.main.strings):
        names = string.split(",")
        count = sum(1 for name in names if name.lower() in exported)
        if len(names) > 1 and count > best_count:
            best, best_count = names, count
    if best:
        return "a string of names separated by commas, in the code of the main timeline", best
    clips = scene_clips(movie)
    if clips and has_set_screen(movie):
        return "the movie clips it exports for its scenes, as it has no list", clips
    return None


def has_function(movie, name):
    """Whether a movie's main timeline has a function, defined by that name, or under another
    one, as in protected games, and set to that name."""
    return name in movie.main.functions or name in movie.main.strings + movie.nested.strings


def has_set_screen(movie):
    """Whether a movie's main timeline has the engine's setScreen."""
    return has_function(movie, "setScreen")


def scene_clips(movie):
    """The export names of the movie clips of the scenes of a game whose version of the engine
    has no scene list, in the order Ruffle's menu lists them: the main menu, screen1, screen2...
    (or screen_1, screen_2...), then the others starting with screen_. There are none unless
    numbered ones are there."""
    sprites = {sprite_id for sprite_id, _, _ in movie.sprites}
    clips = []
    for character, name in movie.exports.items():
        lower = name.lower()
        if character not in sprites:
            continue
        if lower in ("mainmenu", "main_menu"):
            order = (0, 0)
        elif re.fullmatch(r"screen_?\d+", lower):  # screen1, or screen_1 as in Universal Soldier
            order = (1, int(lower[len("screen"):].lstrip("_")))
        elif lower.startswith("screen_"):
            order = (2, 0)
        else:
            continue
        clips.append((order, lower, name))
    if not any(order[0] == 1 for order, _, _ in clips):
        return []
    return [name for _, _, name in sorted(clips)]


def scene_count(movie):
    """How many scenes a game's own scene list has, if the code says."""
    scene_list = game_scene_list(movie)
    return len(scene_list[1]) if scene_list else None


def frame_jumps(movie):
    """The frames after the first that a movie's code jumps its main timeline to, which
    Ruffle's menu lists for games with no other way to navigate: from the code of the main
    timeline and of the buttons placed on it, and _root.gotoAndPlay(n) anywhere."""
    jumps = movie.main.own_frame_jumps + movie.main.root_frame_jumps + movie.nested.root_frame_jumps
    for character in movie.root_children:
        jumps += movie.button_jumps.get(character, [])
    return sorted({frame for frame in jumps if 1 < frame <= movie.frame_count})


def scene_method(movie):
    """How a movie handles its scenes, as (status, description)."""
    strings = set(movie.main.strings)
    if has_set_screen(movie) and strings & set(SCENE_LISTS):
        version = next(version for variable, version in ENGINE_VERSIONS if variable in strings)
        count = scene_count(movie)
        details = f", {count} scenes" if count else ""
        return SUPPORTED, f"its own scene list, in {version} version of its engine{details}"
    clips = scene_clips(movie)
    if has_set_screen(movie) and clips:
        return SUPPORTED, (f"its own scene clips, in Avatar The Last Airbender's version of its "
                           f"engine, {len(clips)} scenes")
    if len(movie.scenes) > 1:
        return SUPPORTED, f"{len(movie.scenes)} scenes on the main timeline"
    # Launchers that load all their parts at once, which report their scenes when loaded,
    # and srv_send_screen shows the part with the scene and calls its gotoScreen
    if "srv_send_screen" in movie.main.functions and "files" in movie.main.strings:
        parts = [values for name, values in movie.main.arrays if name == "files"]
        details = f" ({', '.join(map(str, parts[0]))})" if parts else ""
        return SUPPORTED, f"its parts' scene lists, in Metropolis Files' version of its engine{details}"
    if movie.labels:
        return LABELS_ONLY, f"no scenes, {len(movie.labels)} frame labels on the main timeline"
    jumps = frame_jumps(movie)
    if jumps:
        return JUMPS_ONLY, (f"no scenes or frame labels; the menu lists frame 1 and the {len(jumps)} "
                            f"frames its code jumps the main timeline to")

    hints = ["AS3" if movie.as3 else "AS1/AS2"]
    all_strings = movie.main.strings + movie.nested.strings + movie.abc
    if any(string in ("loadMovie", "loadMovieNum", "loadClip", "Loader", "URLRequest")
           or string.lower().endswith(".swf") for string in all_strings):
        hints.append("loads other files")
    if movie.embedded:
        hints.append(f"embeds {len(movie.embedded)} SWFs")
    labelled = sum(1 for _, _, labels in movie.sprites if labels)
    if labelled:
        hints.append(f"{labelled} movie clips with frame labels")
    return UNKNOWN, f"unknown method for handling scenes in this game ({'; '.join(hints)})"


def describe_scenes(path):
    """Prints the scenes of a game that keeps its own list of them, and the code that shows them."""
    name = os.path.basename(path)
    data = open(path, "rb").read()
    if len(data) <= 8 or decompress(data) is None:
        print(f"== {name}: not a SWF ({not_swf_kind(data)})")
        return
    m = Movie(data)
    status, description = scene_method(m)
    if status == UNKNOWN:
        status, description = launcher_method(path, m) or (status, description)
    print(f"== {name}: [{status}] {description}")
    if status == JUMPS_ONLY:
        print("   frames its code jumps the main timeline to: " + limited(frame_jumps(m)))
    scene_list = game_scene_list(m)
    if scene_list:
        where, names = scene_list
        print(f"   the list ({len(names)} scenes) is {where}")
        by_export = {export.lower(): character for character, export in m.exports.items()}
        if not any(str(scene).lower() in by_export for scene in names):
            print("   none of them are movie clips of this file, so they're probably in the parts "
                  "it loads: " + limited(names, repr))
            names = []
        sprites = {sprite_id: (frames, labels) for sprite_id, frames, labels in m.sprites}
        # Names that tell how a scene's code works with the main timeline: its functions,
        # and the engine's variables.
        functions = set(m.main.functions)
        known = re.compile(r"^(Q[a-z]{2,4}|screen_name|pos|scenelist|scenepart)$")

        def uses(strings):
            return dedupe(s for s in strings if s in functions or known.match(s))

        for number, scene in enumerate(names, 1):
            character = by_export.get(str(scene).lower())
            if character is None:
                print(f"   {number}. {scene!r}: no clip of this name in this file")
                continue
            if character not in sprites:
                print(f"   {number}. {scene!r}: #{character}, which isn't a movie clip")
                continue
            frames, labels = sprites[character]
            label_text = f", labels {format_labels(labels, MAX_CLIP_LABELS)}" if labels else ""
            print(f"   {number}. {scene!r}: clip #{character}, {frames} frames{label_text}")
            own = uses(m.own_strings.get(character, []))
            if own:
                print(f"         its code uses {', '.join(own)}")
            inside, seen, queue = [], {character}, list(m.children.get(character, ()))
            while queue:
                child = queue.pop()
                if child in seen:
                    continue
                seen.add(child)
                inside += m.own_strings.get(child, [])
                queue += m.children.get(child, ())
            inside = uses(inside)
            if inside:
                print(f"         code of the clips and buttons in it uses {', '.join(inside)}")
    for title, start, end in m.blocks:
        if title.startswith("frame "):
            print(f"   -- {title} ({end - start} bytes)")
            for line in disassemble(m.body, start, end):
                print("      " + line)


MAX_PICKING_BLOCKS = 30
MAX_SETS = 12


def describe_variables(path):
    """Prints the code that picks the frame of a timeline with a variable, like the version
    of a character a game shows once it's been picked, and where the game sets those
    variables. A scene opened from the scene menu skips the code that sets them, which
    leaves such clips playing through all of their frames."""
    name = os.path.basename(path)
    data = open(path, "rb").read()
    if len(data) <= 8 or decompress(data) is None:
        print(f"== {name}: not a SWF ({not_swf_kind(data)})")
        return
    m = Movie(data)
    if m.as3:
        print(f"== {name}: AS3, whose code this doesn't read")
        return
    sprites = {sprite_id: frames for sprite_id, frames, _ in m.sprites}

    def about(character):
        """What a character is, and where it's placed."""
        details = []
        if character in m.exports:
            details.append(f"exported as {m.exports[character]!r}")
        if character in sprites:
            details.append(f"{sprites[character]} frames")
        names = dedupe(m.instance_names.get(character, []))
        if names:
            details.append(f"named {limited(names, repr, 5)}")
        places = dedupe(m.placements.get(character, []))
        if places:
            details.append(f"placed on {limited(places, str, 5)}")
        return ", ".join(details)

    blocks = []
    for title, start, end in m.blocks:
        try:
            blocks.append((title, statements(m.body, start, end)))
        except Exception as e:  # One odd block shouldn't hide the rest
            print(f"   (couldn't read the code of {title}: {type(e).__name__}: {e})")
    picking, read = [], set()
    for title, code in blocks:
        # Within a clip, a jump to a frame that depends on a condition picks it too.
        in_clip = not title.startswith("button ") and not re.match(r"frame \d+$", title)
        conditions, picks = set(), []
        for index, statement in enumerate(code):
            if statement.kind == "if":
                conditions |= statement.reads
            elif statement.kind == "goto":
                if statement.frame is None or statement.frame.constant is None:
                    picks.append(index)
                    read |= statement.reads
                elif in_clip and conditions:
                    picks.append(index)
        if picks:
            if in_clip:
                read |= conditions
            picking.append((title, code, picks))
    read = {variable for variable in read if variable not in PROPERTIES}
    print(f"== {name}: {len(picking)} blocks of code pick a frame with a variable or a condition")
    for title, code, picks in picking[:MAX_PICKING_BLOCKS]:
        character = re.search(r"clip #(\d+)", title)
        details = about(int(character.group(1))) if character else ""
        button = re.match(r"button #(\d+)", title)
        if button:
            details = about(int(button.group(1)))
        print(f"   -- {title}" + (f" ({details})" if details else ""))
        # Long blocks are cut down to what comes just before the jumps.
        shown = set(range(len(code))) if len(code) <= 40 else \
            {line for pick in picks for line in range(pick - 10, pick + 3)}
        last = -1
        for index, statement in enumerate(code):
            if index in shown:
                if index > last + 1:
                    print("      ...")
                print(f"      {statement.offset:04x}  {statement.text}")
                last = index
        if last < len(code) - 1:
            print("      ...")
    if len(picking) > MAX_PICKING_BLOCKS:
        print(f"   ... (+{len(picking) - MAX_PICKING_BLOCKS} more blocks)")
    def where(title, statement):
        button = re.match(r"button #(\d+)", title)
        places = dedupe(m.placements.get(int(button.group(1)), [])) if button else []
        text = title + (f" (placed on {limited(places, str, 3)})" if places else "")
        text += f" @{statement.offset:04x}"
        if statement.function:
            text += f", in function {statement.function}"
        return text

    if read:
        print(f"   where the variables they read are set ({len(read)}):")
    for variable in sorted(read, key=str.lower):
        sets = [(title, statement) for title, code in blocks for statement in code
                if statement.kind == "set" and statement.sets
                and statement.sets.lower() == variable.lower()]
        if not sets:
            print(f"      {variable}: not set by this file's code")
            continue
        print(f"      {variable} ({len(sets)}):")
        for title, statement in sets[:MAX_SETS]:
            print(f"         {where(title, statement)}: {statement.text.strip()}")
        if len(sets) > MAX_SETS:
            print(f"         ... (+{len(sets) - MAX_SETS} more)")

    # Clips that other code moves between frames, or shows and hides, like a character's
    # versions picked by buttons, which a scene opened from the menu skips too.
    by_name = {}
    for character, names in m.instance_names.items():
        for instance in names:
            by_name.setdefault(instance.lower(), set()).add(character)
    own_stops = {}
    for title, code in blocks:
        frame = re.match(r"clip #(\d+) frame (\d+)$", title)
        if frame and any(statement.target is None and statement.text.strip() == "stop()"
                         for statement in code):
            own_stops.setdefault(int(frame.group(1)), []).append(int(frame.group(2)))
    moved = {}
    for title, code in blocks:
        for statement in code:
            if statement.target is None:
                continue
            leaf = path_leaf(statement.target.strip("'\""))
            if leaf:
                moved.setdefault(leaf.lower(), []).append((title, statement))

    def frames_of(leaf):
        return max((sprites.get(character, 0) for character in by_name.get(leaf, ())), default=0)

    shown = sorted(moved, key=lambda leaf: (-frames_of(leaf), leaf))
    if shown:
        print(f"   clips that other code moves between frames or shows and hides ({len(shown)}):")
    for leaf in shown[:MAX_PICKING_BLOCKS]:
        characters = sorted(by_name.get(leaf, ()))
        if characters:
            details = []
            for character in characters[:3]:
                stops = own_stops.get(character)
                stopping = f"stops itself on frames {limited(sorted(set(stops)), str, 8)}" \
                    if stops else "never stops itself"
                details.append(f"clip #{character}, {sprites.get(character, '?')} frames, {stopping}")
            about_leaf = "; ".join(details)
        else:
            about_leaf = "no clip is placed with this name (attached by code, or a part's?)"
        statements_of = moved[leaf]
        print(f"      {leaf!r}: {about_leaf}")
        for title, statement in statements_of[:MAX_SETS]:
            text = statement.text.strip()
            if not text.startswith(statement.target) and not text.startswith("setProperty"):
                text = f"(in tellTarget({statement.target!r})) {text}"
            print(f"         {where(title, statement)}: {text}")
        if len(statements_of) > MAX_SETS:
            print(f"         ... (+{len(statements_of) - MAX_SETS} more)")
    if len(shown) > MAX_PICKING_BLOCKS:
        print(f"      ... (+{len(shown) - MAX_PICKING_BLOCKS} more)")


def launcher_method(path, movie):
    """(status, description) of a launcher, from the parts it names (that it loads over
    itself): supported when it keeps its scene list in _global and a part has the code that
    switches scenes, like Big Queen 1, or as a part's own code handles its scenes, like
    Matsumoto vs. Tsunade's. None when no part tells."""
    lists = [(name, values) for name, values in movie.main.arrays if name in LAUNCHER_LISTS]
    folder = os.path.dirname(os.path.abspath(path))
    found = None
    read = 0
    # Parts are named like "parts/btmq0", or "parts/esrm" with the number added in code.
    for string in dedupe(movie.main.strings):
        if not re.fullmatch(r"[\w.-]+(/[\w.-]+)+", string):
            continue
        directory, prefix = os.path.split(os.path.join(folder, *string.split("/")))
        try:
            names = sorted(os.listdir(directory))
        except OSError:
            continue
        for part in [n for n in names if n.lower().startswith(prefix.lower())][:3]:
            if read >= 6:  # Enough to tell, for launchers that name many files
                return found
            read += 1
            try:
                data = open(os.path.join(directory, part), "rb").read()
                part_movie = Movie(data) if len(data) > 8 and decompress(data) else None
            except Exception:  # A part that can't be read doesn't make the game unreadable
                continue
            if not part_movie:
                continue
            if lists and has_set_screen(part_movie):
                name, scenes = lists[0]
                return SUPPORTED, (f"its own scene list, in {LAUNCHER_LISTS[name]} version of its "
                                   f"engine, {len(scenes)} scenes, switched by its parts")
            status, description = scene_method(part_movie)
            if status == SUPPORTED:
                return status, f"{description}, in its part {part}"
            if status != UNKNOWN and not found:
                found = status, f"{description}, in its part {part}"
    return found


def classify(args, unsupported_only=False, names_only=False, max_depth=None):
    """Prints how each game in the given folders handles its scenes, and a tally. With
    `unsupported_only`, the supported games are left out of the list. With `names_only`,
    it prints only the paths of the games that aren't supported. With `max_depth`, only
    the SWFs that many folders down or less are looked at (1 for direct subfolders)."""
    by_folder = {}  # Folder -> (folder to show it relative to, SWFs in it)
    for arg in args:
        base = os.path.dirname(os.path.abspath(arg))
        if os.path.isdir(arg):
            for root, dirs, names in os.walk(arg):
                depth = len(os.path.relpath(root, arg).split(os.sep)) if root != arg else 0
                if max_depth is not None and depth >= max_depth:
                    dirs.clear()
                dirs.sort()
                swfs = [os.path.join(root, name) for name in sorted(names)
                        if name.lower().endswith(".swf")]
                if swfs:
                    by_folder[os.path.abspath(root)] = (base, swfs)
        else:
            by_folder.setdefault(os.path.dirname(os.path.abspath(arg)), (base, []))[1].append(arg)

    statuses, details, unknown, part_count = {}, {}, [], 0
    linked_folders = {}  # Folder -> names of the folders its SWFs name, like "parts"
    for folder, (base, folder_paths) in by_folder.items():
        # SWFs in a subfolder that a SWF above it names, like "parts/", are its parts.
        ancestors = [other for other in linked_folders if folder.startswith(other + os.sep)]
        if ancestors:
            nearest = max(ancestors, key=len)
            subfolder = os.path.relpath(folder, nearest).split(os.sep)[0]
            if subfolder.lower() in linked_folders[nearest]:
                part_count += len(folder_paths)
                linked_folders[folder] = linked_folders[nearest]
                continue
        movies = {}
        for path in folder_paths:
            try:
                data = open(path, "rb").read()
                movies[path] = Movie(data) if len(data) > 8 and decompress(data) else None
            except Exception as e:  # Keep going, so one odd file doesn't hide the rest
                movies[path] = e
        texts = {path: movie.text() for path, movie in movies.items() if isinstance(movie, Movie)}
        linked_folders[folder] = {name.lower() for text in texts.values()
                                  for name in re.findall(r"([\w .-]+)[/\\]", text)}
        # So are SWFs that another SWF next to them names, as it loads them.
        names = {os.path.basename(path): path for path in folder_paths}
        parts = set()
        for path, text in texts.items():
            if len(names) > 1:
                parts |= {other for name, other in names.items() if other != path and
                          re.search(r"(?<![\w-])" + re.escape(name) + r"(?![\w-])", text)}
        part_count += len(parts)

        lines = []
        for path, movie in movies.items():
            if path in parts:
                continue
            name = os.path.basename(path)
            if isinstance(movie, Movie):
                status, description = scene_method(movie)
                if status == UNKNOWN:
                    status, description = launcher_method(path, movie) or (status, description)
            elif movie is None:
                status, description = UNREADABLE, "not a SWF"
            else:
                status, description = UNREADABLE, f"{type(movie).__name__}: {movie}"
            statuses[status] = statuses.get(status, 0) + 1
            if status == SUPPORTED:
                version = re.search(r"in (.*) version", description)
                key = version.group(1) if version else "scenes on the main timeline"
                details[key] = details.get(key, 0) + 1
            elif status == UNKNOWN:
                shown = os.path.relpath(path, base).replace(os.sep, "/")
                unknown.append(f"{shown} {description[description.index('('):]}")
            if names_only:
                if status != SUPPORTED:
                    print(path)
            elif not (unsupported_only and status == SUPPORTED):
                lines.append(f"   [{status}] {name}: {description}")
        if lines:
            print(f"== {os.path.relpath(folder, base)}")
            print("\n".join(lines))

    if names_only:
        return 0
    total = sum(statuses.values())
    print(f"== {total} games: " + ", ".join(f"{statuses.get(status, 0)} {status}" for status in STATUSES))
    if details:
        print("   supported, by kind: " + ", ".join(f"{key} {count}" for key, count in details.items()))
    if unknown and not unsupported_only:  # With unsupported_only, the list above has them
        print("   unknown: " + "; ".join(unknown))
    if part_count:
        were = "SWF was" if part_count == 1 else "SWFs were"
        print(f"   ({part_count} {were} taken for parts of these games and left out: those in "
              f"a subfolder that a game names, like parts/, and those another SWF next to them names)")
    return 0


def main(args):
    dump = "main" if "--dump-main" in args else "all" if "--dump" in args else None
    names_only = "--unsupported-names" in args
    max_depth = 1 if "--onefolderdeep" in args else None
    unsupported_only = "--unsupported" in args or names_only
    classifying, scenes = "--classify" in args or unsupported_only, "--scenes" in args
    variables = "--variables" in args
    modes = ("--dump", "--dump-main", "--classify", "--unsupported", "--unsupported-names", "--scenes",
             "--variables", "--onefolderdeep")
    unknown_modes = [arg for arg in args if arg.startswith("--") and arg not in modes]
    if unknown_modes:  # It would be taken for a file name, and everything else described
        print(f"Unknown option {unknown_modes[0]}; this version has: {', '.join(modes)}")
        return 1
    args = [arg for arg in args if arg not in modes]
    expanded = []
    for arg in args:
        matches = sorted(glob.glob(arg)) if "*" in arg or "?" in arg else []
        expanded += matches or [arg]
    args = expanded
    if not args:
        print(__doc__)
        return 1
    try:  # Don't fail on names in other alphabets when the output goes to a file
        sys.stdout.reconfigure(encoding="utf-8", errors="backslashreplace")
    except AttributeError:
        pass
    if classifying:
        return classify(args, unsupported_only, names_only, max_depth)
    if scenes or variables:
        for path in args:
            try:
                (describe_scenes if scenes else describe_variables)(path)
            except Exception as e:  # Keep going, so one odd file doesn't hide the rest
                print(f"== {os.path.basename(path)}  (couldn't read: {type(e).__name__}: {e})")
        return 0
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
            describe(path, name, names, dump)
        except Exception as e:  # Keep going, so one odd file doesn't hide the rest
            print(f"== {name}  (couldn't read: {type(e).__name__}: {e})")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
