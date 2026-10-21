"""A small AVM1 assembler for building test movies."""
import struct


class Asm:
    def __init__(self):
        self.items = []

    def op(self, code, body=b""):
        if code < 0x80:
            self.items.append(bytes([code]))
        else:
            self.items.append(bytes([code]) + struct.pack("<H", len(body)) + body)
        return self

    def push(self, *values):
        body = b""
        for value in values:
            if value is None:
                body += b"\x03"
            elif value is True or value is False:
                body += b"\x05" + bytes([int(value)])
            elif isinstance(value, int):
                body += b"\x07" + struct.pack("<i", value)
            else:
                body += b"\x00" + value.encode() + b"\x00"
        return self.op(0x96, body)

    def label(self, name):
        self.items.append(("label", name))
        return self

    def jump(self, name):
        self.items.append(("jump", 0x99, name))
        return self

    def if_(self, name):
        self.items.append(("jump", 0x9D, name))
        return self

    def function(self, name, params, body, v2=False):
        code = body.assemble()
        if v2:
            raise NotImplementedError
        header = name.encode() + b"\0" + struct.pack("<H", len(params))
        header += b"".join(p.encode() + b"\0" for p in params) + struct.pack("<H", len(code))
        self.op(0x9B, header)
        self.items.append(code)
        return self

    def assemble(self):
        offsets, pos = {}, 0
        for item in self.items:
            if isinstance(item, tuple):
                if item[0] == "label":
                    offsets[item[1]] = pos
                else:
                    pos += 5
            else:
                pos += len(item)
        out, pos = b"", 0
        for item in self.items:
            if isinstance(item, tuple):
                if item[0] == "label":
                    continue
                pos += 5
                out += bytes([item[1]]) + struct.pack("<Hh", 2, offsets[item[2]] - pos)
            else:
                out += item
                pos += len(item)
        return out


GET_VARIABLE, SET_VARIABLE, GET_MEMBER, SET_MEMBER, CALL_FUNCTION, CALL_METHOD = 0x1C, 0x1D, 0x4E, 0x4F, 0x3D, 0x52
DEFINE_LOCAL, DEFINE_LOCAL2, TO_NUMBER, GREATER, LESS2, EQUALS2, NOT = 0x3C, 0x41, 0x4A, 0x67, 0x48, 0x49, 0x12
ADD2, SUBTRACT, INCREMENT, RETURN, POP, STOP, TRACE, REMOVE_SPRITE = 0x47, 0x0B, 0x50, 0x3E, 0x17, 0x07, 0x26, 0x25
GET_URL2 = 0x9A


def tag(code, body=b""):
    if len(body) < 63 and code != 39:
        return struct.pack("<H", code << 6 | len(body)) + body
    return struct.pack("<HI", code << 6 | 63, len(body)) + body


def rect(xmin, xmax, ymin, ymax, bits=16):
    fields = format(bits, "05b") + "".join(format(v, f"0{bits}b") for v in (xmin, xmax, ymin, ymax))
    fields += "0" * (-len(fields) % 8)
    return int(fields, 2).to_bytes(len(fields) // 8, "big")


def swf(version, frame_count, tags):
    body = rect(0, 640 * 20, 0, 480 * 20) + struct.pack("<BBH", 0, 24, frame_count) + b"".join(tags) + tag(0)
    return b"FWS" + bytes([version]) + struct.pack("<I", 8 + len(body)) + body
