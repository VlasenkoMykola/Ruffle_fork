"""A movie with the scene code of Mortal Combat's version of the engine, as its dump shows."""
import struct
import sys
from asm import *

# function getposition(src) { i = Number(src); if (i > 0) return i;
#   i = 0; while (i < Qfrm) { if (Qvar[i] == src) return i + 1; i++; } }
getposition = (Asm()
    .push("i", "src").op(GET_VARIABLE).op(TO_NUMBER).op(SET_VARIABLE)
    .push("i").op(GET_VARIABLE).push(0).op(GREATER).op(NOT).if_("search")
    .push("i").op(GET_VARIABLE).op(RETURN)
    .label("search").push("i", 0).op(SET_VARIABLE)
    .label("loop").push("i").op(GET_VARIABLE).push("Qfrm").op(GET_VARIABLE).op(LESS2).op(NOT).if_("end")
    .push("Qvar").op(GET_VARIABLE).push("i").op(GET_VARIABLE).op(GET_MEMBER)
    .push("src").op(GET_VARIABLE).op(EQUALS2).op(NOT).if_("next")
    .push("i").op(GET_VARIABLE).push(1).op(ADD2).op(RETURN)
    .label("next").push("i", "i").op(GET_VARIABLE).op(INCREMENT).op(SET_VARIABLE).jump("loop")
    .label("end"))

# function setScreen(x) { x = getposition(x); getURL("fscommand:" + "bar", x);
#   if (game.screen != undefined) removeMovieClip(game.screen);
#   game.attachMovie(Qvar[x - 1], "screen", 0); screen_name = Qvar[x - 1]; }
set_screen = (Asm()
    .push("x", "x").op(GET_VARIABLE).push(1, "getposition").op(CALL_FUNCTION).op(SET_VARIABLE)
    .push("fscommand:", "bar").op(ADD2).push("x").op(GET_VARIABLE).op(GET_URL2, b"\x00")
    .push("game").op(GET_VARIABLE).push("screen").op(GET_MEMBER).push(None).op(EQUALS2).op(NOT).op(NOT).if_("attach")
    .push("game").op(GET_VARIABLE).push("screen").op(GET_MEMBER).op(REMOVE_SPRITE)
    .label("attach").push(0, "screen", "Qvar").op(GET_VARIABLE).push("x").op(GET_VARIABLE).push(1).op(SUBTRACT).op(GET_MEMBER)
    .push(3, "game").op(GET_VARIABLE).push("attachMovie").op(CALL_METHOD).op(POP)
    .push("screen_name", "Qvar").op(GET_VARIABLE).push("x").op(GET_VARIABLE).push(1).op(SUBTRACT).op(GET_MEMBER).op(SET_VARIABLE))

frame1 = (Asm()
    .function("getposition", ["src"], getposition)
    .function("setScreen", ["x"], set_screen)
    .op(STOP)
    .push("Qscr").op(GET_VARIABLE).push(None).op(EQUALS2).op(NOT).if_("show")
    .push("screen_name").op(DEFINE_LOCAL2)
    .push("Qstr", "mainmenu,screen1,screen2").op(SET_VARIABLE)
    .push("Qvar", ",", 1, "Qstr").op(GET_VARIABLE).push("split").op(CALL_METHOD).op(SET_VARIABLE)
    .push("Qfrm", "Qvar").op(GET_VARIABLE).push("length").op(GET_MEMBER).op(SET_VARIABLE)
    .push("fscommand:", "mnf").op(ADD2).push("Qfrm").op(GET_VARIABLE).op(GET_URL2, b"\x00")
    .push("Qscr", 1).op(SET_VARIABLE)
    .label("show")
    .push("Qscr").op(GET_VARIABLE).push(1, "setScreen").op(CALL_FUNCTION).op(POP)
    .assemble() + b"\0")


def sprite(character_id, code=b""):
    tags = (tag(12, code) if code else b"") + tag(1) + tag(0)
    return tag(39, struct.pack("<HH", character_id, 1) + tags)


def shown(name):
    return Asm().push("shown " + name).op(TRACE).assemble() + b"\0"


names = ["mainmenu", "screen1", "screen2"]
tags = [tag(69, struct.pack("<I", 0)), sprite(1)]
tags += [sprite(2 + i, shown(name)) for i, name in enumerate(names)]
exports = struct.pack("<H", len(names)) + b"".join(struct.pack("<H", 2 + i) + n.encode() + b"\0" for i, n in enumerate(names))
tags.append(tag(56, exports))
# PlaceObject2: has name and character, depth 1, character 1, name "game"
tags.append(tag(26, bytes([0x22]) + struct.pack("<HH", 1, 1) + b"game\0"))
tags += [tag(12, frame1), tag(1), tag(1)]
sys.stdout.buffer.write(swf(7, 2, tags))
