#!/usr/bin/env python3
"""Copies SWF games to an Android device with adb, skipping the ones already there.

Usage: python push_games.py [--dest <folder on the device>] [--serial <device>]
                            [--adb <path to adb>] [--dry-run] [<folder>]

Each direct subfolder of <folder> (the current folder by default) that has a .swf file
in it is a game, and is copied whole, with its parts, to a folder of the same name in
the device's games folder (/sdcard/SWF_Games by default). Games whose folder is already
there, whatever its case, are skipped.

A game is copied under a temporary name, <name>.partial, which gets its real name once
the copy is complete, so a copy that was cut off is copied again on the next run
instead of being taken for a game already there.

--dry-run only lists what would be copied. --serial picks a device when more than one
is connected (`adb devices` lists them). --adb gives the path to adb when it isn't on
the PATH; it comes with Android's platform tools:
https://developer.android.com/tools/releases/platform-tools

Only the Python standard library is needed.
"""

import os
import shutil
import subprocess
import sys

PARTIAL = ".partial"


def quote(text):
    """`text` quoted for the device's shell."""
    return "'" + text.replace("'", "'\\''") + "'"


class Device:
    def __init__(self, adb, serial):
        self.adb = [adb] + (["-s", serial] if serial else [])

    def run(self, *args, check=True):
        result = subprocess.run(self.adb + list(args), capture_output=True)
        output = (result.stdout + result.stderr).decode("utf-8", "replace").replace("\r", "")
        if check and result.returncode != 0:
            raise RuntimeError(f"adb {' '.join(args)} failed: {output.strip()}")
        return result.returncode, output

    def shell(self, command, check=True):
        return self.run("shell", command, check=check)

    def folders(self, path):
        """The names of the folders in `path` on the device."""
        # ls -p marks folders with a slash, which a name can't have.
        status, output = self.shell(f"ls -1p {quote(path)}", check=False)
        if status != 0:
            return []
        return [line[:-1] for line in output.split("\n") if line.endswith("/")]


def find_adb(given):
    if given:
        return given
    found = shutil.which("adb")
    if found:
        return found
    # Where Android Studio installs the platform tools on Windows
    local = os.path.join(os.environ.get("LOCALAPPDATA", ""), "Android", "Sdk", "platform-tools", "adb.exe")
    return local if os.path.isfile(local) else None


def games(folder):
    """(name, path, size in bytes) of each direct subfolder of `folder` with a .swf file in it."""
    found = []
    for name in sorted(os.listdir(folder), key=str.lower):
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
            found.append((name, path, size))
    return found


def games_count(count):
    return f"{count} game" + ("" if count == 1 else "s")


def megabytes(size):
    return f"{size / 1024 / 1024:.1f} MB"


def main(args):
    options = {"--dest": "/sdcard/SWF_Games", "--serial": None, "--adb": None}
    dry_run = False
    folders = []
    args = list(args)
    while args:
        arg = args.pop(0)
        if arg in options:
            if not args:
                print(f"{arg} needs a value")
                return 1
            options[arg] = args.pop(0)
        elif arg == "--dry-run":
            dry_run = True
        elif arg.startswith("--"):
            print(f"Unknown option {arg}")
            print(__doc__)
            return 1
        else:
            folders.append(arg)
    if len(folders) > 1:
        print(__doc__)
        return 1
    folder = folders[0] if folders else "."
    dest = options["--dest"].rstrip("/")
    try:  # Don't fail on names in other alphabets when the output goes to a file
        sys.stdout.reconfigure(encoding="utf-8", errors="backslashreplace")
    except AttributeError:
        pass

    local_games = games(folder)
    if not local_games:
        print(f"No subfolder of {os.path.abspath(folder)} has a .swf file in it.")
        return 1

    adb = find_adb(options["--adb"])
    if not adb:
        print("adb wasn't found. Install Android's platform tools, from "
              "https://developer.android.com/tools/releases/platform-tools, and put the folder "
              "on the PATH, or give the path to adb with --adb.")
        return 1
    device = Device(adb, options["--serial"])
    try:
        _, output = device.run("devices")
        connected = [line.split("\t") for line in output.split("\n")[1:] if "\t" in line]
        ready = [serial for serial, state in connected if state == "device"]
        if options["--serial"] is None:
            if not ready:
                states = ", ".join(f"{serial} ({state})" for serial, state in connected)
                print("No device is ready" + (f": {states}." if states else ".") +
                      " Connect one with USB debugging on, and allow this computer on it.")
                return 1
            if len(ready) > 1:
                print(f"More than one device is connected ({', '.join(ready)}); pick one with --serial.")
                return 1
        device.shell(f"mkdir -p {quote(dest)}")
        on_device = device.folders(dest)
    except (OSError, RuntimeError) as e:
        print(e)
        return 1

    existing = {name.casefold() for name in on_device if not name.endswith(PARTIAL)}
    leftovers = {name.casefold(): name for name in on_device if name.endswith(PARTIAL)}
    new = [game for game in local_games if game[0].casefold() not in existing]
    skipped = len(local_games) - len(new)
    total = sum(size for _, _, size in new)
    print(f"{len(local_games)} games in {os.path.abspath(folder)}: {skipped} already in {dest}, "
          f"{len(new)} to copy ({megabytes(total)}).")
    if dry_run:
        for name, _, size in new:
            print(f"   would copy {name} ({megabytes(size)})")
        return 0

    copied, failed, done = 0, [], 0
    for number, (name, path, size) in enumerate(new, 1):
        print(f"[{number}/{len(new)}] {name} ({megabytes(size)}, {megabytes(done)} of "
              f"{megabytes(total)} done)", flush=True)
        target = f"{dest}/{name}"
        partial = target + PARTIAL
        try:
            # A copy that was cut off is started over.
            leftover = leftovers.get((name + PARTIAL).casefold())
            device.shell(f"rm -rf {quote(dest + '/' + (leftover or name + PARTIAL))}")
            device.run("push", path, partial)
            device.shell(f"mv {quote(partial)} {quote(target)}")
            copied += 1
        except (OSError, RuntimeError) as e:
            print(f"   couldn't copy it: {e}")
            device.shell(f"rm -rf {quote(partial)}", check=False)
            failed.append(name)
        done += size
    print(f"Copied {games_count(copied)} to {dest}; {skipped} {'was' if skipped == 1 else 'were'} "
          f"already there.")
    if failed:
        print(f"{len(failed)} couldn't be copied, and will be tried again on the next run: "
              + ", ".join(failed))
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
