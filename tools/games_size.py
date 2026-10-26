#!/usr/bin/env python3
"""Adds up the size of the SWF games in a folder, to see how much space copying them needs.

Usage: python games_size.py [<folder>]

Each direct subfolder of <folder> (the current folder by default) that has a .swf file
in it is a game, as push_games.py counts them, and its size is that of everything in it,
parts included. The games are listed largest first, then the total.

push_games.py --dry-run gives the size of only the games not on the device yet.

Only the Python standard library is needed.
"""

import os
import sys


def games(folder):
    """(name, size in bytes) of each direct subfolder of `folder` with a .swf file in it."""
    found = []
    for name in os.listdir(folder):
        path = os.path.join(folder, name)
        if not os.path.isdir(path):
            continue
        try:
            has_swf = any(entry.lower().endswith(".swf") and os.path.isfile(os.path.join(path, entry))
                          for entry in os.listdir(path))
        except OSError:
            continue
        if has_swf:
            size = sum(os.path.getsize(os.path.join(root, file))
                       for root, _, files in os.walk(path) for file in files)
            found.append((name, size))
    return found


def readable(size):
    for unit in ("bytes", "KB", "MB"):
        if size < 1024:
            return f"{size:.0f} {unit}" if unit == "bytes" else f"{size:.1f} {unit}"
        size /= 1024
    return f"{size:.2f} GB"


def main(args):
    if len(args) > 1 or any(arg.startswith("-") for arg in args):
        print(__doc__)
        return 1
    folder = args[0] if args else "."
    try:  # Don't fail on names in other alphabets when the output goes to a file
        sys.stdout.reconfigure(encoding="utf-8", errors="backslashreplace")
    except AttributeError:
        pass
    found = sorted(games(folder), key=lambda game: -game[1])
    if not found:
        print(f"No subfolder of {os.path.abspath(folder)} has a .swf file in it.")
        return 1
    for name, size in found:
        print(f"{readable(size):>10}  {name}")
    total = sum(size for _, size in found)
    print(f"{len(found)} games in {os.path.abspath(folder)}: {readable(total)} in all")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
