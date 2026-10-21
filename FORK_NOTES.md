# Notes on this fork

This fork of Ruffle adds a way to jump between the scenes of a movie, without playing
through it. It's for personal use: user experience comes before keeping permissions
minimal. These notes keep what was learned while making it.

## Repositories and branches

- **This repository**, `VlasenkoMykola/Ruffle_fork`, branch `claude/relaxed-gauss-ymscnb`,
  which is also its default branch. It has the core API, the desktop and web menus, the
  Windows build, and `tools/swfinfo.py`.
- **The Android app**, `VlasenkoMykola/ruffle-android-with-sceneselect`, branch `scene-menu`
  (its `main` is upstream's). Its `Cargo.toml` takes the Ruffle crates from this
  repository's branch, and its `Cargo.lock` pins the commit: after pushing core changes
  here, run `cargo update -p ruffle_core` there and commit the lock file. Its own notes
  are in its `FORK_NOTES.md`.

## Builds and releases

- **Windows**: `.github/workflows/build_windows.yml` builds `ruffle_desktop` for Windows on
  every push to the branch (`--release --features jpegxr`, with the C runtime linked
  statically), and replaces the pre-release `latest-windows-build`, whose zip is at
  https://github.com/VlasenkoMykola/Ruffle_fork/releases/download/latest-windows-build/ruffle-windows-x86_64.zip.
  A new push cancels a build in progress.
- **Android**: its `Build` workflow only runs when dispatched (or for `main`), e.g. with the
  GitHub MCP tool `actions_run_trigger` (`run_workflow`, `workflow_id` `build.yml`, `ref`
  `scene-menu`); dispatch `lint.yml` the same way. Once the APKs are built and the emulator
  tests (API 26 and 36) pass, its `Publish` job (`publish_build.yml`, through
  `workflow_call`) replaces the pre-release `latest-build`:
  https://github.com/VlasenkoMykola/ruffle-android-with-sceneselect/releases/tag/latest-build.
  Its notes name the commit it was built from, which is how to tell builds apart. Each APK
  is also its own unzipped artifact of the run, since the user's connection is slow and they
  only need `app-arm64-v8a-release.apk`.
- Workflow artifacts are unreliable to download for the user, hence the releases.
- From the cloud sessions this was made in: pushing tags is refused (only branches can be
  pushed); `dl.google.com`, Termux mirrors and the artifact storage are blocked, so Android
  builds can't be made locally (`cargo check --target x86_64-linux-android` fails at
  `ring`, which needs the NDK's clang). GitHub release assets download fine, e.g. ktlint
  (`https://github.com/pinterest/ktlint/releases/download/1.5.0/ktlint`) to check Kotlin
  formatting locally; `pip install actionlint-py` checks workflows.

## Core API

In `core/src/root_timeline.rs` and `core/src/player.rs`:

- `Player::root_timeline()` returns a `RootTimeline`: the root clip's scenes (from
  `DefineSceneAndFrameLabelData`, which only AS3 movies usually have; Flash merges AS1/AS2
  scenes into one timeline), frame labels, current frame, total and loaded frames, and
  `game_scenes`.
- `Player::goto_and_play_root_frame(frame)` jumps the root timeline (1-based).
- `GameScenes { names, current }`, with `previous()` and `next()`, is the scene list of games
  that keep their own (see below), and `Player::goto_game_scene(index)` switches to one.
- Tests: `cargo test -p ruffle_core --lib root_timeline`. They build their game movies with
  the `swf` crate, in the variable layouts below, and with exported clips for the version
  without a list.

### How games with their own scene list are handled

A movie counts as such a game when its root timeline has a `setScreen` function and an
array in `Qvar` or `scenelist`, or, without those, exports movie clips named `screen1`,
`screen2`... (the list is then `mainmenu`, the numbered ones in order, and the other
`screen_*` ones, like `screen_end`). The scene being shown is the one named in
`screen_name`, or else numbered (1-based) in `Qpos`, `pos` or `Qmod`, or else, for exported
clips, the one the root's child `screen` is an instance of. To switch, core does what the
games' own controls do: stop all sounds, show the mouse cursor, and call the root's
`setScreen` with:
- the difference between the scene's number and `Qmod`, when the root has `Qmod` (Hitomi's
  `setScreen` moves on by that many scenes);
- else the 1-based number of the scene, when the root has `Qfrm`;
- else its name.
(An earlier approach set `Qscr`, which only works in the versions that check it every
frame.)

## The games' engine

Most of the games tested are made by one developer, with one engine in several versions.
What's known about each, from `tools/swfinfo.py` dumps:

0. **Version without a list** (Avatar The Last Airbender, Baka 1). One SWF, whose scenes are
   movie clips exported as `mainmenu`, `screen1`... (plus `screen_end` or `screen_failed`).
   `setScreen(clip_name)` removes `screen` and attaches the clip as `screen` on the root;
   `gotoScreen(clip_name)` plays `trans_screen` first. Baka also sets `screen_name`, sends
   `fscommand("bar", n + 1)` for `screenN` (1 for the main menu) and `fscommand("mnf", 10)`,
   and checks `Qscr` every frame (1 is the main menu, n is `screen(n - 1)`), stopping sounds
   and showing the mouse first. Its `parts/red.bar` is only its progress bar.

1. **`scenelist` version** (Olympic Contest, April Story). A small launcher SWF that loads
   its parts. Variables on the root: `scenelist` (array of names like `screen_0`),
   `scenepart` (the part each scene is in), `pos` (current, 1-based), `Qscr` (requested).
   `setScreen(name)` shows a scene, after loading its part with `LoadGame(part)` if needed,
   which loads `filepath + filename + part + fileext` (`"parts/" + "obt" + n + ""`, so the
   part files have no extension) into the `game` clip with a `MovieClipLoader`. A
   `gameHandler` checks `Qscr` every frame, and the keys Page Up (33) and Page Down (34)
   (previous and next scene), and M or numpad minus (stop sounds). It only starts when
   `_url.substring(2, 4) == "le"`, which is true for `file://` URLs; elsewhere it opens the
   developer's site. Sends `fscommand("bct", count)` and `fscommand("bar", pos)`. Olympic
   Contest is protected: junk code and decoy constant pools behind branches always taken.
2. **`Qvar`/`Qpos` version** (Nice Christmas 2, Titans 2, Legend of Korra). Also a launcher
   with parts (`parts/nc2`, `parts/titans2`, `parts/loksf`), loaded into the `game` clip
   (`loader.loadClip(_root.game_path, game)`), plus per-screen extra parts through a
   `LoadingManager` class and a `socket()` function (`"parts/nc2" + screen_name +
   parts_progress`). Root variables: `Qvar` (array literal of names: `mainmenu`,
   `screen1`...), `Qmax` (count), `Qpos` (current), `Qscr` (requested, checked by the root's
   `onEnterFrame`, with the same keys). `setScreen(name)`, and `gotoScreen(name)`, which
   plays the `trans_screen` transition first.
3. **`Qvar`/`Qfrm` version** (Mortal Combat parody, Catwoman, Avatar, Hitomi).
   One SWF: the scenes are exported movie clips (`screen1`... or hashed names like
   `e14865a3` in Avatar), attached into the `game` clip. `Qstr` is a comma-separated string
   of the names, `Qvar = Qstr.split(",")`, `Qfrm` the count. No current number:
   `screen_name` holds the current name. `Qscr` is only the first scene, and nothing checks
   it later; there are no Page Up/Down keys. `setScreen(x)` attaches `Qvar[x - 1]`; in Mortal
   Combat and Catwoman it first calls `getposition(x)`, which takes a number or a name, while
   Avatar has no `getposition`, so it needs the number, and attaches to the root. Hitomi's
   `setScreen(x)` is relative: `Qmod = Qmod + int(x)`, back to 1 when that's past `Qfrm` or
   below 1, then attaches `Qvar[Qmod - 1]` into `game` (its scene clips call
   `gotoScreen(1)` to go on); it builds `Qvar` itself when called with `undefined`, which its
   first frame does (with `Qmod` 0). Passing it the scene's number made most choices
   overshoot to the main menu. Hitomi's first frame also carries MD5, RSA and `XMLSocket`
   code for `secured_key.php`, after the `End`, so it never runs. `gotoScreen(x)` attaches
   `trans_screen`, which calls
   `_parent.setScreen(next_screen)` at its frame 10. Sends `fscommand("mnf"/"mnf1", Qfrm)`.
   The scene clips of Catwoman and Avatar have their own frame labels for the stages within a
   scene (`act0`...`act3`, `trans1`...`trans3`, `result`, or `hold1`...`hold5`): a possible
   next feature is listing the labels of the scene being shown (`game.screen`) to jump
   between them.

Unknown to `--classify`, but of the same developer (samples sent so far):

- **Big Queen 2**: a launcher of the `Qvar` version (12 scenes, `socket`, `parts/mq2`); it was
  supported all along, and the menu works in it (tested).
- **Big Queen 1** and **Evil Sorceress** (the menu doesn't work in them yet): launchers that load their first part over
  themselves (`loadMovie("parts/btmq0", this)`, and into `_level0` from `parts/esrm`), so
  the game's code is in the parts. Their arrays are like the `scenelist` version's, with
  other names: `screens` (and `fscommand("bct", 11)`), and `screens_list` with the part of
  each scene in `screens_arr`. Each part (SWF 15, AS2) has its own `setScreen` and
  `gotoScreen`, exports the clips of its scenes (`screen_0`...`screen_3` in `btmq0`,
  `screen_4`...`screen_7` in `btmq1`, `screen_8`...`screen_10` in `btmq2`; `screen_0`,
  `screen_1`, `screen_5` in `esrm1`, `screen_2`...`screen_4` in `esrm2`, `screen_6`...
  `screen_10` in `esrm3`, as `screens_arr` says), and loads the other parts over the root
  in turn (`loadMovie("parts/btmq1", this)`; `esrm` ones into `_level0`). The `btmq` parts
  use `_root.screens` and `_root.Qscr`, which the launcher set before it was replaced.
- **Avatar The Last Airbender** and **Baka 1**: the version without a list (0 above), which
  the menu supports since it lists the exported scene clips.

Other games:

- **Digital Adventure** (another developer, AS3): its scenes look like SWFs embedded in the
  main one (`Main_s0_class`...`Main_s8_class`, from `s0.swf`...`s8.swf`), loaded by a `Main`
  class. Not supported yet; a `--dump` of it shows its classes and methods.

User testing so far: Nice Christmas 2, Olympic Contest, Titans 2 and Mortal Combat switch
scenes. Mortal Combat first did nothing, on the build that set `Qscr`; it works since
Android `6b41de4` (Windows `09eaca375`), which call `setScreen` and pass a number to the
`Qfrm` version. In Avatar the menu shows hashed names like `540e8d62` (the game's own: its
scene 1 is a one-frame clip, its main menu), and most lead to a scene. In Hitomi many led
to the main menu, as its `setScreen` is relative (see above); core gives it the
difference since then, untested by the user so far. On the user's collection,
`--classify` finds 532 of 693 games supported, 10 with frame labels only and 151 unknown;
the user sends samples of the unknown ones first.

## Frontends

- **Desktop** (`desktop/src/gui/menu_bar.rs`): a Scenes menu with the game's scenes or the
  timeline's scenes, Previous and Next Scene, a Frame Labels submenu, and a frame field with
  Go. Texts in `desktop/assets/texts/en-US/main_menu.ftl`. The menu stays open on clicks
  inside it (`PopupCloseBehavior::CloseOnClickOutside`), so the frame field can be used.
- **Web** (`web/src/lib.rs`, `web/packages/core/src/internal/player/inner.tsx`): the context
  menu lists the timeline's scenes. It doesn't have the game scene lists yet.
- **Android**: see the app's `FORK_NOTES.md`.

## tools/swfinfo.py

A script for the user to run on their own games (Python 3, standard library only), whose
output they paste back, since the games themselves aren't shared:

- `python swfinfo.py <files or folders>`: a summary of each SWF: version, frames, scenes,
  labels, stops, gotos, loads, main timeline functions, file names, fscommands and arrays
  in the code, movie clips with labels or long timelines, exported symbols, AS3 classes and
  strings, which other given files it names, and embedded SWFs, described after it.
- `--dump`: also the tags, an outline of the AS3 classes (fields, methods, and what each
  method uses), and a disassembly of all AS1/AS2 code. `--dump-main`: only the main
  timeline's code, for big games.
- `--classify <folders>`: one line per game on how it handles scenes (supported, frame
  labels only, unknown, unreadable), and a tally. SWFs in a subfolder that a game names
  (like `parts/`), or named by another SWF next to them, count as its parts.
- `--unsupported <folders>`: the same, listing only the games that aren't supported (the
  tally still counts all of them), to find the next samples.
- Names with `*` or `?` are expanded by the script, since the Windows command line doesn't.
- `--scenes <files>`: for games with their own scene list, each entry with the clip the
  movie exports under that name (frames, labels, which engine functions and variables its
  code and the code inside it use), or that it has none; then the main timeline's code.

It reads AVM1 code the way it runs (following jumps, branches and functions), so junk after
jumps doesn't hide code, and it ignores garbled constant pools as decoys. Changes are best
checked against Ruffle's test SWFs (`tests/tests/swfs/avm1` and `avm2`), which it should all
read without errors.

`tools/scene-test-swfs` has a small AVM1 assembler (`asm.py`) and `mc_like.py`, which writes
a movie with the scene code of Mortal Combat's version of the engine, used to check that
version's scene switching in core.

## Open questions for the user

- Merging `scene-menu` into the Android app's `main` (which would make each push publish).
- Giving the Android fork its own app ID, so it installs next to the official app.
- Removing Dependabot from this repository, and closing its pull requests.
- Whether to list the stages within a scene (the labels of the scene's clip).
